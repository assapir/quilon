// LLVM code generator for Quilon

use crate::ast::{
    BinaryOperator, Expression, FunctionDeclaration, InterpolationPart, Item, MatchArm,
    MethodDeclaration, Pattern, Program, Type, TypeDeclaration, TypeDefinition, UnaryOperator,
    VariableDeclaration, is_compiler_provided_name, is_operator_symbol,
};
use crate::codegen::debug::DebugInfo;
use crate::lexer::Span;
use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::{AsDIScope, DILocation, DIScope, DIType};
use inkwell::module::Module;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

// The generator's methods live in child modules — one per lowering area — as further
// `impl<'ctx> CodeGenerator<'ctx>` blocks. Children of this file rather than siblings
// under `codegen`, so the state declared below stays private to the generator: a child
// can reach its ancestor's private items, a sibling could not.
mod arrays;
mod assertions;
mod calls;
mod closures;
mod collections;
mod decls;
mod di;
mod exprs;
mod interpolation;
mod intrinsics;
mod mangle;
mod matching;
mod oracle;
mod records;
mod sums;
mod tco;
#[cfg(test)]
mod tests;
mod text;

use mangle::{fmt_parameter_types, mangle_overload, method_symbol, type_mangle};
use oracle::zeroed;

/// Provenance watermark embedded in every native binary. Lowered as an `!llvm.ident`
/// module metadata entry, which LLVM emits into the ELF `.comment` section during object
/// generation (visible via `readelf -p .comment` and `strings`), coexisting with the
/// toolchain's own producer string. A single compile-time constant so there is one source
/// of truth, and no build-date/dynamic content so builds stay reproducible.
pub const WATERMARK: &str = "Built with Quilon by Assaf Sapir - github.com/assapir/quilon";

/// The failure message for a fallible LLVM builder call: `ctx("build return")` turns the
/// builder's error into `"build return: <error>"`. Every IR-emitting call in the generator
/// reports failure this way, so the phrasing lives in one place instead of a closure per
/// call site.
fn ctx<E: std::fmt::Debug>(what: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{what}: {e:?}")
}

/// A saved (possibly-absent) binding for one name, captured so `inline_lambda` can
/// restore whatever a lambda parameter shadowed: its `variables` entry (alloca + LLVM
/// type) and its `var_types` entry (Quilon type for overload mangling).
type SavedBinding<'ctx> = (
    String,
    Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    Option<Type>,
);

/// A closure's call ABI: its source-parameter LLVM types and its return type. The
/// implicit trailing environment pointer is NOT included (every closure call appends it).
/// Recovered at a call site because a closure value (`{ ptr fn, ptr env }`) does not
/// encode its callee signature.
type ClosureSig<'ctx> = (Vec<BasicTypeEnum<'ctx>>, BasicTypeEnum<'ctx>);

/// One captured free variable of a closure, resolved at the lambda site.
/// `slot` is the variable's storage in the enclosing frame — for a by-value (`=`)
/// capture it is the source slot we snapshot from; for a by-reference (`:=`) capture it
/// IS the shared GC cell pointer we store into the environment. `value_ty` is the
/// captured value's LLVM type.
struct Capture<'ctx> {
    name: String,
    slot: PointerValue<'ctx>,
    value_ty: BasicTypeEnum<'ctx>,
    by_ref: bool,
}

/// The per-function emission state: every map keyed by a *variable* name, valid only
/// while emitting one function body. Function emission must start from an empty frame
/// (`take_frame` and drop), and nested emission — lambdas, local functions — must
/// restore the enclosing frame afterwards (`take_frame` … `restore_frame`). A stale
/// entry left behind by a previously emitted function silently mis-routes field access,
/// method dispatch, and overloaded-call mangling for any later variable that reuses the
/// same name.
struct FrameState<'ctx> {
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    record_types: HashMap<String, Vec<String>>,
    var_named_types: HashMap<String, String>,
    var_types: HashMap<String, Type>,
    boxed_vars: std::collections::HashSet<String>,
}

/// Enclosing-function state saved by `suspend_enclosing_function` around emitting a
/// nested function mid-body, and restored by `resume_enclosing_function`.
struct SuspendedFunction<'ctx> {
    block: Option<inkwell::basic_block::BasicBlock<'ctx>>,
    function: Option<FunctionValue<'ctx>>,
    frame: FrameState<'ctx>,
    debug_location: Option<DILocation<'ctx>>,
}

pub struct CodeGenerator<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    // Track record field mappings: variable name -> (field names, field types)
    record_types: HashMap<String, Vec<String>>,
    // Named record types: type name -> field names (declared order)
    named_type_fields: HashMap<String, Vec<String>>,
    // Track which named type a variable was constructed from: var name -> type name
    var_named_types: HashMap<String, String>,
    // Sum-type variant registry: variant (constructor) name -> (tag, owning type name).
    // Built from user `TypeDefinition::Sum` declarations plus the built-in Result. The tag is
    // the variant's declaration index. Drives constructor codegen and tag-based pattern
    // dispatch (generalizing the old hardcoded Ok=0/NotOk=1).
    sum_variants: HashMap<String, (u8, String)>,
    // Declared payload Quilon types per variant (constructor name -> field types).
    // Lets `bind_pattern` record a matched payload binding's type in `var_types`, so an
    // overloaded call on that binding (e.g. `Circle(n) => area(n)`) mangles by the
    // payload's concrete type. (Result's `Ok`/`NotOk` carry `Generic`, which resolves
    // as Num for overloads — see the type checker's `types_match`.)
    variant_payloads: HashMap<String, Vec<Type>>,
    // Per-sum-type canonical payload layout (one LLVM type per payload slot), sized to
    // the widest variant so EVERY value of the type has the same struct shape
    // `{ i8 tag, slot0, slot1, ... }`. This lets a match arm extract any variant's
    // payload slots without going out of range, even when the runtime value was built
    // from a narrower variant. Keyed by sum-type name. USER sum types are entered here
    // as they're declared; the predefined `Result` is entered up front with a SINGLE
    // canonical `{ptr,i64}` payload slot (see `register_builtin_sum_types`) so that
    // every Result — whatever its `Ok`/`NotOk` payload — shares one LLVM shape
    // `{ i8, {ptr,i64} }` and can cross a generic `(r :: Result)` boundary.
    sum_layouts: HashMap<String, Vec<BasicTypeEnum<'ctx>>>,
    current_function: Option<FunctionValue<'ctx>>,
    // Names of `:=` (mutable) locals in the CURRENT function that are captured by
    // reference by some nested closure. These are allocated as heap GC cells (boxes)
    // rather than plain stack allocas, so a closure capturing one shares the very same
    // cell — writes from either side are visible to the other and survive the closure
    // escaping its defining frame. Their `variables` entry stores the cell pointer
    // directly; since a load/store of `value_type` works through any pointer, ordinary
    // reads/writes need no special-casing. Recomputed on entry to each function body.
    boxed_vars: std::collections::HashSet<String>,
    // Monotonic counter for naming the lifted top-level function of each lambda
    // (`__lambda_0`, `__lambda_1`, …). Lambdas have no source name of their own.
    lambda_counter: usize,
    // The type oracle: authoritative inferred types for every expression, keyed by span,
    // produced by the type checker (see `TypeOracle`). Codegen consults it at READ sites
    // (array index, record-field access, match-arm result) to recover the *declared*
    // element/field/result LLVM type instead of guessing `f64` from a runtime value.
    // Populated at the start of `generate`; empty before then.
    oracle: TypeOracle,
    // The deferred-value coloring from the taint pass: which expressions evaluate to a
    // deferred (promise) value. Codegen emits the pointer representation for one, and a
    // `force` where a force-set primitive reads it. Empty for pure programs and IR-only
    // tests, which therefore carry no force sites.
    defer: crate::deferral::DeferInfo,
    // Whether this module is being emitted for an ahead-of-time build rather than the JIT.
    // Backs `core.info`'s `runMode`; only the caller knows which it is.
    aot: bool,
    // Overload sets, keyed by name (function names AND operator symbols like `"+"`).
    // Each entry is the list of that name's overload parameter-type signatures. A name
    // is present here iff it is an overload set (operator-named, or 2+ same-named
    // top-level defs); calls/operators to these names dispatch to a NAME-MANGLED
    // function (`mangle_overload`) chosen by exact argument types. Operator builtins
    // (Num/Text `+`, comparisons) are NOT entered here — they keep their inline
    // lowering; only USER operator overloads add an operator symbol to this map.
    // Each member is its `(parameter types, return type)`.
    overloads: HashMap<String, Vec<(Vec<Type>, Type)>>,
    // Every reachable top-level function's LLVM declaration, made before any body is
    // emitted so bodies may call functions later in item order (see
    // `declare_module_function`). Keyed by the declaration's span — its identity — and
    // consumed by `emit_module_function` as each body is filled in.
    predeclared_functions: HashMap<Span, inkwell::values::FunctionValue<'ctx>>,
    // Quilon type of each in-scope local/parameter, for argument-type inference at
    // overloaded call sites (codegen lacks the type checker's full inference, so it
    // tracks just enough — locals, parameters, and constructor results — to mangle).
    var_types: HashMap<String, Type>,
    // Declared return type of each NON-overloaded top-level function, so `infer_type`
    // can give a call's result its real type (not a `Num` default) when that result is
    // an argument to an overloaded call/operator — keeping codegen dispatch in sync
    // with the type checker. (Overloaded callees' returns come from `overloads`.)
    fn_return_types: HashMap<String, Type>,
    // Active self-tail-call optimization context for the function currently being
    // emitted, set up by `generate_function_declaration` only when the body has a self-call in
    // tail position. A tail self-call then overwrites the parameter slots and branches back
    // to `loop_header` instead of emitting a stack-growing `call` + `ret` — guaranteeing
    // self-tail-recursion runs in constant stack (see `Tco` / `generate_tail_expression`).
    tco: Option<Tco<'ctx>>,
    // Named types (records) that define their own `` ` `` render operator override. A
    // value of such a type renders via the user's `Type_op$backtick` method instead of the
    // built-in default (type name). Populated once, in the type-declaration pre-pass.
    render_overrides: std::collections::HashSet<String>,
    // Which types declare each receiver-dispatched method: method name -> the types
    // declaring it. A call resolves against what a type actually declares rather than
    // against whether a symbol of the mangled shape happens to exist (a top-level
    // `Counter_bump` is a function, not `Counter`'s `bump`). Keyed by METHOD name so the
    // overwhelmingly common answer — no type has a method by this name — costs one
    // borrowed lookup on the call-lowering hot path. Filled in source order, so a call
    // sees only the methods declared above it, as the checker does.
    declared_methods: HashMap<String, HashSet<String>>,
    // While emitting the body of a type's own `` ` `` override, this holds that type's
    // name. Rendering the receiver `it` wholesale (a hole that is literally `it`) then
    // falls back to the built-in default instead of re-invoking the override — breaking
    // what would otherwise be unbounded self-recursion at runtime.
    generating_backtick_for: Option<String>,
    // DWARF line-number debug info, installed only for a `--debug` native build (via
    // [`enable_debug`]). When present, each emitted function gets a `DISubprogram` and every
    // expression sets a source location before lowering, so `llvm-dwarfdump` and debuggers
    // can map machine code back to `.qn` lines. `None` on every other path (JIT, `compile`,
    // IR-only tests), which keeps the non-debug output unchanged.
    debug: Option<DebugInfo<'ctx>>,
    // The `DISubprogram` scope of the function currently being emitted, as a `DIScope`.
    // Saved/restored around nested function emission (closures, local fns) so a source
    // location is always attributed to the right function. `None` unless `debug` is set.
    di_scope: Option<DIScope<'ctx>>,
    // DWARF debug types (only under `--debug`). Full field types of every record type
    // (`named_type_fields` keeps only names), and each sum type's variant list — both needed
    // to build a composite type's members/payload slots. Populated by a pre-pass in
    // `generate`, so a type used before its declaration still resolves.
    record_field_types: HashMap<String, Vec<(String, Type)>>,
    sum_variant_defs: HashMap<String, Vec<crate::ast::SumVariant>>,
    // Structural type keys currently being lowered to DWARF, so a (hypothetically) recursive
    // record/sum breaks the cycle with an opaque pointer instead of recursing forever.
    di_building: RefCell<HashSet<String>>,
    // Every source file the program was assembled from, keyed by the `FileId` its spans
    // carry. Read when filling in a `Site` argument at a call site, which needs the call's
    // path, line, column, and the text of its line. Empty for the IR-only codegen tests
    // (a program with no files on disk), where a call site resolves to no location.
    sources: Rc<crate::source_map::SourceMap>,
    // One read-only global per distinct call site that fills in a `Site`, keyed by the
    // whole span (file, start, end) — so the same site asked for twice reuses one constant,
    // while two spans that merely start alike stay distinct (`width` is the span's length).
    site_globals: HashMap<(crate::lexer::FileId, u32, u32), PointerValue<'ctx>>,
    // Byte constants behind those sites' `Text` fields, interned by content: a file's path
    // repeats in every call site in it, and no pass merges duplicate globals at -O0.
    text_constants: HashMap<String, PointerValue<'ctx>>,
    // Every NON-overloaded top-level function whose LAST parameter is a `Site`, mapped to
    // its full parameter count — all a call site needs to know to fill that argument in
    // (see `fills_call_site`). Only such functions are listed, so an ordinary call looks up
    // a miss and copies nothing. (Overloaded callees' parameters come from `overloads`.)
    fn_call_site_arity: HashMap<String, usize>,
}

/// The loop-lowering context for self-tail-call optimization of one function. Present
/// (in `CodeGenerator::tco`) only while emitting a function whose body has at least one
/// self-call in tail position. Classic TCO transform: the body's parameter `=`-bindings
/// become mutable slots (`parameter_slots`), and a tail self-call stores its argument values
/// into those slots and `br`s back to `header` — turning the recursion into a loop.
struct Tco<'ctx> {
    /// The LLVM symbol of the function being optimized (mangled if overloaded). A `Call`
    /// is a self-tail-call only if it resolves to exactly this symbol with matching arity.
    self_symbol: String,
    /// The function being optimized — the one a declined back-edge calls instead. Held as
    /// a value so that path needs no lookup: the callee of a *self*-call is never in doubt.
    /// Its parameter types are also the slot types (both come from the same declaration,
    /// in order), which is what `emit_tail_self_call` checks its argument values against.
    function: FunctionValue<'ctx>,
    /// The function's parameter alloca slots, in declaration order. A tail self-call
    /// recomputes the args and rewrites these slots (its length is the arity).
    parameter_slots: Vec<PointerValue<'ctx>>,
    /// The loop header — the block a tail self-call branches back to. Positioned right
    /// after the parameter slots are (re)loaded into the `variables` map for the body.
    header: inkwell::basic_block::BasicBlock<'ctx>,
}

/// Codegen-side view of the type checker's [`TypeTable`] — the "type oracle".
///
/// # Why this exists
/// A runtime `BasicValueEnum::get_type()` cannot name the type a read should produce: it
/// loses element and field types at every READ site and reports `f64`. That corrupts any
/// non-`f64` payload nested in a composite — `Text` in a record or array, nested arrays,
/// `Ok(text)`/`NotOk(text)`. So codegen reads the *declared* types from here instead, as
/// the checker computed them.
///
/// # API
/// The single primitive is [`TypeOracle::type_at`] — the `Type` the checker recorded for a
/// source `Span` — with [`TypeOracle::expression_type`] the expression-shaped convenience
/// over it. The checker records the *result* type of every node, so the element type of an
/// `arr[i]` is `expression_type(<the Index node>)`, the type of `rec.field` is
/// `expression_type(<the FieldAccess node>)`, and a `match`'s result is
/// `expression_type(<the Match node>)` — there is no need for per-shape accessors, the read
/// site just asks for the type of the whole node it is lowering. A parameter is not an
/// expression and is looked up by its own span (see `parameter_type`), which is how a
/// lambda parameter typed from context reaches codegen.
///
/// Lookups are by `Span` (one per AST node), so the oracle is AST-shape-agnostic and
/// additive: new expression kinds get types recorded automatically by `infer_expression`. A
/// `None` means the span wasn't recorded (e.g. the IR-only codegen tests that skip the
/// type-check pass); callers fall back to their historical `f64` assumption.
///
/// A `Span` is a byte range plus the identity of the file it indexes into, which is what
/// makes it a sound key here: the `<<` import system lexes each module independently
/// (offsets restart at 0) before merging items into one `Program`, so two expressions in
/// different modules routinely share a byte range. Keyed on the range alone they would
/// collide in the table (last-inferred wins) and codegen would read one module's type for
/// another module's expression — a wrong overload member, a wrong element repr. The file
/// id keeps every node's key distinct across the merge.
#[derive(Default)]
struct TypeOracle {
    table: crate::typechecker::TypeTable,
}

/// A piece to be laid into a freshly GC-allocated array by `build_array_from_parts`:
/// an `Inline` single element (stored at the running offset), or a `Spread` whole
/// `{ptr,size}` array whose elements are memcpy'd in bulk. Shared by the `<-` spread
/// lowering (`[<-a, 4, <-b]`) and `+` array concatenation (`a + b`).
enum ArrayPart<'v> {
    Inline(BasicValueEnum<'v>),
    Spread(BasicValueEnum<'v>),
}

/// Where an array method reads its receiver's elements from: a materialized `{ptr, size}`
/// array's backing store, or — for a `lo <- hi` receiver lowered lazily — the range's own
/// bounds, each element computed as `(f64)(lo + i * step)` with nothing allocated.
#[derive(Clone, Copy)]
enum ElementSource<'v> {
    Memory(PointerValue<'v>),
    Range {
        lo: inkwell::values::IntValue<'v>,
        step: inkwell::values::IntValue<'v>,
    },
}

impl<'ctx> CodeGenerator<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();

        let mut codegen = CodeGenerator {
            context,
            module,
            builder,
            variables: HashMap::new(),
            record_types: HashMap::new(),
            named_type_fields: HashMap::new(),
            var_named_types: HashMap::new(),
            sum_variants: HashMap::new(),
            variant_payloads: HashMap::new(),
            sum_layouts: HashMap::new(),
            current_function: None,
            boxed_vars: std::collections::HashSet::new(),
            lambda_counter: 0,
            oracle: TypeOracle::default(),
            defer: crate::deferral::DeferInfo::default(),
            aot: false,
            overloads: HashMap::new(),
            predeclared_functions: HashMap::new(),
            var_types: HashMap::new(),
            fn_return_types: HashMap::new(),
            tco: None,
            render_overrides: std::collections::HashSet::new(),
            declared_methods: HashMap::new(),
            generating_backtick_for: None,
            debug: None,
            di_scope: None,
            record_field_types: HashMap::new(),
            sum_variant_defs: HashMap::new(),
            di_building: RefCell::new(HashSet::new()),
            sources: Rc::new(crate::source_map::SourceMap::default()),
            site_globals: HashMap::new(),
            text_constants: HashMap::new(),
            fn_call_site_arity: HashMap::new(),
        };
        codegen.register_builtin_sum_types();
        codegen.register_builtin_record_types();
        codegen
    }

    /// Register the predefined `Result` variants: `Ok` is tag 0, `NotOk` is tag 1.
    /// Result's payloads are generic (`Ok(T)` / `NotOk(E)`) and its two variants routinely
    /// carry DIFFERENT concrete types (e.g. `Ok(num)` vs `NotOk(text)`), so Result is given
    /// ONE canonical payload slot of type `{ptr,i64}` — wide enough to hold any payload —
    /// making every Result the single LLVM shape `{ i8 tag, {ptr,i64} slot }`. A scalar
    /// payload is PACKED into that slot at construction (`pack_result_payload`) and UNPACKED
    /// back to its concrete type at a match binding (`unpack_result_payload`); a Text/array
    /// payload is already `{ptr,i64}` and fills the slot directly. This uniform shape is what
    /// lets a Result carrying any payload cross a generic `(r :: Result)` parameter/return.
    fn register_builtin_sum_types(&mut self) {
        self.sum_variants
            .insert("Ok".to_string(), (0u8, "Result".to_string()));
        self.sum_variants
            .insert("NotOk".to_string(), (1u8, "Result".to_string()));
        // The single canonical payload slot: a `{ptr,i64}` big enough for any payload.
        self.sum_layouts.insert(
            "Result".to_string(),
            vec![self.ptr_len_struct_type().into()],
        );
        // Result's payloads are generic (`Ok(T)` / `NotOk(E)`); a `Generic` binding
        // resolves as Num for overload dispatch (see the type checker's `types_match`).
        let generic = |n: &str| Type::Generic {
            name: n.to_string(),
            arguments: vec![],
        };
        self.variant_payloads
            .insert("Ok".to_string(), vec![generic("T")]);
        self.variant_payloads
            .insert("NotOk".to_string(), vec![generic("E")]);
    }

    /// The LLVM struct a `Site` record lowers to — `{ {ptr,i64}, double, double, {ptr,i64},
    /// double }` for the declared [`crate::ast::site_fields`].
    ///
    /// Public so a test can hold the runtime's hand-written `QlSite` mirror
    /// (`quilon_rt::QlSite`, which every fallible intrinsic receives) to the layout actually
    /// emitted: the two are connected by nothing but agreement, and a drifted field order
    /// would make the runtime read a text pointer as a line number rather than fail to build.
    pub fn site_struct_type(
        context: &'ctx Context,
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        let generator = CodeGenerator::new(context, "site_layout");
        generator.record_struct_type(&crate::ast::site_fields())
    }

    /// Register the built-in `Site` record so a `:: Site` parameter and the field reads on
    /// it (`site.line`) resolve like any declared record's — the checker registers the same
    /// type (`ast::site_type`), and `site_value` fills one in at a call site.
    fn register_builtin_record_types(&mut self) {
        let fields = crate::ast::site_fields();
        self.named_type_fields.insert(
            crate::ast::SITE_TYPE_NAME.to_string(),
            fields.iter().map(|(n, _)| n.clone()).collect(),
        );
        self.record_field_types
            .insert(crate::ast::SITE_TYPE_NAME.to_string(), fields);
    }

    /// Access the underlying LLVM module after `generate` has populated it.
    /// Used by the JIT runner to create an execution engine in-process.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    /// Install the **type oracle**: the per-expression `TypeTable` the type checker
    /// already produced, which codegen consults at read sites to recover precise
    /// element/field/match-result types. Every real compilation path hands over the
    /// table from the front end's check, so the program is type-checked exactly once.
    /// Without it the oracle is empty, every lookup misses, and read sites fall back to
    /// their historical `f64` assumption — which is what the IR-only codegen tests (no
    /// typecheck pass) rely on.
    pub fn set_type_table(&mut self, table: crate::typechecker::TypeTable) {
        self.oracle = TypeOracle::new(table);
    }

    /// Install the **deferred-value coloring** from the taint pass. Codegen consults it to
    /// emit the promise representation for deferred values and a `force` at force-set sites,
    /// and to decide whether to run the entry on a scheduler fiber. Left empty (nothing
    /// deferred) for the IR-only codegen tests, which then compile exactly as before.
    pub fn set_defer_info(&mut self, defer: crate::deferral::DeferInfo) {
        self.defer = defer;
    }

    /// Mark this module as an ahead-of-time build, which `core.info`'s `runMode` reports.
    pub fn set_aot(&mut self) {
        self.aot = true;
    }

    /// Install the compilation's [`SourceMap`](crate::source_map::SourceMap) — the path and
    /// text of every file its spans point into. Codegen needs it to fill in a `Site`
    /// argument at a call site; without it (the IR-only codegen tests) a call site has no
    /// resolvable location and the filled-in `Site` reads as unknown.
    pub fn set_source_map(&mut self, sources: Rc<crate::source_map::SourceMap>) {
        self.sources = sources;
    }

    pub fn generate(&mut self, program: &Program) -> Result<String, String> {
        // Pre-pass: register all user sum-type variants so constructors and pattern
        // dispatch resolve regardless of declaration order relative to their uses.
        for item in &program.items {
            if let Item::TypeDeclaration(TypeDeclaration {
                name,
                type_definition: TypeDefinition::Sum { variants, .. },
                ..
            }) = item
            {
                self.register_sum_variants(name, variants)?;
            }
            // Under `--debug` only, keep every record type's full field types (name + type) so
            // the DWARF builders can build its composite members regardless of declaration order
            // (`named_type_fields` keeps only names, which is all the non-debug paths need, so
            // this deep clone is skipped entirely when debug info is off).
            if self.debug.is_some()
                && let Item::TypeDeclaration(TypeDeclaration {
                    name,
                    type_definition: TypeDefinition::Record { fields, .. },
                    ..
                }) = item
            {
                self.record_field_types.insert(name.clone(), fields.clone());
            }
        }

        // Pre-pass: discover overload sets (operator-named, or 2+ same-named defs),
        // mirroring the type checker. Their definitions are name-mangled by parameter
        // type and dispatched by exact argument type at each call/operator site.
        let mut fn_counts: HashMap<&str, usize> = HashMap::new();
        for item in &program.items {
            if let Item::FunctionDeclaration(declaration) = item
                && !declaration.is_inert_corelib_placeholder()
            {
                *fn_counts.entry(declaration.name.as_str()).or_insert(0) += 1;
            }
        }
        for item in &program.items {
            if let Item::FunctionDeclaration(declaration) = item
                && !declaration.is_inert_corelib_placeholder()
                && (is_operator_symbol(&declaration.name)
                    || fn_counts
                        .get(declaration.name.as_str())
                        .copied()
                        .unwrap_or(0)
                        > 1
                    || is_compiler_provided_name(&declaration.name))
                && declaration.name != "^"
            {
                let parameters = self.parameter_types(&declaration.parameters);
                // The return type drives argument-type inference for a value bound to
                // an overloaded call/operator (e.g. a user `+` returning a record).
                let ret = declaration
                    .declared_return_type()
                    .cloned()
                    .unwrap_or(Type::Num);
                self.overloads
                    .entry(declaration.name.clone())
                    .or_default()
                    .push((parameters, ret));
            }
        }

        // Pre-pass: an operator overload now lives inside a type (as a member). Register
        // each type's operator members as members of the operator's overload set, with the
        // receiver `it` as the left operand — so `a <op> b` mangles to and dispatches
        // through the same per-signature symbol the member is emitted under.
        for item in &program.items {
            if let Item::TypeDeclaration(declaration) = item {
                let self_type = Type::named_ref(&declaration.name);
                for method in declaration.type_definition.methods() {
                    if !is_operator_symbol(&method.name) {
                        continue;
                    }
                    // The `%` hash hook is a UNARY member (`it` only) whose overload takes
                    // just the receiver; every other operator member is binary (`it` + one
                    // explicit right operand).
                    let parameters = if method.name == "%" && method.parameters.is_empty() {
                        vec![self_type.clone()]
                    } else if method.parameters.len() == 1 {
                        vec![
                            self_type.clone(),
                            self.parameter_type(&method.parameters[0]),
                        ]
                    } else {
                        continue;
                    };
                    let ret = method.return_type.clone().unwrap_or(Type::Num);
                    self.overloads
                        .entry(method.name.clone())
                        .or_default()
                        .push((parameters, ret));
                }
            }
        }

        // Pre-pass: record each NON-overloaded top-level function's declared return
        // type, so `infer_type` can give a call result its real type when it feeds an
        // overloaded call/operator (keeps codegen dispatch in sync with the checker).
        for item in &program.items {
            if let Item::FunctionDeclaration(declaration) = item
                && !declaration.is_inert_corelib_placeholder()
                && !self.overloads.contains_key(&declaration.name)
            {
                if let Some(ret) = declaration.declared_return_type() {
                    self.fn_return_types
                        .insert(declaration.name.clone(), ret.clone());
                }
                let parameters = self.parameter_types(&declaration.parameters);
                if crate::ast::takes_call_site(&parameters) {
                    self.fn_call_site_arity
                        .insert(declaration.name.clone(), parameters.len());
                }
            }
        }

        // A function nothing can reach from `^` is not emitted. Importing a module brings in
        // every function it defines, so without this a program that calls one assertion emits
        // — and, under the JIT, compiles — all of them. The analysis over-approximates (see
        // `ast::reachability`). `None` means there is no `^` to measure from, so nothing is
        // pruned.
        let reachable = crate::ast::reachability::reachable_functions(program);

        // Pre-pass: DECLARE every top-level function that will be emitted, so a body may
        // call a function later in item order. Item order still governs name RESOLUTION
        // (no hoisting — the checker enforced it); this only decouples emission from it,
        // which the compiler's own cross-module calls need: a member call lowers to its
        // `core.text` implementation wherever the module landed in the merge.
        for item in program.items.iter() {
            if let Item::FunctionDeclaration(declaration) = item
                && declaration.emits_module_function()
                && reachable
                    .as_ref()
                    .is_none_or(|reachable| reachable.contains(declaration.name.as_str()))
            {
                let function = self.declare_module_function(declaration)?;
                self.predeclared_functions
                    .insert(declaration.span.clone(), function);
            }
        }

        // Generate code for all top-level items. Reset the current-function context
        // before each one: a top-level item is never nested, so codegen must not see a
        // stale function left over from the previous top-level declaration (which would make it
        // look like a nested/local declaration — see `generate_function_declaration`).
        for item in program.items.iter() {
            if let Item::FunctionDeclaration(declaration) = item
                && let Some(reachable) = reachable.as_ref()
                && !reachable.contains(declaration.name.as_str())
            {
                continue;
            }
            self.current_function = None;
            self.generate_item(item)?;
        }

        // Every pre-declared function must have had its body filled in — a leftover means
        // the pre-pass and body emission disagreed on what gets emitted, which would
        // otherwise only surface as a bodiless declaration failing module verification,
        // nowhere near the cause.
        debug_assert!(
            self.predeclared_functions.is_empty(),
            "pre-declared but never emitted: the pre-pass and `generate_function_declaration` \
             disagree on what is emitted"
        );

        // Check if entry point function (^) exists and generate C main wrapper.
        // Pass `^`'s DECLARED Quilon parameter types so the wrapper can dispatch on the
        // real types (`[]Text` / `[|Text => Text|]`) — the lowered LLVM types are
        // ambiguous (`Text`, records, sum types, and arrays all become `{ ptr, i64 }`
        // structs), so dispatching on the LLVM shape would mis-route them.
        if self.module.get_function("^").is_some() {
            let entry_parameters: Vec<Type> = program
                .items
                .iter()
                .find_map(|item| match item {
                    Item::FunctionDeclaration(declaration) if declaration.name == "^" => {
                        Some(self.parameter_types(&declaration.parameters))
                    }
                    _ => None,
                })
                .unwrap_or_default();
            self.generate_main_wrapper(&entry_parameters)?;
        }

        // Embed the provenance watermark as an `!llvm.ident` entry (harmless for the JIT
        // path, which produces no artifact to carry it).
        let ident = self.context.metadata_string(WATERMARK);
        let ident_node = self.context.metadata_node(&[ident.into()]);
        self.module
            .add_global_metadata("llvm.ident", &ident_node)
            .expect("llvm.ident metadata node is always a valid node");

        // Resolve all debug-info forward references before anything reads the metadata.
        // The module verifier validates debug info, so this must precede `verify` (and any
        // later object emission). A no-op when debug info was never enabled.
        if let Some(debug) = self.debug.as_ref() {
            debug.finalize();
        }

        // Verify the module
        if let Err(e) = self.module.verify() {
            return Err(format!("Module verification failed: {}", e));
        }

        // Return the LLVM IR as a string
        Ok(self.module.print_to_string().to_string())
    }

    fn generate_main_wrapper(&mut self, entry_parameters: &[Type]) -> Result<(), String> {
        // Create C-compatible main: `int main(int argc, char** argv, char** envp)`.
        // The third (`envp`) parameter is the POSIX/glibc extension to C `main`; passing
        // it is harmless even for a program that only declares `args`, and it is how we
        // thread the environment in for an `^(args, env)` entry point.
        let i32_type = self.context.i32_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());

        let main_type =
            i32_type.fn_type(&[i32_type.into(), ptr_type.into(), ptr_type.into()], false);

        let main_fn = self.module.add_function("main", main_type, None);
        // The generated wrapper has no source of its own; attribute it to the file header so
        // its instructions (GC init, the call into `^`) carry a valid debug location — the
        // verifier requires one on a call to a function that itself has debug info.
        let main_span = Span::in_root(0, 0);
        let saved_scope = self.begin_di_entry_shim(main_fn, &main_span);
        let argc = main_fn.get_nth_param(0).unwrap().into_int_value();
        let argv = main_fn.get_nth_param(1).unwrap().into_pointer_value();
        let envp = main_fn.get_nth_param(2).unwrap().into_pointer_value();

        let entry = self.context.append_basic_block(main_fn, "entry");
        self.builder.position_at_end(entry);

        // Initialize the Boehm GC before any user code runs. Compiled programs
        // allocate heap memory (Text, sum payloads) via GC_malloc.
        let gc_init = self.get_intrinsic("__gc_init")?;
        self.builder
            .build_call(gc_init, &[], "")
            .map_err(ctx("Failed to call GC init"))?;

        // Every entry runs on a scheduler fiber, so every `@` primitive it reaches has a
        // fiber to park on. The dispatch goes into a `__ql_entry` thunk, and `main` runs the
        // thunk through `__run_fiber_main`.
        //
        // Gating this on whether the program reaches an `@` primitive would save nothing.
        // Every binary carries the scheduler anyway: `__run_fiber_main` sits in the
        // `INTRINSICS` retention table (`quilon-rt/src/lib.rs`), and the runtime's codegen
        // units pull in the rest. A gate would only add a second GC root-scanning regime to
        // keep correct, since an entry on the main thread scans through Boehm's ordinary
        // stack scan and one on a fiber scans through `push_other_roots` and a per-switch
        // `GC_set_stackbottom`.
        //
        // Start-up costs a fixed ~320us on aarch64: an epoll instance from `Reactor::new`,
        // the seed fiber's stack mapping and guard page, and registering that range with the
        // GC. A program that never parks uses none of it, so `scheduler::run` could reclaim
        // it with a lazy reactor. Shrinking the seed stack would not — `^` recurses on it
        // (see `scheduler::SEED_STACK_SIZE`).
        let entry_fn = self.module.add_function("__ql_entry", main_type, None);
        let thunk_scope = self.begin_di_entry_shim(entry_fn, &main_span);
        let thunk_argc = entry_fn.get_nth_param(0).unwrap().into_int_value();
        let thunk_argv = entry_fn.get_nth_param(1).unwrap().into_pointer_value();
        let thunk_envp = entry_fn.get_nth_param(2).unwrap().into_pointer_value();
        let thunk_block = self.context.append_basic_block(entry_fn, "entry");
        self.builder.position_at_end(thunk_block);
        let exit_code =
            self.emit_entry_dispatch(entry_parameters, thunk_argc, thunk_argv, thunk_envp)?;
        self.builder
            .build_return(Some(&exit_code))
            .map_err(ctx("Failed to build entry-thunk return"))?;
        self.end_di_scope(thunk_scope);

        // Back in `main`: run the thunk on a scheduler fiber; its result is the exit code.
        // Re-seed the builder's debug location to `main`'s scope — emitting the thunk left
        // it pointing at `__ql_entry`'s subprogram, and the verifier rejects an instruction
        // whose `!dbg` scope is a different function than the one it lives in.
        self.builder.position_at_end(entry);
        self.set_debug_loc(&main_span);
        let runner = self.get_intrinsic("__run_fiber_main")?;
        let entry_ptr = entry_fn.as_global_value().as_pointer_value();
        use inkwell::values::AnyValue;
        let return_val = self
            .builder
            .build_call(
                runner,
                &[entry_ptr.into(), argc.into(), argv.into(), envp.into()],
                "run_main",
            )
            .map_err(ctx("Failed to run entry on a fiber"))?
            .as_any_value_enum()
            .into_int_value();

        self.builder
            .build_return(Some(&return_val))
            .map_err(ctx("Failed to build return"))?;

        self.end_di_scope(saved_scope);
        Ok(())
    }

    /// Emit the entry-point dispatch into the current block and return the i32 exit code:
    /// build `args`/`env` from `argc`/`argv`/`envp` per `^`'s declared signature, call `^`,
    /// then convert its result. Emitted into the `__ql_entry` thunk. Separate from
    /// `generate_main_wrapper` because it is one piece: what `^`'s three legal shapes mean
    /// to codegen.
    fn emit_entry_dispatch(
        &mut self,
        entry_parameters: &[Type],
        argc: inkwell::values::IntValue<'ctx>,
        argv: inkwell::values::PointerValue<'ctx>,
        envp: inkwell::values::PointerValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let i32_type = self.context.i32_type();
        // Get the ^ (entry point) function
        let user_entry = self
            .module
            .get_function("^")
            .ok_or_else(|| "Entry point function ^ not found".to_string())?;

        // Dispatch on `^`'s DECLARED Quilon parameter types (not the lowered LLVM types:
        // `Text`/record/sum/array all lower to `{ ptr, i64 }` structs, so the LLVM shape
        // can't tell them apart — dispatching on it would silently call a `Text` parameter
        // with the argv array). The supported signatures are `^()`, `^(args :: []Text)`,
        // and `^(args :: []Text, env :: [|Text => Text|])`. We match on the EXACT
        // element/key/value types — the runtime builds `Text` args and a `Text => Text`
        // env Map, so a `[]Num` (or any other element) parameter must NOT reach the array arm.
        let is_text_array = |t: &Type| matches!(t, Type::Array(e) if **e == Type::Text);
        let is_text_map =
            |t: &Type| matches!(t, Type::Map(k, v) if **k == Type::Text && **v == Type::Text);

        // `argc` arrives as the C `int` (i32); widen it to the i64 the runtime expects.
        let argc_i64 = self
            .builder
            .build_int_s_extend(argc, self.context.i64_type(), "argc_i64")
            .map_err(ctx("Failed to widen argc"))?;

        // Build the real `args :: []Text` from argc/argv (used by the modern forms).
        let build_args = |me: &Self| -> Result<BasicValueEnum<'ctx>, String> {
            let f = me.get_intrinsic("__argv_to_text_array")?;
            use inkwell::values::AnyValue;
            Ok(me
                .builder
                .build_call(f, &[argc_i64.into(), argv.into()], "args_arr")
                .map_err(ctx("Failed to build argv array"))?
                .as_any_value_enum()
                .into_struct_value()
                .into())
        };
        // Build the real `env :: [|Text => Text|]` Map from envp (used by the 2-arg modern
        // form). A Map value is a single opaque pointer to its runtime representation.
        let build_env = |me: &Self| -> Result<BasicValueEnum<'ctx>, String> {
            let f = me.get_intrinsic("__envp_to_map")?;
            use inkwell::values::AnyValue;
            Ok(me
                .builder
                .build_call(f, &[envp.into()], "env_map")
                .map_err(ctx("Failed to build envp map"))?
                .as_any_value_enum()
                .into_pointer_value()
                .into())
        };

        let unsupported = || -> String {
            format!(
                "Entry point ^ has an unsupported signature ({}). \
                 Valid signatures: '() -> Num', '(args :: []Text) -> Num', \
                 '(args :: []Text, env :: [|Text => Text|]) -> Num'.",
                fmt_parameter_types(entry_parameters)
            )
        };

        let result = match entry_parameters {
            // `^() -> Num`
            [] => self
                .builder
                .build_call(user_entry, &[], "entry_result")
                .map_err(ctx("Failed to call entry point"))?,
            // `^(args :: []Text) -> Num`
            [a] if is_text_array(a) => {
                let args = build_args(self)?;
                self.builder
                    .build_call(user_entry, &[args.into()], "entry_result")
                    .map_err(ctx("Failed to call entry point"))?
            }
            // `^(args :: []Text, env :: [|Text => Text|]) -> Num`
            [a, e] if is_text_array(a) && is_text_map(e) => {
                let args = build_args(self)?;
                let env = build_env(self)?;
                self.builder
                    .build_call(user_entry, &[args.into(), env.into()], "entry_result")
                    .map_err(ctx("Failed to call entry point"))?
            }
            // Any other signature (e.g. `^(x :: Text)`, `^(args :: []Num)` with a
            // non-`Text` element, `^(a :: Num, b :: Num)`, `^(env :: [|Text => Text|])`
            // without args, 3+ parameters) is rejected with a clear diagnostic instead of a silent
            // miscompile or an LLVM verification crash.
            _ => return Err(unsupported()),
        };

        // Convert result to i32
        use inkwell::values::AnyValue;
        let return_val = match result.as_any_value_enum() {
            inkwell::values::AnyValueEnum::FloatValue(f) => {
                // Convert double to i32
                self.builder
                    .build_float_to_signed_int(f, i32_type, "result_int")
                    .map_err(ctx("Failed to convert result"))?
            }
            _ => {
                // Return 0 if not a numeric result
                i32_type.const_zero()
            }
        };

        Ok(return_val)
    }

    fn generate_item(&mut self, item: &Item) -> Result<(), String> {
        match item {
            Item::VariableDeclaration(declaration) => {
                self.generate_variable_declaration(declaration)
            }
            Item::FunctionDeclaration(declaration) => {
                self.generate_function_declaration(declaration)
            }
            Item::TypeDeclaration(declaration) => self.generate_type_declaration(declaration),
        }
    }

    /// Register a sum type: map each variant to `(tag, type_name)` (tag = declaration index)
    /// and record the type's canonical payload layout from [`payload_slot_types`].
    fn register_sum_variants(
        &mut self,
        type_name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<(), String> {
        // Under `--debug` only, keep the whole variant list so the DWARF builders can build the
        // sum's payload slots (skipped otherwise — normal codegen sizes sums from `sum_layouts`).
        if self.debug.is_some() {
            self.sum_variant_defs
                .insert(type_name.to_string(), variants.to_vec());
        }
        for (tag, variant) in variants.iter().enumerate() {
            self.sum_variants
                .insert(variant.name.clone(), (tag as u8, type_name.to_string()));
            // Record the variant's declared (concrete) payload types so a match arm's
            // payload binding gets its real type for overloaded-call mangling.
            self.variant_payloads
                .insert(variant.name.clone(), variant.fields.clone());
        }

        let layout = self.payload_slot_types(variants)?;
        self.sum_layouts.insert(type_name.to_string(), layout);
        Ok(())
    }

    /// Detach the current per-function frame (see `FrameState`), leaving an empty one.
    /// Pair with `restore_frame` around nested function emission; when starting a fresh
    /// top-level function the detached frame is dead and is simply dropped.
    fn take_frame(&mut self) -> FrameState<'ctx> {
        FrameState {
            variables: std::mem::take(&mut self.variables),
            record_types: std::mem::take(&mut self.record_types),
            var_named_types: std::mem::take(&mut self.var_named_types),
            var_types: std::mem::take(&mut self.var_types),
            boxed_vars: std::mem::take(&mut self.boxed_vars),
        }
    }

    /// Reinstate a frame detached by `take_frame`.
    fn restore_frame(&mut self, frame: FrameState<'ctx>) {
        self.variables = frame.variables;
        self.record_types = frame.record_types;
        self.var_named_types = frame.var_named_types;
        self.var_types = frame.var_types;
        self.boxed_vars = frame.boxed_vars;
    }

    /// Suspend the enclosing function (frame, insert block, current function) to emit a
    /// nested one mid-body. Also saves and clears the builder's `!dbg` location, so an
    /// instruction emitted back in the enclosing body right after resuming isn't stamped
    /// with the nested function's scope. Pair with [`resume_enclosing_function`].
    fn suspend_enclosing_function(&mut self) -> SuspendedFunction<'ctx> {
        let block = self.builder.get_insert_block();
        let function = self.current_function;
        let frame = self.take_frame();
        let debug_location = self.builder.get_current_debug_location();
        self.builder.unset_current_debug_location();
        SuspendedFunction {
            block,
            function,
            frame,
            debug_location,
        }
    }

    /// Reinstate the state suspended by [`suspend_enclosing_function`] once the nested
    /// function's body is emitted.
    fn resume_enclosing_function(&mut self, saved: SuspendedFunction<'ctx>) {
        self.restore_frame(saved.frame);
        self.current_function = saved.function;
        if let Some(block) = saved.block {
            self.builder.position_at_end(block);
        }
        if let Some(loc) = saved.debug_location {
            self.builder.set_current_debug_location(loc);
        }
    }
}

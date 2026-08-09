// LLVM code generator for Quilon

use crate::ast::{
    BinOp, Expr, FunctionDecl, Item, MatchArm, MethodDecl, Pattern, Program, Type, TypeDecl,
    TypeDef, UnaryOp, VarDecl, is_operator_symbol,
};
use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use std::collections::HashMap;

/// Names that the compiler provides built-in overloads for (`print`/`eprint`, lowered
/// to runtime intrinsics). A user definition of one ADDS an overload member (and is
/// mangled), rather than shadowing the built-in single-arg Num/Text/Bool forms.
fn is_builtin_overload_name(name: &str) -> bool {
    matches!(name, "print" | "eprint")
}

/// A saved (possibly-absent) binding for one name, captured so `inline_lambda` can
/// restore whatever a lambda parameter shadowed: its `variables` entry (alloca + LLVM
/// type) and its `var_types` entry (Quilon type for overload mangling).
type SavedBinding<'ctx> = (
    String,
    Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    Option<Type>,
);

/// A short, mangling-safe tag for a Quilon type used in overload name mangling. Must be
/// deterministic and identical at definition and call sites (built from the declared
/// parameter type and from the inferred argument type respectively).
fn type_mangle(ty: &Type) -> String {
    match ty {
        Type::Num => "N".to_string(),
        Type::Text => "T".to_string(),
        Type::Bool => "B".to_string(),
        Type::Unit => "U".to_string(),
        Type::Array(elem) => format!("A{}", type_mangle(elem)),
        Type::Named { name, .. } | Type::Sum { name, .. } => format!("named${}", name),
        // A not-yet-concrete sum payload (`Generic`) resolves as `Num` for overload
        // dispatch (see the type checker's `types_match`), so it mangles to the Num tag
        // — keeping codegen's chosen symbol in agreement with the checker.
        Type::Generic { .. } => "N".to_string(),
        // Any other shape (e.g. a function type) — a stable, mangling-safe fallback.
        other => format!("X{:?}", other)
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '$')
            .collect(),
    }
}

/// Render an entry point's declared parameter types as a readable signature fragment
/// (comma-joined `Num`/`Text`/`[]Text`-style labels) for the unsupported-signature
/// diagnostic. `()` renders as an empty string. Uses the shared `ast::type_label` so
/// codegen and the type checker render types identically.
fn fmt_param_types(params: &[Type]) -> String {
    params
        .iter()
        .map(crate::ast::type_label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The distinct LLVM symbol for one overload member: its name plus a per-parameter
/// type tag. Operator symbols (which aren't valid LLVM identifiers) are spelled out so
/// e.g. `+` on `(Point, Point)` becomes `op.add$named$Point$named$Point`.
fn mangle_overload(name: &str, params: &[Type]) -> String {
    let base = operator_word(name)
        .map(|w| format!("op.{}", w))
        .unwrap_or_else(|| name.to_string());
    let mut s = base;
    for p in params {
        s.push('$');
        s.push_str(&type_mangle(p));
    }
    s
}

/// A pronounceable word for an operator symbol, for use in a mangled LLVM name (which
/// can't contain the raw symbol). Returns `None` for non-operator (ordinary) names.
fn operator_word(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "%" => "mod",
        "==" => "eq",
        "!=" => "ne",
        "<" => "lt",
        "<=" => "le",
        ">" => "gt",
        ">=" => "ge",
        "&&" => "and",
        "||" => "or",
        _ => return None,
    })
}

/// A zero/`undef`-free constant of any basic LLVM type, used to fill a payload slot that
/// carries no information (a `$` Unit payload stored into a sized slot).
fn zeroed(ty: BasicTypeEnum<'_>) -> BasicValueEnum<'_> {
    match ty {
        BasicTypeEnum::IntType(t) => t.const_zero().into(),
        BasicTypeEnum::FloatType(t) => t.const_zero().into(),
        BasicTypeEnum::PointerType(t) => t.const_zero().into(),
        BasicTypeEnum::StructType(t) => t.const_zero().into(),
        BasicTypeEnum::ArrayType(t) => t.const_zero().into(),
        BasicTypeEnum::VectorType(t) => t.const_zero().into(),
        BasicTypeEnum::ScalableVectorType(t) => t.const_zero().into(),
    }
}

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
    /// If the captured value is itself a closure, its recorded signature
    /// (param types, return type) so the lifted body can re-register it and call it. A
    /// closure value is an opaque `{ ptr, ptr }` struct that does not encode its callee
    /// signature, and the lifted body starts with a cleared `closure_sigs`.
    closure_sig: Option<ClosureSig<'ctx>>,
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
    closure_sigs: HashMap<String, ClosureSig<'ctx>>,
    boxed_vars: std::collections::HashSet<String>,
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
    // Built from user `TypeDef::Sum` declarations plus the built-in Result. The tag is
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
    // from a narrower variant. Keyed by sum-type name. Only USER sum types are entered
    // here; the predefined `Result` is intentionally absent (its generic, heterogeneous
    // payloads are sized per-value at construction — see `register_builtin_sum_types`).
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
    // Signature of each local variable currently bound to a closure value:
    // (source-parameter LLVM types, return LLVM type). A closure value is an opaque
    // `{ ptr fn, ptr env }` struct that does not encode its callee signature, so calling
    // one needs the signature recovered here (recorded when the lambda is bound). The
    // trailing env-pointer parameter is implicit and not stored. Cleared per function.
    closure_sigs: HashMap<String, ClosureSig<'ctx>>,
    // The type oracle: authoritative inferred types for every expression, keyed by span,
    // produced by the type checker (see `TypeOracle`). Codegen consults it at READ sites
    // (array index, record-field access, match-arm result) to recover the *declared*
    // element/field/result LLVM type instead of guessing `f64` from a runtime value.
    // Populated at the start of `generate`; empty before then.
    oracle: TypeOracle,
    // Overload sets, keyed by name (function names AND operator symbols like `"+"`).
    // Each entry is the list of that name's overload parameter-type signatures. A name
    // is present here iff it is an overload set (operator-named, or 2+ same-named
    // top-level defs); calls/operators to these names dispatch to a NAME-MANGLED
    // function (`mangle_overload`) chosen by exact argument types. Operator builtins
    // (Num/Text `+`, comparisons) are NOT entered here — they keep their inline
    // lowering; only USER operator overloads add an operator symbol to this map.
    // Each member is its `(parameter types, return type)`.
    overloads: HashMap<String, Vec<(Vec<Type>, Type)>>,
    // Quilon type of each in-scope local/param, for argument-type inference at
    // overloaded call sites (codegen lacks the type checker's full inference, so it
    // tracks just enough — locals, params, and constructor results — to mangle).
    var_types: HashMap<String, Type>,
    // Declared return type of each NON-overloaded top-level function, so `infer_type`
    // can give a call's result its real type (not a `Num` default) when that result is
    // an argument to an overloaded call/operator — keeping codegen dispatch in sync
    // with the type checker. (Overloaded callees' returns come from `overloads`.)
    fn_return_types: HashMap<String, Type>,
    // Active self-tail-call optimization context for the function currently being
    // emitted, set up by `generate_function_decl` only when the body has a self-call in
    // tail position. A tail self-call then overwrites the param slots and branches back
    // to `loop_header` instead of emitting a stack-growing `call` + `ret` — guaranteeing
    // self-tail-recursion runs in constant stack (see `Tco` / `generate_tail_expr`).
    tco: Option<Tco<'ctx>>,
}

/// The loop-lowering context for self-tail-call optimization of one function. Present
/// (in `CodeGenerator::tco`) only while emitting a function whose body has at least one
/// self-call in tail position. Classic TCO transform: the body's parameter `=`-bindings
/// become mutable slots (`param_slots`), and a tail self-call stores its argument values
/// into those slots and `br`s back to `header` — turning the recursion into a loop.
struct Tco<'ctx> {
    /// The LLVM symbol of the function being optimized (mangled if overloaded). A `Call`
    /// is a self-tail-call only if it resolves to exactly this symbol with matching arity.
    self_symbol: String,
    /// The function's parameter alloca slots, in declaration order. A tail self-call
    /// recomputes the args and rewrites these slots (its length is the arity).
    param_slots: Vec<PointerValue<'ctx>>,
    /// The loop header — the block a tail self-call branches back to. Positioned right
    /// after the parameter slots are (re)loaded into the `variables` map for the body.
    header: inkwell::basic_block::BasicBlock<'ctx>,
}
// (merge note) `boxed_vars`/`lambda_counter`/`closure_sigs` (closures, M3) coexist with
// `oracle`/`overloads`/`var_types`/`fn_return_types` (overloads + Text-in-composite oracle).

/// Codegen-side view of the type checker's [`TypeTable`] — the "type oracle".
///
/// # Why this exists
/// Codegen used to recover LLVM types from runtime `BasicValueEnum::get_type()`, which
/// loses element/field types at every READ site and hardcodes `f64`. That corrupts any
/// non-`f64` payload nested in a composite — `Text` in a record/array, nested arrays,
/// `Ok(text)`/`NotOk(text)`. The fix is to thread the *declared* types (already computed
/// by the checker) through to the read sites.
///
/// # API (for downstream M3 waves: array methods, spread, args/env)
/// The single primitive is [`TypeOracle::expr_type`] — the inferred `Type` of any
/// expression, looked up by its source `Span`. The checker records the *result* type of
/// every node, so the element type of an `arr[i]` is `expr_type(<the Index node>)`, the
/// type of `rec.field` is `expr_type(<the FieldAccess node>)`, and a `match`'s result is
/// `expr_type(<the Match node>)` — there is no need for per-shape accessors, the read
/// site just asks for the type of the whole node it is lowering.
///
/// Lookups are by `Span` (one per AST node), so the oracle is AST-shape-agnostic and
/// additive: new expression kinds get types recorded automatically by `infer_expr`. A
/// `None` means the span wasn't recorded (e.g. the IR-only codegen tests that skip the
/// type-check pass); callers fall back to their historical `f64` assumption.
///
/// LIMITATION (tracked for a later M-wave): a `Span` is a byte range with no file/module
/// identity, and the `<<` import system lexes each module independently (offsets restart
/// at 0) before merging items into one `Program`. Two expressions in different modules can
/// therefore share a span and collide in the table (last-inferred wins). Today's imported
/// modules are numeric helpers/intrinsics with no composite reads, so this is latent, not
/// live; the robust fix is a stable per-node id (or a `(module, span)` key) assigned at
/// parse time. Until then, the oracle is only fully sound for single-file programs.
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

impl TypeOracle {
    fn new(table: crate::typechecker::TypeTable) -> Self {
        Self { table }
    }

    /// The inferred type of `expr`, by its span. `None` if the checker didn't record it.
    fn expr_type(&self, expr: &Expr) -> Option<&Type> {
        self.table.get(expr.span())
    }
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
            closure_sigs: HashMap::new(),
            oracle: TypeOracle::default(),
            overloads: HashMap::new(),
            var_types: HashMap::new(),
            fn_return_types: HashMap::new(),
            tco: None,
        };
        codegen.register_builtin_sum_types();
        codegen
    }

    /// Register the predefined `Result` variants: `Ok` is tag 0, `NotOk` is tag 1.
    /// Unlike user sum types, Result is NOT given a fixed payload layout: its variants
    /// have generic payloads (`Ok(T)` / `NotOk(E)`) whose concrete type is only known at
    /// each construction site, and the two variants routinely carry DIFFERENT payload
    /// types (e.g. `Ok(num)` vs `NotOk(text)`). So a Result value is sized to its
    /// actual payload at construction (`generate_sum_constructor`'s no-registered-layout
    /// path), preserving the historical per-value representation
    /// (`Ok(42) -> { i8, double }`, `NotOk("e") -> { i8, ptr }`).
    fn register_builtin_sum_types(&mut self) {
        self.sum_variants
            .insert("Ok".to_string(), (0u8, "Result".to_string()));
        self.sum_variants
            .insert("NotOk".to_string(), (1u8, "Result".to_string()));
        // Result's payloads are generic (`Ok(T)` / `NotOk(E)`); a `Generic` binding
        // resolves as Num for overload dispatch (see the type checker's `types_match`).
        let generic = |n: &str| Type::Generic {
            name: n.to_string(),
            args: vec![],
        };
        self.variant_payloads
            .insert("Ok".to_string(), vec![generic("T")]);
        self.variant_payloads
            .insert("NotOk".to_string(), vec![generic("E")]);
    }

    /// Access the underlying LLVM module after `generate` has populated it.
    /// Used by the JIT runner to create an execution engine in-process.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    /// Construct a generator with its **type oracle** already installed.
    ///
    /// Real compilation paths (`quilon run`/`compile`/`build`) reach codegen with a
    /// `program` that already passed the front-end type check (in `driver::front_end`),
    /// but that check's `TypeTable` isn't threaded down to here — so we re-derive it by
    /// type-checking once more and harvesting the table. The re-check is deliberate: it
    /// keeps every codegen entry point (CLI, JIT, tests) oracle-backed through one call
    /// without each caller having to carry the table, and `check_program` is a pure
    /// function of the AST, so the second run cannot disagree with the first. (If the
    /// double pass ever shows up in compile-time profiles, the fix is to have
    /// `front_end` return its table and feed it via [`set_type_table`].) A failure here
    /// would mean codegen was handed an unchecked program — surfaced as an internal error.
    pub fn with_oracle(
        context: &'ctx Context,
        module_name: &str,
        program: &Program,
    ) -> Result<Self, String> {
        let table = crate::typechecker::TypeChecker::new()
            .check_program(program)
            .map_err(|e| format!("internal: type check failed before codegen: {e}"))?;
        let mut codegen = Self::new(context, module_name);
        codegen.set_type_table(table);
        Ok(codegen)
    }

    /// Install the **type oracle** (the type checker's per-expression `TypeTable`) that
    /// codegen consults at read sites to recover precise element/field/match-result
    /// types. The companion to [`with_oracle`] for callers that already hold a table.
    /// Without it the oracle is empty, every lookup misses, and read sites fall back to
    /// their historical `f64` assumption — which is what the IR-only codegen tests (no
    /// typecheck pass) rely on.
    pub fn set_type_table(&mut self, table: crate::typechecker::TypeTable) {
        self.oracle = TypeOracle::new(table);
    }

    pub fn generate(&mut self, program: &Program) -> Result<String, String> {
        // Pre-pass: register all user sum-type variants so constructors and pattern
        // dispatch resolve regardless of declaration order relative to their uses.
        for item in &program.items {
            if let Item::TypeDecl(TypeDecl {
                name,
                type_def: TypeDef::Sum(variants),
                ..
            }) = item
            {
                self.register_sum_variants(name, variants)?;
            }
        }

        // Pre-pass: discover overload sets (operator-named, or 2+ same-named defs),
        // mirroring the type checker. Their definitions are name-mangled by parameter
        // type and dispatched by exact argument type at each call/operator site.
        let mut fn_counts: HashMap<&str, usize> = HashMap::new();
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
            {
                *fn_counts.entry(decl.name.as_str()).or_insert(0) += 1;
            }
        }
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
                && (is_operator_symbol(&decl.name)
                    || fn_counts.get(decl.name.as_str()).copied().unwrap_or(0) > 1
                    || is_builtin_overload_name(&decl.name))
                && decl.name != "^"
            {
                let params: Vec<Type> = decl
                    .params
                    .iter()
                    .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
                    .collect();
                // The return type drives argument-type inference for a value bound to
                // an overloaded call/operator (e.g. a user `+` returning a record).
                let ret = decl.return_type.clone().unwrap_or(Type::Num);
                self.overloads
                    .entry(decl.name.clone())
                    .or_default()
                    .push((params, ret));
            }
        }

        // Pre-pass: record each NON-overloaded top-level function's declared return
        // type, so `infer_type` can give a call result its real type when it feeds an
        // overloaded call/operator (keeps codegen dispatch in sync with the checker).
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
                && !self.overloads.contains_key(&decl.name)
                && let Some(ret) = &decl.return_type
            {
                self.fn_return_types.insert(decl.name.clone(), ret.clone());
            }
        }

        // Generate code for all top-level items. Reset the current-function context
        // before each one: a top-level item is never nested, so codegen must not see a
        // stale function left over from the previous top-level decl (which would make it
        // look like a nested/local declaration — see `generate_function_decl`).
        for item in &program.items {
            self.current_function = None;
            self.generate_item(item)?;
        }

        // Check if entry point function (^) exists and generate C main wrapper.
        // Pass `^`'s DECLARED Quilon parameter types so the wrapper can dispatch on the
        // real types (`[]Text` / `[][]Text` / legacy `Num`) — the lowered LLVM types are
        // ambiguous (`Text`, records, sum types, and arrays all become `{ ptr, i64 }`
        // structs), so dispatching on the LLVM shape would mis-route them.
        if self.module.get_function("^").is_some() {
            let entry_params: Vec<Type> = program
                .items
                .iter()
                .find_map(|item| match item {
                    Item::FunctionDecl(decl) if decl.name == "^" => Some(
                        decl.params
                            .iter()
                            .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            self.generate_main_wrapper(&entry_params)?;
        }

        // Verify the module
        if let Err(e) = self.module.verify() {
            return Err(format!("Module verification failed: {}", e));
        }

        // Return the LLVM IR as a string
        Ok(self.module.print_to_string().to_string())
    }

    fn generate_main_wrapper(&mut self, entry_params: &[Type]) -> Result<(), String> {
        // Create C-compatible main: `int main(int argc, char** argv, char** envp)`.
        // The third (`envp`) parameter is the POSIX/glibc extension to C `main`; passing
        // it is harmless even for a program that only declares `args`, and it is how we
        // thread the environment in for an `^(args, env)` entry point.
        let i32_type = self.context.i32_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());

        let main_type =
            i32_type.fn_type(&[i32_type.into(), ptr_type.into(), ptr_type.into()], false);

        let main_fn = self.module.add_function("main", main_type, None);
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
            .map_err(|e| format!("Failed to call GC init: {:?}", e))?;

        // Get the ^ (entry point) function
        let user_entry = self
            .module
            .get_function("^")
            .ok_or_else(|| "Entry point function ^ not found".to_string())?;

        // Dispatch on `^`'s DECLARED Quilon parameter types (not the lowered LLVM types:
        // `Text`/record/sum/array all lower to `{ ptr, i64 }` structs, so the LLVM shape
        // can't tell them apart — dispatching on it would silently call a `Text` param
        // with the argv array). The supported signatures are `^()`,
        // `^(args :: []Text)`, and `^(args :: []Text, env :: [][]Text)` (plus the legacy
        // `^(argc :: Num, argv :: Num)`). We match on the EXACT element types — the
        // runtime builds `Text`/`[]Text` elements, so a `[]Num` (or any other element)
        // param must NOT reach the array arms, or it would receive mis-sized elements.
        let is_text_array = |t: &Type| matches!(t, Type::Array(e) if **e == Type::Text);
        let is_text_pairs = |t: &Type| matches!(t, Type::Array(e) if is_text_array(e.as_ref()));

        // `argc` arrives as the C `int` (i32); widen it to the i64 the runtime expects.
        let argc_i64 = self
            .builder
            .build_int_s_extend(argc, self.context.i64_type(), "argc_i64")
            .map_err(|e| format!("Failed to widen argc: {:?}", e))?;

        // Build the real `args :: []Text` from argc/argv (used by the modern forms).
        let build_args = |me: &Self| -> Result<BasicValueEnum<'ctx>, String> {
            let f = me.get_intrinsic("__argv_to_text_array")?;
            use inkwell::values::AnyValue;
            Ok(me
                .builder
                .build_call(f, &[argc_i64.into(), argv.into()], "args_arr")
                .map_err(|e| format!("Failed to build argv array: {:?}", e))?
                .as_any_value_enum()
                .into_struct_value()
                .into())
        };
        // Build the real `env :: [][]Text` from envp (used by the 2-arg modern form).
        let build_env = |me: &Self| -> Result<BasicValueEnum<'ctx>, String> {
            let f = me.get_intrinsic("__envp_to_pairs")?;
            use inkwell::values::AnyValue;
            Ok(me
                .builder
                .build_call(f, &[envp.into()], "env_pairs")
                .map_err(|e| format!("Failed to build envp pairs: {:?}", e))?
                .as_any_value_enum()
                .into_struct_value()
                .into())
        };

        let unsupported = || -> String {
            format!(
                "Entry point ^ has an unsupported signature ({}). \
                 Valid signatures: '() -> Num', '(args :: []Text) -> Num', \
                 '(args :: []Text, env :: [][]Text) -> Num' \
                 (or legacy '(argc :: Num, argv :: Num) -> Num').",
                fmt_param_types(entry_params)
            )
        };

        let result = match entry_params {
            // `^() -> Num`
            [] => self
                .builder
                .build_call(user_entry, &[], "entry_result")
                .map_err(|e| format!("Failed to call entry point: {:?}", e))?,
            // `^(args :: []Text) -> Num`
            [a] if is_text_array(a) => {
                let args = build_args(self)?;
                self.builder
                    .build_call(user_entry, &[args.into()], "entry_result")
                    .map_err(|e| format!("Failed to call entry point: {:?}", e))?
            }
            // `^(args :: []Text, env :: [][]Text) -> Num`
            [a, e] if is_text_array(a) && is_text_pairs(e) => {
                let args = build_args(self)?;
                let env = build_env(self)?;
                self.builder
                    .build_call(user_entry, &[args.into(), env.into()], "entry_result")
                    .map_err(|e| format!("Failed to call entry point: {:?}", e))?
            }
            // Legacy `^(argc :: Num, argv :: Num) -> Num`: argc as a Num, argv a `0`
            // placeholder. Deprecated in favour of `^(args :: []Text)`.
            [Type::Num, Type::Num] => {
                let argc_as_f64 = self
                    .builder
                    .build_signed_int_to_float(argc, self.context.f64_type(), "argc_f64")
                    .map_err(|e| format!("Failed to convert argc: {:?}", e))?;
                let argv_placeholder = self.context.f64_type().const_zero();
                self.builder
                    .build_call(
                        user_entry,
                        &[argc_as_f64.into(), argv_placeholder.into()],
                        "entry_result",
                    )
                    .map_err(|e| format!("Failed to call entry point: {:?}", e))?
            }
            // Any other signature (e.g. `^(x :: Text)`, `^(args :: []Num)` with a
            // non-`Text` element, `^(a :: Num, b :: Text)`, `^(env :: [][]Text)` without
            // args, 3+ params) is rejected with a clear diagnostic instead of a silent
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
                    .map_err(|e| format!("Failed to convert result: {:?}", e))?
            }
            _ => {
                // Return 0 if not a numeric result
                i32_type.const_zero()
            }
        };

        self.builder
            .build_return(Some(&return_val))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn generate_item(&mut self, item: &Item) -> Result<(), String> {
        match item {
            Item::VarDecl(decl) => self.generate_var_decl(decl),
            Item::FunctionDecl(decl) => self.generate_function_decl(decl),
            Item::TypeDecl(decl) => self.generate_type_decl(decl),
        }
    }

    /// Register a sum type: map each variant to `(tag, type_name)` (tag = declaration
    /// index) and compute the type's canonical payload layout — one LLVM slot per
    /// payload position, sized to the widest variant. Per position, the slot type is the
    /// first NON-Unit field at that position (the type checker has validated that all
    /// concrete fields at a position agree). `$` (Unit) payload fields are zero-sized and
    /// contribute no slot, so a position that is `$` in every variant is dropped; a
    /// position mixing `$` with a concrete type uses the concrete type.
    fn register_sum_variants(
        &mut self,
        type_name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<(), String> {
        for (tag, variant) in variants.iter().enumerate() {
            self.sum_variants
                .insert(variant.name.clone(), (tag as u8, type_name.to_string()));
            // Record the variant's declared (concrete) payload types so a match arm's
            // payload binding gets its real type for overloaded-call mangling.
            self.variant_payloads
                .insert(variant.name.clone(), variant.fields.clone());
        }

        let max_arity = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        let mut layout: Vec<BasicTypeEnum<'ctx>> = Vec::with_capacity(max_arity);
        for pos in 0..max_arity {
            // Slot type = the first NON-Unit field at this position (all concrete fields
            // here agree, per the checker). If every variant has `$` (or nothing) here,
            // the slot is a zero `i8` Unit placeholder — keeps the struct shape uniform
            // (one field per position) while storing nothing meaningful.
            let concrete = variants
                .iter()
                .find_map(|v| v.fields.get(pos).filter(|f| **f != Type::Unit));
            let slot = match concrete {
                Some(ty) => self.type_to_llvm(ty)?,
                None => self.context.i8_type().into(),
            };
            layout.push(slot);
        }
        // A purely nullary enum still needs a payload slot so its `{ i8, .. }` value has
        // a uniform shape; use a single `double` placeholder (matches constructor codegen).
        if layout.is_empty() {
            layout.push(self.context.f64_type().into());
        }
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
            closure_sigs: std::mem::take(&mut self.closure_sigs),
            boxed_vars: std::mem::take(&mut self.boxed_vars),
        }
    }

    /// Reinstate a frame detached by `take_frame`.
    fn restore_frame(&mut self, frame: FrameState<'ctx>) {
        self.variables = frame.variables;
        self.record_types = frame.record_types;
        self.var_named_types = frame.var_named_types;
        self.var_types = frame.var_types;
        self.closure_sigs = frame.closure_sigs;
        self.boxed_vars = frame.boxed_vars;
    }

    fn generate_type_decl(&mut self, decl: &TypeDecl) -> Result<(), String> {
        if let TypeDef::Record { fields, methods } = &decl.type_def {
            let field_names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            self.named_type_fields
                .insert(decl.name.clone(), field_names.clone());

            let ptr_type = self.context.ptr_type(AddressSpace::default());

            // Pass 1: declare every method signature first, so a method body may reference
            // sibling methods (or recurse) regardless of declaration order.
            for method in methods {
                let mangled = format!("{}_{}", decl.name, method.name);
                if self.module.get_function(&mangled).is_some() {
                    continue;
                }
                let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum> =
                    vec![ptr_type.into()];
                for p in &method.params {
                    let pt = self.boundary_type(&p.type_annotation.clone().unwrap_or(Type::Num))?;
                    param_types.push(pt.into());
                }
                // Unannotated return type defaults to Num, except a setter body whose
                // tail is an in-place field write (`it.field := v`) yields `$` (i8).
                let inferred_ret =
                    self.default_return_type(method.return_type.as_ref(), &method.body);
                let return_type = self.boundary_type(&inferred_ret)?;
                let fn_type = return_type.fn_type(&param_types, false);
                let method_fn = self.module.add_function(&mangled, fn_type, None);
                // Internal linkage: method symbols are module-private (see generate_function_decl).
                method_fn.set_linkage(inkwell::module::Linkage::Internal);
            }

            // Pass 2: generate each method body.
            for method in methods {
                self.generate_method(&decl.name, &field_names, method)?;
            }
        }

        // Type declarations are not inside a function; clear any stray function context so a
        // following global declaration is not mistaken for a local.
        self.current_function = None;
        Ok(())
    }

    /// Emit the body of a single method as the pre-declared `"{TypeName}_{method}"` function,
    /// with `it` bound to the receiver pointer so `it.field` / sibling-method calls resolve.
    fn generate_method(
        &mut self,
        type_name: &str,
        field_names: &[String],
        method: &MethodDecl,
    ) -> Result<(), String> {
        let mangled = format!("{}_{}", type_name, method.name);
        let function = self
            .module
            .get_function(&mangled)
            .ok_or_else(|| format!("Method function not declared: {}", mangled))?;
        self.current_function = Some(function);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.take_frame(); // fresh frame: the previously emitted function's entries are dead
        self.boxed_vars = self.compute_boxed_vars(&method.body);

        // Param 0 is the implicit receiver `it` (a pointer to the record struct).
        let it_param = function.get_nth_param(0).unwrap();
        it_param.set_name("it");
        let it_type = it_param.as_basic_value_enum().get_type();
        let it_alloca = self.create_entry_block_alloca("it", it_type)?;
        self.builder
            .build_store(it_alloca, it_param)
            .map_err(|e| format!("Failed to store it: {:?}", e))?;
        self.variables
            .insert("it".to_string(), (it_alloca, it_type));
        // So `it.field` and `it.method()` resolve against this type.
        self.record_types
            .insert("it".to_string(), field_names.to_vec());
        self.var_named_types
            .insert("it".to_string(), type_name.to_string());

        // Remaining params follow the receiver.
        for (i, param) in method.params.iter().enumerate() {
            let llvm_param = function.get_nth_param((i + 1) as u32).unwrap();
            llvm_param.set_name(&param.name);
            let param_type = llvm_param.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&param.name, param_type)?;
            self.builder
                .build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            self.variables
                .insert(param.name.clone(), (alloca, param_type));
        }

        let body_value = self.generate_expr(&method.body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn generate_var_decl(&mut self, decl: &VarDecl) -> Result<(), String> {
        // Check if this is a record literal to track field names. Prefer the oracle's
        // inferred type (authoritative field names/order, and it expands `<-` spreads);
        // a functional-update whose result is a NAMED type also tracks that name so
        // method calls on the binding resolve. Fall back to the literal's own field names
        // when the oracle has no entry (IR-only tests) — which never carry spreads.
        if let Expr::Record { fields, .. } = &decl.value {
            // Field names in slot order: prefer the oracle's (it expands spreads and is
            // authoritative), else the literal's own names. Only a NAMED-type result also
            // records `var_named_types` so method calls on the binding resolve.
            let (field_names, named): (Vec<String>, Option<String>) =
                match self.oracle.expr_type(&decl.value) {
                    Some(Type::Named { name, fields, .. }) => (
                        fields.iter().map(|(n, _)| n.clone()).collect(),
                        Some(name.clone()),
                    ),
                    Some(Type::Record(fields)) => {
                        (fields.iter().map(|(n, _)| n.clone()).collect(), None)
                    }
                    _ => (fields.iter().map(|(n, _)| n.clone()).collect(), None),
                };
            self.record_types.insert(decl.name.clone(), field_names);
            if let Some(name) = named {
                self.var_named_types.insert(decl.name.clone(), name);
            }
        }
        // A named-type instance (e.g. `u = User { ... }`) — remember its type so method calls
        // on `u` can resolve to the mangled `User_method` functions.
        if let Expr::Constructor {
            type_name, fields, ..
        } = &decl.value
        {
            let field_names: Vec<String> = self
                .named_type_fields
                .get(type_name)
                .cloned()
                .unwrap_or_else(|| fields.iter().map(|(name, _)| name.clone()).collect());
            self.record_types.insert(decl.name.clone(), field_names);
            self.var_named_types
                .insert(decl.name.clone(), type_name.clone());
        }
        // Binding a function literal: remember its signature so a later `name(args)` can
        // recover the callee type for the indirect closure call (the closure value itself
        // does not encode it).
        if let Expr::Lambda {
            params,
            return_type,
            body,
            ..
        } = &decl.value
        {
            let sig = self.closure_signature(params, return_type.as_ref(), body)?;
            self.closure_sigs.insert(decl.name.clone(), sig);
        }

        // Remember the binding's Quilon type for overloaded-call argument mangling.
        let inferred_qty = self.infer_type(&decl.value);
        // If the value is a named record (e.g. bound to a user operator overload's
        // result), track its type/fields so later `name.field` / method calls resolve.
        if let Type::Named { name, .. } = &inferred_qty
            && let Some(fields) = self.named_type_fields.get(name).cloned()
        {
            self.record_types.insert(decl.name.clone(), fields);
            self.var_named_types.insert(decl.name.clone(), name.clone());
        }
        self.var_types.insert(decl.name.clone(), inferred_qty);

        let value = self.generate_expr(&decl.value)?;

        if self.current_function.is_some() {
            let var_type = value.get_type();

            // Reassignment of an already-bound mutable local (`counter := counter + 1`):
            // store THROUGH the existing slot rather than allocating a fresh one. This is
            // what makes a `:=` capture escape-safe — the cell a closure shares is the
            // very cell later writes target — and it is equivalent to the old realloc for
            // ordinary straight-line code (reads always go through the latest slot).
            if decl.mutable
                && let Some((slot, _)) = self.variables.get(&decl.name).copied()
            {
                self.builder
                    .build_store(slot, value)
                    .map_err(|e| format!("Failed to build store: {:?}", e))?;
                return Ok(());
            }

            // A `:=` local captured by reference by some nested closure lives in a heap
            // GC cell (a "box"), so the closure and this frame share one mutable cell. Its
            // `variables` slot is the cell pointer; loads/stores work through it unchanged.
            let slot = if decl.mutable && self.boxed_vars.contains(&decl.name) {
                self.alloc_box(var_type)?
            } else {
                self.create_entry_block_alloca(&decl.name, var_type)?
            };
            self.builder
                .build_store(slot, value)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            self.variables.insert(decl.name.clone(), (slot, var_type));
        } else {
            // Global variable
            let global =
                self.module
                    .add_global(value.get_type(), Some(AddressSpace::default()), &decl.name);
            global.set_initializer(&value);
        }

        Ok(())
    }

    fn generate_function_decl(&mut self, decl: &FunctionDecl) -> Result<(), String> {
        // The inert core.io print/eprint placeholder is never emitted (the compiler
        // lowers print/eprint to runtime intrinsics).
        if decl.is_inert_io_placeholder() {
            return Ok(());
        }

        // A function declared INSIDE another function (we are mid-emitting a body) is a
        // local declaration. If its body references enclosing locals it is a capturing
        // CLOSURE (lowered via the lambda machinery); otherwise it is a self-contained
        // local function, which we emit as a plain module function — that preserves
        // recursion (`fact = n => … fact(n-1) …`), since a closure value cannot refer to
        // itself before it exists. The choice is by ACTUAL captures, not syntax.
        if self.current_function.is_some() {
            // Emitting a nested function re-enters function emission, which sets and then
            // clears the outer function's TCO context (`self.tco`). Snapshot and restore
            // it so a nested tail-recursive function does not clobber the OUTER function's
            // active context — otherwise the outer tail walk resuming after this nested
            // decl would panic ("generate_tail_expr without a TCO context").
            let saved_tco = self.tco.take();
            let param_names: Vec<String> = decl.params.iter().map(|p| p.name.clone()).collect();
            let outer: std::collections::HashSet<String> = self.variables.keys().cloned().collect();
            let captures =
                crate::ast::captures::lambda_free_idents(&param_names, &decl.body, &outer);
            let result = if !captures.is_empty() {
                self.generate_local_closure(decl)
            } else {
                // No captures: emit a plain module function, but save/restore the
                // enclosing per-function frame and builder state around it, since
                // `emit_module_function` starts from an empty frame.
                let saved_block = self.builder.get_insert_block();
                let saved_function = self.current_function;
                let saved_frame = self.take_frame();

                let result = self.emit_module_function(decl);

                self.restore_frame(saved_frame);
                self.current_function = saved_function;
                if let Some(block) = saved_block {
                    self.builder.position_at_end(block);
                }
                result
            };
            self.tco = saved_tco;
            return result;
        }

        self.emit_module_function(decl)
    }

    /// Emit `decl` as a top-level/module function (internal linkage). Clears and
    /// repopulates the per-function emission state (`variables`, `closure_sigs`,
    /// `boxed_vars`, `var_types`); the entry point `^` gets the special f64-return /
    /// implicit-0 treatment. Used for true top-level functions and for non-capturing
    /// nested functions (which can recurse, unlike a closure value).
    fn emit_module_function(&mut self, decl: &FunctionDecl) -> Result<(), String> {
        // Convert parameter types to LLVM types via the shared boundary rule: an ARRAY
        // param crosses as the `{ ptr, i64 }` VALUE struct (so `.size`/indexing work),
        // everything else via `type_to_llvm` (a record/sum param stays by pointer/struct).
        let param_types: Vec<BasicTypeEnum> = decl
            .params
            .iter()
            .map(|p| self.boundary_type(&p.type_annotation.clone().unwrap_or(Type::Num)))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert return type. The entry point `^` always returns a Num exit code at
        // the LLVM level (the C `main` wrapper expects an f64), regardless of its body
        // type — so a side-effecting main can omit the trailing `0`.
        let return_type = if decl.name == "^" {
            self.context.f64_type().into()
        } else {
            // An unannotated body defaults to `Num`, except a Unit (`$`) tail — e.g.
            // `log = m => print(m)` — which must be `i8`, not f64, or `build_return`
            // would emit `ret i8` into an f64 function and fail module verification.
            // The same boundary rule applies: an array return crosses as the value struct.
            let inferred = self.default_return_type(decl.return_type.as_ref(), &decl.body);
            self.boundary_type(&inferred)?
        };

        // Create function type - use a helper to convert BasicTypeEnum to BasicMetadataTypeEnum
        let fn_type = return_type.fn_type(
            &param_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        // Create the function. Use internal linkage so a Quilon function name never
        // collides with a C library / runtime symbol when the whole program is linked
        // into one native binary (AOT). For example core.io's `write` placeholder, or
        // a user function named `read`/`open`, would otherwise shadow libc and break
        // the runtime intrinsics. Only the generated `main` wrapper is exported.
        //
        // An overloaded member (operator-named, or one of several same-named defs) is
        // emitted under a per-signature MANGLED name so the members don't collide; each
        // call site dispatches to the matching mangled symbol by exact argument type.
        let symbol = if self.overloads.contains_key(&decl.name) {
            let params: Vec<Type> = decl
                .params
                .iter()
                .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
                .collect();
            mangle_overload(&decl.name, &params)
        } else {
            decl.name.clone()
        };
        let function = self.module.add_function(&symbol, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);
        self.current_function = Some(function);

        // Create entry block
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Store parameters in variables map
        self.take_frame(); // fresh frame: the previously emitted function's entries are dead
        // Which `:=` locals must be heap-boxed because a nested closure captures them.
        self.boxed_vars = self.compute_boxed_vars(&decl.body);
        for (i, param) in decl.params.iter().enumerate() {
            let llvm_param = function.get_nth_param(i as u32).unwrap();
            llvm_param.set_name(&param.name);

            // Allocate space for the parameter
            let param_type = llvm_param.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&param.name, param_type)?;
            self.builder
                .build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;

            self.variables
                .insert(param.name.clone(), (alloca, param_type));
            // Track the parameter's Quilon type for overloaded-call mangling, and so a
            // record/sum parameter's methods/fields resolve.
            let qty = param.type_annotation.clone().unwrap_or(Type::Num);
            if let Type::Named { name, .. } | Type::Sum { name, .. } = &qty {
                self.var_named_types
                    .insert(param.name.clone(), name.clone());
                if let Some(fields) = self.named_type_fields.get(name) {
                    self.record_types.insert(param.name.clone(), fields.clone());
                }
            }
            self.var_types.insert(param.name.clone(), qty);
        }

        // Guaranteed self-tail-call optimization: if the body returns a call to THIS
        // function in tail position, lower the recursion to a loop instead of a
        // stack-growing `call` + `ret`. Set up a loop header (branched to from the entry
        // block, after the param slots are populated) and a TCO context; a tail self-call
        // then rewrites the param slots and `br`s back here. The param allocas created
        // above are reused as the loop's mutable slots — there is no separate IR shape for
        // recursive vs. non-recursive functions beyond this header + the back-edge.
        let body_value = if self.body_has_self_tail_call(decl, &symbol) {
            let param_slots: Vec<PointerValue<'ctx>> = decl
                .params
                .iter()
                .map(|p| self.variables[&p.name].0)
                .collect();
            let header = self.context.append_basic_block(function, "tco_loop");
            self.builder
                .build_unconditional_branch(header)
                .map_err(|e| format!("Failed to build branch to loop header: {:?}", e))?;
            self.builder.position_at_end(header);
            self.tco = Some(Tco {
                self_symbol: symbol.clone(),
                param_slots,
                header,
            });
            // Emit the body in tail-aware mode. A `None` result means every tail exit was a
            // self-call (e.g. an unconditional `f(...)` body, or a match all of whose arms
            // tail-recurse): the function never falls through to a normal return, and
            // `generate_tail_expr` has already terminated the current block (with the
            // back-edge `br`, or an `unreachable`). In that case there is no `ret` to emit.
            let result = self.generate_tail_expr(&decl.body)?;
            self.tco = None;
            match result {
                Some(v) => v,
                None => return Ok(()),
            }
        } else {
            self.generate_expr(&decl.body)?
        };

        // Entry point `^`: if the body's value isn't a Num (f64) — e.g. a side-effecting
        // main ending in a Text/Bool/record expression — discard it and implicitly
        // return 0 (C `main`-style success). A Num body is used as the exit code as
        // usual. Scoped to `^`; ordinary functions return their body's actual type.
        let return_value: inkwell::values::BasicValueEnum =
            if decl.name == "^" && !body_value.is_float_value() {
                self.context.f64_type().const_float(0.0).into()
            } else {
                body_value
            };
        self.builder
            .build_return(Some(&return_value))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    // ---- Self-tail-call optimization (loop lowering) --------------------------------
    //
    // A call is in **tail position** when it is the value the enclosing function returns
    // directly — i.e. nothing happens to its result before the `ret`. Tail position flows
    // through exactly the constructs that yield a value as their tail without further
    // computation: a block's last expression, both arms of an `if`/ternary, every arm of a
    // `?`/`|` match, a parenthesizing pipeline's desugaring, and the function body itself.
    // It does NOT flow into the operand of a `+`/`*`/comparison, a call argument, an array
    // element, etc. — those consume the value, so a call there is not in tail position.
    //
    // `body_has_self_tail_call` is the pure analysis (no IR), used once to decide whether
    // to set up the loop. `generate_tail_expr` is the codegen counterpart: it walks the
    // SAME tail-position structure and, at a tail self-call, rewrites the param slots and
    // branches to the loop header; everything else (and every non-tail subexpression) goes
    // through the ordinary `generate_expr`. The two must agree on what "tail position" is.

    /// Does `decl`'s body contain a self-call in tail position? Pure (emits no IR).
    /// `self_symbol` is the LLVM symbol the function is emitted under (mangled if
    /// overloaded) — passed in from `emit_module_function` so the "which symbol?" rule
    /// lives in one place, and a tail call is recognized as a SELF-call by matching it.
    fn body_has_self_tail_call(&self, decl: &FunctionDecl, self_symbol: &str) -> bool {
        self.expr_has_self_tail_call(&decl.body, self_symbol, decl.params.len())
    }

    /// Whether `expr`, evaluated in tail position, contains a self-call (to `self_symbol`
    /// with `arity` args). Recurses only through tail-position sub-expressions.
    fn expr_has_self_tail_call(&self, expr: &Expr, self_symbol: &str, arity: usize) -> bool {
        match expr {
            Expr::Call { .. } => self.is_self_tail_call(expr, self_symbol, arity),
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => {
                    self.expr_has_self_tail_call(tail, self_symbol, arity)
                }
                _ => false,
            },
            Expr::If { then, else_, .. } => {
                self.expr_has_self_tail_call(then, self_symbol, arity)
                    || self.expr_has_self_tail_call(else_, self_symbol, arity)
            }
            Expr::Match { arms, .. } => arms
                .iter()
                .any(|arm| self.expr_has_self_tail_call(&arm.body, self_symbol, arity)),
            // A pipeline desugars to a call; check the call it becomes.
            Expr::Pipeline { left, right, span } => {
                let call = Expr::desugar_pipeline(left, right, span);
                self.is_self_tail_call(&call, self_symbol, arity)
            }
            _ => false,
        }
    }

    /// Whether `expr` is a direct call that resolves to `self_symbol` with `arity` args —
    /// i.e. the function calling itself. Resolution mirrors `generate_call`'s: a plain
    /// name maps to itself, an overloaded name to its exact mangled member by argument
    /// types. A constructor/method/intrinsic call (which `generate_call` routes elsewhere)
    /// is never a self-call. NB only the *callee identity* matters here; the arguments are
    /// generated normally by `generate_tail_expr`.
    fn is_self_tail_call(&self, expr: &Expr, self_symbol: &str, arity: usize) -> bool {
        let Expr::Call { func, args, .. } = expr else {
            return false;
        };
        let Expr::Ident { name, .. } = func.as_ref() else {
            return false;
        };
        if args.len() != arity {
            return false;
        }
        // A name shadowed by a sum-type constructor or an intrinsic is not a self-call.
        // The intrinsic names here MUST stay in sync with those intercepted in
        // `generate_call` (`print`/`eprint`/`write`/`__exit`).
        if self.sum_variants.contains_key(name.as_str())
            || matches!(name.as_str(), "print" | "eprint" | "write" | "__exit")
        {
            return false;
        }
        let symbol = if self.overloads.contains_key(name.as_str()) {
            let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
            match self.resolve_overload_symbol(name, &arg_types) {
                Some(s) => s,
                None => return false,
            }
        } else {
            name.clone()
        };
        symbol == self_symbol
    }

    /// Emit `expr` in tail position under an active [`Tco`] context. Returns `Some(value)`
    /// for an ordinary tail (the caller `ret`s it) or `None` when this path does not fall
    /// through to a normal return — every tail exit was a self-call. **Invariant:** on
    /// `None`, the current insert block is already TERMINATED (by the back-edge `br` of a
    /// tail self-call, or an `unreachable` for an if/match all of whose arms recurse), so
    /// the caller must not emit anything more into it. Walks the same tail-position
    /// structure as `expr_has_self_tail_call`; any non-tail node falls through to
    /// `generate_expr` (always `Some`).
    fn generate_tail_expr(&mut self, expr: &Expr) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let arity = self
            .tco
            .as_ref()
            .expect("generate_tail_expr without a TCO context")
            .param_slots
            .len();

        match expr {
            // A pipeline in tail position is its desugared call; lower that.
            Expr::Pipeline { left, right, span } => {
                let call = Expr::desugar_pipeline(left, right, span);
                self.generate_tail_expr(&call)
            }

            // A call in tail position: if it resolves to THIS function, lower it to the
            // loop back-edge; otherwise it is an ordinary value. Clone `self_symbol` only
            // here (a call leaf), not on every tail node.
            Expr::Call { args, .. } => {
                let self_symbol = self.tco.as_ref().unwrap().self_symbol.clone();
                if self.is_self_tail_call(expr, &self_symbol, arity) {
                    self.emit_tail_self_call(args)?;
                    Ok(None)
                } else {
                    Ok(Some(self.generate_expr(expr)?))
                }
            }

            Expr::Block { stmts, .. } => {
                // Emit every statement normally except the tail expression, which stays in
                // tail position. A non-`Expr`-tail block (ends in an item) has no tail call
                // (the analysis returned false), so generating it whole is correct.
                match stmts.split_last() {
                    Some((crate::ast::Statement::Expr(tail), init)) => {
                        for stmt in init {
                            match stmt {
                                crate::ast::Statement::Item(item) => self.generate_item(item)?,
                                crate::ast::Statement::Expr(e) => {
                                    self.generate_expr(e)?;
                                }
                            }
                        }
                        self.generate_tail_expr(tail)
                    }
                    _ => Ok(Some(self.generate_block(stmts)?)),
                }
            }

            Expr::If {
                cond, then, else_, ..
            } => self.generate_tail_if(cond, then, else_),

            Expr::Match {
                expr: scrutinee,
                arms,
                ..
            } => self.generate_tail_match(expr, scrutinee, arms),

            // Anything else in tail position is an ordinary value.
            other => Ok(Some(self.generate_expr(other)?)),
        }
    }

    /// Lower a tail self-call: evaluate the argument expressions, write them into the
    /// parameter slots, then `br` back to the loop header. All args are evaluated into
    /// temporaries BEFORE any slot is overwritten, so an argument that reads a parameter
    /// (e.g. `f(n - 1, acc + n)` reading `n` for `acc`) sees the current iteration's
    /// values, not a half-updated set.
    fn emit_tail_self_call(&mut self, args: &[Expr]) -> Result<(), String> {
        let new_vals: Vec<BasicValueEnum<'ctx>> = args
            .iter()
            .map(|a| self.generate_expr(a))
            .collect::<Result<Vec<_>, _>>()?;
        // Snapshot slots + header before the mutable stores (releases the `self.tco`
        // borrow so the `&mut self` builder calls below are allowed).
        let tco = self
            .tco
            .as_ref()
            .expect("emit_tail_self_call without a TCO context");
        let slots: Vec<PointerValue<'ctx>> = tco.param_slots.clone();
        let header = tco.header;
        for (slot, val) in slots.iter().zip(new_vals) {
            self.builder
                .build_store(*slot, val)
                .map_err(|e| format!("Failed to store tail-call arg: {:?}", e))?;
        }
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to branch to loop header: {:?}", e))?;
        Ok(())
    }

    /// Tail-position `if`/ternary: emit each arm in tail position. An arm that tail-recurses
    /// branches to the loop header (yields no value); an arm that produces a value branches
    /// to a merge block. We `phi` only over the value-producing arms — if both arms tail
    /// self-call, there is no merge value and we return `None`.
    fn generate_tail_if(
        &mut self,
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let cond_val = self.generate_expr(cond)?;
        let BasicValueEnum::IntValue(cond_bool) = cond_val else {
            return Err("Condition must be a boolean".to_string());
        };
        let function = self
            .current_function
            .ok_or_else(|| "If expression outside of function".to_string())?;

        let then_bb = self.context.append_basic_block(function, "then");
        let else_bb = self.context.append_basic_block(function, "else");
        let merge_bb = self.context.append_basic_block(function, "ifcont");

        self.builder
            .build_conditional_branch(cond_bool, then_bb, else_bb)
            .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

        // Collect each non-tail-recursing arm's (value, originating block) for the phi.
        let mut incoming: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::new();

        self.builder.position_at_end(then_bb);
        if let Some(v) = self.generate_tail_expr(then_expr)? {
            let bb = self.builder.get_insert_block().unwrap();
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
            incoming.push((v, bb));
        }

        self.builder.position_at_end(else_bb);
        if let Some(v) = self.generate_tail_expr(else_expr)? {
            let bb = self.builder.get_insert_block().unwrap();
            self.builder
                .build_unconditional_branch(merge_bb)
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
            incoming.push((v, bb));
        }

        self.builder.position_at_end(merge_bb);
        match incoming.as_slice() {
            // Both arms tail-recursed: control never reaches the merge block. Terminate it
            // as `unreachable` (it has no value-producing predecessors) and report `None`
            // — every `None` from a tail node leaves the current block already terminated.
            [] => {
                self.builder
                    .build_unreachable()
                    .map_err(|e| format!("Failed to build unreachable: {:?}", e))?;
                Ok(None)
            }
            _ => {
                let phi = self
                    .builder
                    .build_phi(incoming[0].0.get_type(), "iftmp")
                    .map_err(|e| format!("Failed to build phi: {:?}", e))?;
                for (v, bb) in &incoming {
                    phi.add_incoming(&[(v as &dyn BasicValue, *bb)]);
                }
                Ok(Some(phi.as_basic_value()))
            }
        }
    }

    /// Tail-position `?`/`|` match: same shape as `generate_match`, but each arm body is
    /// emitted in tail position. An arm that tail-recurses branches to the loop header and
    /// stores nothing; an arm that yields a value stores it into the shared result slot and
    /// falls through to the continuation. If EVERY arm tail-recurses, the continuation is
    /// unreachable and we return `None` (no result to load).
    fn generate_tail_match(
        &mut self,
        match_expr: &Expr,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let match_val = self.generate_expr(scrutinee)?;
        let function = self
            .current_function
            .ok_or_else(|| "Match expression must be in a function".to_string())?;

        let mut arm_blocks = vec![];
        let mut check_blocks = vec![];
        for i in 0..arms.len() {
            check_blocks.push(
                self.context
                    .append_basic_block(function, &format!("check_{}", i)),
            );
            arm_blocks.push(
                self.context
                    .append_basic_block(function, &format!("arm_{}", i)),
            );
        }
        let cont_block = self.context.append_basic_block(function, "match_cont");

        // Result slot for the value-producing (non-tail-recursing) arms, sized from the
        // oracle exactly as `generate_match` does. Only written by arms that yield a value.
        let result_llvm = self.oracle_value_type(match_expr)?;
        let result_alloca = self.create_entry_block_alloca("match_result", result_llvm)?;

        self.builder
            .build_unconditional_branch(check_blocks[0])
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        let mut any_value_arm = false;
        for (i, arm) in arms.iter().enumerate() {
            self.builder.position_at_end(check_blocks[i]);
            let matches = self.check_pattern(&arm.pattern, match_val)?;
            let next_block = if i + 1 < check_blocks.len() {
                check_blocks[i + 1]
            } else {
                cont_block
            };
            self.builder
                .build_conditional_branch(matches, arm_blocks[i], next_block)
                .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

            self.builder.position_at_end(arm_blocks[i]);
            self.bind_pattern(&arm.pattern, match_val, scrutinee)?;
            if let Some(arm_val) = self.generate_tail_expr(&arm.body)? {
                any_value_arm = true;
                self.builder
                    .build_store(result_alloca, arm_val)
                    .map_err(|e| format!("Failed to store result: {:?}", e))?;
                self.builder
                    .build_unconditional_branch(cont_block)
                    .map_err(|e| format!("Failed to build branch: {:?}", e))?;
            }
            // Else: the arm tail-recursed and already branched to the loop header.
        }

        self.builder.position_at_end(cont_block);
        if any_value_arm {
            Ok(Some(
                self.builder
                    .build_load(result_llvm, result_alloca, "match_result")
                    .map_err(|e| format!("Failed to load result: {:?}", e))?,
            ))
        } else {
            // Every arm tail-recursed: control never produces a value here (the only edge
            // into `cont_block` is the last check's no-match fallthrough, which an
            // exhaustive match never takes). Terminate it as `unreachable` and report
            // `None` — keeping the "a `None` leaves the block terminated" invariant.
            self.builder
                .build_unreachable()
                .map_err(|e| format!("Failed to build unreachable: {:?}", e))?;
            Ok(None)
        }
    }

    /// Bind a capturing nested function as a local closure value: lower it via the lambda
    /// machinery (capturing enclosing locals per the `=`/`:=` rule) and store the
    /// resulting `{ ptr fn, ptr env }` in a local slot, recording its signature so
    /// `name(args)` resolves to an indirect closure call.
    fn generate_local_closure(&mut self, decl: &FunctionDecl) -> Result<(), String> {
        let sig = self.closure_signature(&decl.params, decl.return_type.as_ref(), &decl.body)?;
        self.closure_sigs.insert(decl.name.clone(), sig);

        let closure = self.generate_lambda(&decl.params, decl.return_type.as_ref(), &decl.body)?;
        let slot = self.create_entry_block_alloca(&decl.name, closure.get_type())?;
        self.builder
            .build_store(slot, closure)
            .map_err(|e| format!("Failed to store closure: {:?}", e))?;
        self.variables
            .insert(decl.name.clone(), (slot, closure.get_type()));
        Ok(())
    }

    fn create_entry_block_alloca(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let builder = self.context.create_builder();

        let entry = self
            .current_function
            .unwrap()
            .get_first_basic_block()
            .unwrap();
        match entry.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry),
        }

        builder
            .build_alloca(ty, name)
            .map_err(|e| format!("Failed to build alloca: {:?}", e))
    }

    // ---- Closures (M3) -----------------------------------------------------------------
    //
    // A closure value is a flat `{ ptr fn, ptr env }` struct: a pointer to the lifted
    // top-level function, and a pointer to its heap-allocated environment of captured
    // values. The lifted function takes the captured environment as an extra TRAILING
    // pointer parameter (after the source parameters), so calling through a closure is a
    // plain indirect call passing `env` last. Closures are monomorphic (M3): captured
    // values and parameters are concrete-typed; generic closures are M4.

    /// The uniform closure representation: `{ ptr fn, ptr env }`.
    fn closure_struct_type(&self) -> inkwell::types::StructType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.context.struct_type(&[ptr.into(), ptr.into()], false)
    }

    /// Allocate a GC-managed heap cell large enough to hold one `ty` value and return the
    /// pointer to it. Used to "box" a `:=` local captured by reference, so the cell
    /// outlives the defining frame and is shared with the closure.
    fn alloc_box(&self, ty: BasicTypeEnum<'ctx>) -> Result<PointerValue<'ctx>, String> {
        use inkwell::values::AnyValue;
        let size = ty
            .size_of()
            .ok_or_else(|| format!("cannot size box for type {:?}", ty))?;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        Ok(self
            .builder
            .build_call(alloc_fn, &[size.into()], "box")
            .map_err(|e| format!("Failed to call __alloc for box: {:?}", e))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// The `:=` (mutable) locals of the function body `body` that some nested closure
    /// captures by reference, and so must be heap-boxed. A captured `=` local is copied
    /// by value into the closure's environment and needs no box; only a captured mutable
    /// local must share a single cell with the closure. Computed by collecting the
    /// function's `:=` binding names and intersecting with the union of every nested
    /// lambda's free variables.
    fn compute_boxed_vars(&self, body: &Expr) -> std::collections::HashSet<String> {
        let mut mutable_locals = std::collections::HashSet::new();
        Self::collect_mutable_locals(body, &mut mutable_locals);

        // Find which of those mutable locals a nested closure captures. Passing the
        // mutable-local set as the lambdas' `outer` scope means a closure's captures are
        // already restricted to (and recognize reassignments of) exactly these names.
        let mut captured = std::collections::HashSet::new();
        Self::collect_lambda_captures(body, &mutable_locals, &mut captured);
        captured
    }

    /// Collect the names of all `:=` (mutable) `VarDecl`s bound in THIS function frame —
    /// i.e. in `expr` and its nested control-flow, but NOT inside a nested lambda body (a
    /// lambda's own `:=` locals live in the lambda's frame, not ours).
    fn collect_mutable_locals(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        match expr {
            // A nested function literal opens its own frame — do not descend.
            Expr::Lambda { .. } => {}
            Expr::Block { stmts, .. } => {
                for stmt in stmts {
                    match stmt {
                        crate::ast::Statement::Expr(e) => Self::collect_mutable_locals(e, out),
                        crate::ast::Statement::Item(Item::VarDecl(decl)) => {
                            if decl.mutable {
                                out.insert(decl.name.clone());
                            }
                            Self::collect_mutable_locals(&decl.value, out);
                        }
                        crate::ast::Statement::Item(_) => {}
                    }
                }
            }
            Expr::BinOp { left, right, .. }
            | Expr::Pipeline { left, right, .. }
            | Expr::Range {
                start: left,
                end: right,
                ..
            } => {
                Self::collect_mutable_locals(left, out);
                Self::collect_mutable_locals(right, out);
            }
            Expr::UnaryOp { expr, .. } | Expr::FieldAccess { expr, .. } => {
                Self::collect_mutable_locals(expr, out)
            }
            Expr::Call { func, args, .. } => {
                Self::collect_mutable_locals(func, out);
                for a in args {
                    Self::collect_mutable_locals(a, out);
                }
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                Self::collect_mutable_locals(cond, out);
                Self::collect_mutable_locals(then, out);
                Self::collect_mutable_locals(else_, out);
            }
            Expr::Match { expr, arms, .. } => {
                Self::collect_mutable_locals(expr, out);
                for arm in arms {
                    Self::collect_mutable_locals(&arm.body, out);
                }
            }
            Expr::FieldAssign { target, value, .. } => {
                Self::collect_mutable_locals(target, out);
                Self::collect_mutable_locals(value, out);
            }
            Expr::Index { expr, index, .. } => {
                Self::collect_mutable_locals(expr, out);
                Self::collect_mutable_locals(index, out);
            }
            Expr::Array { elements, .. } => {
                for e in elements {
                    Self::collect_mutable_locals(e, out);
                }
            }
            Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
                for (_, e) in fields {
                    Self::collect_mutable_locals(e, out);
                }
            }
            Expr::SumConstructor { args, .. } => {
                for a in args {
                    Self::collect_mutable_locals(a, out);
                }
            }
            Expr::Spread { expr, .. } => Self::collect_mutable_locals(expr, out),
            Expr::Number { .. }
            | Expr::String { .. }
            | Expr::Bool { .. }
            | Expr::Unit { .. }
            | Expr::Ident { .. } => {}
        }
    }

    /// Union, over every lambda appearing (at any depth) in `expr`, of the names it
    /// captures from `outer`. Used to find which of the enclosing frame's mutable locals
    /// a closure shares (and so must be heap-boxed).
    fn collect_lambda_captures(
        expr: &Expr,
        outer: &std::collections::HashSet<String>,
        out: &mut std::collections::HashSet<String>,
    ) {
        Self::for_each_closure(expr, &mut |params, body| {
            let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            for name in crate::ast::captures::lambda_free_idents(&names, body, outer) {
                out.insert(name);
            }
        });
    }

    /// Invoke `f(params, body)` for every closure appearing (at any depth) in `expr` — a
    /// `Expr::Lambda` OR a nested `Item::FunctionDecl` (both are closures; the latter is
    /// only resolved to a plain function at codegen when it captures nothing). Used to
    /// gather captures across all closures in a frame.
    fn for_each_closure(expr: &Expr, f: &mut impl FnMut(&[crate::ast::Param], &Expr)) {
        Self::walk_exprs(expr, &mut |e| match e {
            Expr::Lambda { params, body, .. } => f(params, body),
            Expr::Block { stmts, .. } => {
                for stmt in stmts {
                    if let crate::ast::Statement::Item(Item::FunctionDecl(decl)) = stmt {
                        f(&decl.params, &decl.body);
                        // The function body is an expression position `walk_exprs` does
                        // not enter (it only descends VarDecl initializers), so recurse to
                        // find closures nested inside this nested function too.
                        Self::for_each_closure(&decl.body, f);
                    }
                }
            }
            _ => {}
        });
    }

    /// Pre-order walk over every sub-expression of `expr`, invoking `f` on each. Used by
    /// the closure pre-passes above. Does not descend into nested item declarations'
    /// signatures (only expression positions), which is all closure analysis needs.
    fn walk_exprs(expr: &Expr, f: &mut impl FnMut(&Expr)) {
        f(expr);
        match expr {
            Expr::BinOp { left, right, .. }
            | Expr::Pipeline { left, right, .. }
            | Expr::Range {
                start: left,
                end: right,
                ..
            } => {
                Self::walk_exprs(left, f);
                Self::walk_exprs(right, f);
            }
            Expr::UnaryOp { expr, .. } | Expr::FieldAccess { expr, .. } => {
                Self::walk_exprs(expr, f)
            }
            Expr::Call { func, args, .. } => {
                Self::walk_exprs(func, f);
                for a in args {
                    Self::walk_exprs(a, f);
                }
            }
            Expr::Lambda { body, .. } => Self::walk_exprs(body, f),
            Expr::Block { stmts, .. } => {
                for stmt in stmts {
                    match stmt {
                        crate::ast::Statement::Expr(e) => Self::walk_exprs(e, f),
                        crate::ast::Statement::Item(Item::VarDecl(d)) => {
                            Self::walk_exprs(&d.value, f)
                        }
                        crate::ast::Statement::Item(_) => {}
                    }
                }
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                Self::walk_exprs(cond, f);
                Self::walk_exprs(then, f);
                Self::walk_exprs(else_, f);
            }
            Expr::Match { expr, arms, .. } => {
                Self::walk_exprs(expr, f);
                for arm in arms {
                    Self::walk_exprs(&arm.body, f);
                }
            }
            Expr::FieldAssign { target, value, .. } => {
                Self::walk_exprs(target, f);
                Self::walk_exprs(value, f);
            }
            Expr::Index { expr, index, .. } => {
                Self::walk_exprs(expr, f);
                Self::walk_exprs(index, f);
            }
            Expr::Array { elements, .. } => {
                for e in elements {
                    Self::walk_exprs(e, f);
                }
            }
            Expr::Record { fields, .. } | Expr::Constructor { fields, .. } => {
                for (_, e) in fields {
                    Self::walk_exprs(e, f);
                }
            }
            Expr::SumConstructor { args, .. } => {
                for a in args {
                    Self::walk_exprs(a, f);
                }
            }
            Expr::Spread { expr, .. } => Self::walk_exprs(expr, f),
            Expr::Number { .. }
            | Expr::String { .. }
            | Expr::Bool { .. }
            | Expr::Unit { .. }
            | Expr::Ident { .. } => {}
        }
    }

    /// The default return TYPE codegen assigns a function with the given (possibly
    /// missing) return annotation and body: the annotation if present, else `$` (Unit)
    /// for a Unit-tailed body, else `Num`. Codegen lacks the checker's full inference, so
    /// this picks the LLVM-level return type for an unannotated function/lambda/method.
    /// (The entry point `^` is handled separately — it always returns an f64 exit code.)
    fn default_return_type(&self, return_type: Option<&Type>, body: &Expr) -> Type {
        match return_type {
            // A GENERIC annotation — only `-> Result`, whose `Ok(T)`/`NotOk(E)` payload
            // slots are type variables the language can't otherwise name — is refined to
            // the body's concrete type from the oracle, so the LLVM return matches the
            // value the body actually produces (`Ok("x")` => `{ i8, Text }`, not the
            // generic `{ i8, double }`). Mirrors the checker refining a generic return.
            Some(t) if t.contains_generic() => self
                .oracle
                .expr_type(body)
                .cloned()
                .unwrap_or_else(|| t.clone()),
            // A concrete annotation is authoritative.
            Some(t) => t.clone(),
            None if self.expr_is_unit(body) => Type::Unit,
            // Unannotated: the oracle holds the checker's inferred body type (concrete,
            // including a specialized Result payload such as `Result[Ok(Text)]`), so a
            // function returning `Ok("x")` lowers its return to the payload's real shape
            // and a downstream match binds it usably. Fall back to `Num` for the IR-only
            // codegen tests that run without a type-check pass.
            None => self.oracle.expr_type(body).cloned().unwrap_or(Type::Num),
        }
    }

    /// The LLVM signature of a function literal: (source-parameter types, return type).
    /// Mirrors the type rules used when emitting the lifted function, but without the
    /// trailing env pointer (which is implicit to every closure call).
    fn closure_signature(
        &self,
        params: &[crate::ast::Param],
        return_type: Option<&Type>,
        body: &Expr,
    ) -> Result<ClosureSig<'ctx>, String> {
        let param_types: Vec<BasicTypeEnum> = params
            .iter()
            .map(|p| self.boundary_type(&p.type_annotation.clone().unwrap_or(Type::Num)))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = self.default_return_type(return_type, body);
        Ok((param_types, self.boundary_type(&ret)?))
    }

    /// Lower a function literal to a value: lift its body into a fresh top-level function
    /// taking the captured environment as a trailing `ptr` parameter, build and populate
    /// that environment on the heap, and return the `{ ptr fn, ptr env }` closure struct.
    ///
    /// Capture rule, inferred from each captured name's binding operator:
    ///   `=`  binding -> captured BY VALUE: a snapshot is copied into the env (read-only).
    ///   `:=` binding -> captured BY REFERENCE: the env holds the pointer to the shared
    ///                   GC cell (the box), so reads see — and writes escape to — the one
    ///                   cell, surviving the closure outliving its defining frame.
    fn generate_lambda(
        &mut self,
        params: &[crate::ast::Param],
        return_type: Option<&Type>,
        body: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        // 1. Determine the captured names: a lambda free variable that is actually a
        //    binding in the current frame. A by-reference capture is one whose name is in
        //    the current `boxed_vars` (its storage is a shared cell); the rest are by-value.
        let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let outer: std::collections::HashSet<String> = self.variables.keys().cloned().collect();
        let free = crate::ast::captures::lambda_free_idents(&param_names, body, &outer);
        let mut captures: Vec<Capture<'ctx>> = Vec::new();
        for name in free {
            // Only names with a live local slot are captured; a free name that resolves to
            // a top-level function/global is referenced directly inside the lifted body
            // (it has module scope) and needs no capture.
            if let Some((slot, value_ty)) = self.variables.get(&name).copied() {
                let by_ref = self.boxed_vars.contains(&name);
                let closure_sig = self.closure_sigs.get(&name).cloned();
                captures.push(Capture {
                    name,
                    slot,
                    value_ty,
                    by_ref,
                    closure_sig,
                });
            }
        }

        // 2. Build the environment struct type. A by-value capture stores the value; a
        //    by-reference capture stores the cell pointer (`ptr`).
        let env_field_types: Vec<BasicTypeEnum> = captures
            .iter()
            .map(|c| if c.by_ref { ptr_ty.into() } else { c.value_ty })
            .collect();
        let env_struct_ty = self.context.struct_type(&env_field_types, false);

        // 3. Allocate and populate the environment on the GC heap (so it survives the
        //    closure escaping). For a by-value capture, snapshot the current value; for a
        //    by-reference capture, store the shared cell pointer itself.
        let env_ptr = if captures.is_empty() {
            ptr_ty.const_null()
        } else {
            let env = self.alloc_box(env_struct_ty.into())?;
            for (i, cap) in captures.iter().enumerate() {
                let field = self
                    .builder
                    .build_struct_gep(env_struct_ty, env, i as u32, "env_field")
                    .map_err(|e| format!("Failed to GEP env field: {:?}", e))?;
                let stored: BasicValueEnum = if cap.by_ref {
                    cap.slot.into()
                } else {
                    self.builder
                        .build_load(cap.value_ty, cap.slot, &cap.name)
                        .map_err(|e| format!("Failed to load capture: {:?}", e))?
                };
                self.builder
                    .build_store(field, stored)
                    .map_err(|e| format!("Failed to store capture: {:?}", e))?;
            }
            env
        };

        // 4. Emit the lifted top-level function `__lambda_N(params..., ptr env)`.
        let fn_value =
            self.emit_lambda_function(params, return_type, body, &captures, env_struct_ty)?;

        // 5. Assemble the closure value `{ fn_ptr, env_ptr }`.
        let closure_ty = self.closure_struct_type();
        let fn_ptr = fn_value.as_global_value().as_pointer_value();
        let with_fn = self
            .builder
            .build_insert_value(closure_ty.get_undef(), fn_ptr, 0, "clo_fn")
            .map_err(|e| format!("Failed to insert closure fn: {:?}", e))?
            .into_struct_value();
        let closure = self
            .builder
            .build_insert_value(with_fn, env_ptr, 1, "clo_env")
            .map_err(|e| format!("Failed to insert closure env: {:?}", e))?
            .into_struct_value();
        Ok(closure.into())
    }

    /// Emit the lifted top-level function for a lambda: its source parameters followed by
    /// a trailing `ptr env`. Inside, parameters are bound normally and each captured name
    /// is re-bound from the environment — a by-value capture is copied into a local slot,
    /// a by-reference capture re-uses the shared cell pointer directly (so writes escape).
    /// Saves and restores the enclosing codegen state (current function, variable scope,
    /// boxed set, builder position) around the nested emission.
    fn emit_lambda_function(
        &mut self,
        params: &[crate::ast::Param],
        return_type: Option<&Type>,
        body: &Expr,
        captures: &[Capture<'ctx>],
        env_struct_ty: inkwell::types::StructType<'ctx>,
    ) -> Result<FunctionValue<'ctx>, String> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        // Same (param types, return type) the call site reconstructs, plus the trailing
        // env pointer — keeping the emitted function and the indirect-call type in lockstep.
        let (mut param_types, ret_ty) = self.closure_signature(params, return_type, body)?;
        param_types.push(ptr_ty.into()); // trailing env pointer

        let fn_type = ret_ty.fn_type(
            &param_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        let name = format!("__lambda_{}", self.lambda_counter);
        self.lambda_counter += 1;
        let function = self.module.add_function(&name, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);

        // Save enclosing emission state — we are about to emit a DIFFERENT function body.
        let saved_block = self.builder.get_insert_block();
        let saved_function = self.current_function;
        let saved_frame = self.take_frame();
        // The lifted body has its own frame: recompute which of ITS `:=` locals are boxed.
        self.boxed_vars = self.compute_boxed_vars(body);
        // A by-reference capture is ALSO a shared cell in this frame: mark it boxed so a
        // FURTHER nested closure capturing the same name captures it by reference too
        // (sharing the one cell across all nesting levels). Without this, a `:=` value
        // mutated through two levels of closures would be silently snapshotted by value.
        for cap in captures.iter().filter(|c| c.by_ref) {
            self.boxed_vars.insert(cap.name.clone());
        }
        // A captured variable's Quilon-type metadata must travel into the lifted frame
        // with it: field access, method dispatch, and overloaded-call mangling on the
        // captured name inside the closure body all resolve through these maps.
        for cap in captures {
            if let Some(qty) = saved_frame.var_types.get(&cap.name) {
                self.var_types.insert(cap.name.clone(), qty.clone());
            }
            if let Some(fields) = saved_frame.record_types.get(&cap.name) {
                self.record_types.insert(cap.name.clone(), fields.clone());
            }
            if let Some(type_name) = saved_frame.var_named_types.get(&cap.name) {
                self.var_named_types
                    .insert(cap.name.clone(), type_name.clone());
            }
        }
        self.current_function = Some(function);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Bind source parameters (indices 0..n); the env pointer is the last parameter.
        for (i, param) in params.iter().enumerate() {
            let llvm_param = function.get_nth_param(i as u32).unwrap();
            llvm_param.set_name(&param.name);
            let pty = llvm_param.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&param.name, pty)?;
            self.builder
                .build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to store param: {:?}", e))?;
            self.variables.insert(param.name.clone(), (alloca, pty));
        }

        // Re-bind captures from the environment pointer (the trailing parameter).
        if !captures.is_empty() {
            let env_ptr = function
                .get_nth_param(params.len() as u32)
                .unwrap()
                .into_pointer_value();
            for (i, cap) in captures.iter().enumerate() {
                let field = self
                    .builder
                    .build_struct_gep(env_struct_ty, env_ptr, i as u32, "cap_field")
                    .map_err(|e| format!("Failed to GEP capture field: {:?}", e))?;
                if cap.by_ref {
                    // The field holds the shared cell pointer; load it and bind the name
                    // to that cell so reads/writes inside the closure hit the one cell.
                    let cell = self
                        .builder
                        .build_load(ptr_ty, field, &cap.name)
                        .map_err(|e| format!("Failed to load cell ptr: {:?}", e))?
                        .into_pointer_value();
                    self.variables
                        .insert(cap.name.clone(), (cell, cap.value_ty));
                } else {
                    // By-value capture: copy the snapshot into a fresh local slot.
                    let val = self
                        .builder
                        .build_load(cap.value_ty, field, &cap.name)
                        .map_err(|e| format!("Failed to load capture value: {:?}", e))?;
                    let alloca = self.create_entry_block_alloca(&cap.name, cap.value_ty)?;
                    self.builder
                        .build_store(alloca, val)
                        .map_err(|e| format!("Failed to store capture value: {:?}", e))?;
                    self.variables
                        .insert(cap.name.clone(), (alloca, cap.value_ty));
                }
                // If the captured value is itself a closure, re-register its signature so
                // a `name(args)` inside this lifted body resolves to an indirect call (the
                // lifted body began with a cleared `closure_sigs`).
                if let Some(sig) = &cap.closure_sig {
                    self.closure_sigs.insert(cap.name.clone(), sig.clone());
                }
            }
        }

        let body_value = self.generate_expr(body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(|e| format!("Failed to build closure return: {:?}", e))?;

        // Restore the enclosing emission state.
        self.restore_frame(saved_frame);
        self.current_function = saved_function;
        if let Some(block) = saved_block {
            self.builder.position_at_end(block);
        }

        Ok(function)
    }

    fn generate_expr(&mut self, expr: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        match expr {
            Expr::Number { value, .. } => {
                // For now, use f64 for all numbers
                Ok(self.context.f64_type().const_float(*value).into())
            }

            Expr::String { value, .. } => {
                // Text is { ptr data, i64 byte_len }. `data` points at a global,
                // NUL-terminated C string (so `print` can treat it as a C string);
                // `byte_len` is the UTF-8 byte length, excluding the terminator.
                let global = self
                    .builder
                    .build_global_string_ptr(value, "str")
                    .map_err(|e| format!("Failed to build string: {:?}", e))?;
                let data_ptr = global.as_pointer_value();
                let len = self.context.i64_type().const_int(value.len() as u64, false);
                let text_ty = self.ptr_len_struct_type();
                let with_ptr = self
                    .builder
                    .build_insert_value(text_ty.get_undef(), data_ptr, 0, "text_ptr")
                    .map_err(|e| format!("Failed to insert text ptr: {:?}", e))?
                    .into_struct_value();
                let text = self
                    .builder
                    .build_insert_value(with_ptr, len, 1, "text_len")
                    .map_err(|e| format!("Failed to insert text len: {:?}", e))?
                    .into_struct_value();
                Ok(text.into())
            }

            Expr::Bool { value, .. } => Ok(self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into()),

            // The unit value `$`: a zero `i8` placeholder. The value is never
            // inspected; it just needs a concrete, single-inhabitant representation.
            Expr::Unit { .. } => Ok(self.unit_value().into()),

            Expr::Ident { name, .. } => {
                // A bare nullary sum-type constructor (e.g. `Red`) builds its tagged
                // struct here. Payload-carrying constructors are calls, handled above.
                // (We only treat it as a constructor when it isn't a bound variable.)
                if !self.variables.contains_key(name)
                    && let Some((tag, type_name)) = self.sum_variants.get(name).cloned()
                {
                    return self.generate_sum_constructor(tag, &type_name, &[]);
                }
                // Local binding (function-scoped alloca) first.
                if let Some((ptr, ty)) = self.variables.get(name) {
                    return self
                        .builder
                        .build_load(*ty, *ptr, name)
                        .map_err(|e| format!("Failed to build load: {:?}", e));
                }
                // Otherwise a top-level/module global constant (e.g. core.io's
                // `stdout`/`stderr`, or any top-level `name = <const>`).
                if let Some(global) = self.module.get_global(name) {
                    let ty = global
                        .get_initializer()
                        .map(|v| v.get_type())
                        .unwrap_or_else(|| self.context.f64_type().into());
                    return self
                        .builder
                        .build_load(ty, global.as_pointer_value(), name)
                        .map_err(|e| format!("Failed to build load global: {:?}", e));
                }
                Err(format!("Undefined variable: {}", name))
            }

            Expr::BinOp {
                left, op, right, ..
            } => self.generate_binop(left, *op, right),

            Expr::UnaryOp { op, expr, .. } => self.generate_unary_op(*op, expr),

            Expr::Call { func, args, .. } => self.generate_call(func, args),

            Expr::Lambda {
                params,
                return_type,
                body,
                ..
            } => self.generate_lambda(params, return_type.as_ref(), body),

            Expr::If {
                cond, then, else_, ..
            } => self.generate_if(cond, then, else_),

            Expr::Block { stmts, .. } => self.generate_block(stmts),

            Expr::Array { elements, .. } => self.generate_array(expr, elements),

            Expr::Record { fields, .. } => self.generate_record_expr(expr, fields),

            // A bare spread never survives to codegen on its own — the parser only
            // produces one as an element of an array literal or a field of a record
            // literal, where `generate_array` / `generate_record_expr` consume it.
            Expr::Spread { .. } => {
                Err("spread `<-` is only valid inside an array or record literal".to_string())
            }

            Expr::Constructor {
                type_name, fields, ..
            } => {
                // A named-type instance has the same struct representation as a record,
                // but its field SLOTS follow the type's DECLARATION order — which is the
                // order `record_types` and the type oracle use to index/GEP fields later.
                // The constructor call may list fields in any order, so reorder them to
                // declaration order before lowering; otherwise a later `obj.field` read
                // would GEP the wrong slot (silent corruption once fields differ in type).
                let ordered = self.constructor_fields_in_decl_order(type_name, fields);
                self.generate_record(&ordered)
            }

            Expr::FieldAccess { expr, field, .. } => self.generate_field_access(expr, field),

            Expr::FieldAssign { target, value, .. } => self.generate_field_assign(target, value),

            Expr::Index {
                expr: array, index, ..
            } => self.generate_index(expr, array, index),

            Expr::Match {
                expr: scrutinee,
                arms,
                ..
            } => self.generate_match(expr, scrutinee, arms),

            Expr::Range { start, end, .. } => self.generate_range(start, end),

            // `left |> right` desugars to a call with `left` as the first arg
            // (must match the type checker's desugaring exactly).
            Expr::Pipeline { left, right, span } => {
                let call = Expr::desugar_pipeline(left, right, span);
                self.generate_expr(&call)
            }

            _ => Err(format!("Unsupported expression type: {:?}", expr)),
        }
    }

    fn generate_binop(
        &mut self,
        left: &Expr,
        op: BinOp,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A USER operator overload (e.g. `+`/`==` on a record type) lowers to a direct
        // call to its mangled function — the operator is just a named overload set.
        // Built-in operators (Num arithmetic/compare, Text `+`/comparison) keep their
        // inline lowering below; they are not entered in `self.overloads`.
        let sym = op.symbol();
        if self.overloads.contains_key(sym) {
            let arg_types = [self.infer_type(left), self.infer_type(right)];
            if let Some(symbol) = self.resolve_overload_symbol(sym, &arg_types) {
                let l = self.generate_expr(left)?;
                let r = self.generate_expr(right)?;
                return self.build_direct_call(&symbol, &[l, r]);
            }
        }

        // `+` on arrays is concatenation / append / prepend — all produce a NEW array.
        // Arrays and Text both lower to `{ptr,size}` structs, so distinguish by the
        // oracle's Quilon type and route BEFORE the generic StructValue path below (which
        // is Text concat). Triggered when either operand is an array: `[]T + []T`,
        // `[]T + T` (append), or `T + []T` (prepend).
        if op == BinOp::Add
            && (matches!(self.oracle.expr_type(left), Some(Type::Array(_)))
                || matches!(self.oracle.expr_type(right), Some(Type::Array(_))))
        {
            return self.generate_array_concat(left, right);
        }

        // `&&`/`||` are SHORT-CIRCUIT (LANGUAGE.md "Logical: `&& || !` (short-circuit)"):
        // the right operand must NOT be evaluated when the left already decides the
        // result — `i < a.size && a[i] == k` must never index out of bounds, and a
        // side-effecting right operand must not run. Lower with control flow BEFORE the
        // eager operand evaluation below.
        if matches!(op, BinOp::And | BinOp::Or) {
            return self.generate_short_circuit(op, left, right);
        }

        let lhs = self.generate_expr(left)?;
        let rhs = self.generate_expr(right)?;

        // Text comparison: both operands are `Text` { ptr, i64 } structs. Lower
        // equality and lexicographic ordering via the `__text_cmp` runtime intrinsic
        // (returns -1/0/1), then compare its result against 0 with the matching
        // integer predicate. (Num operands fall through to the float paths below.)
        if matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        ) && matches!(lhs, BasicValueEnum::StructValue(_))
            && matches!(rhs, BasicValueEnum::StructValue(_))
        {
            return self.generate_text_compare(op, lhs, rhs);
        }

        match op {
            BinOp::Add => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_add(l, r, "addtmp")
                    .map_err(|e| format!("Failed to build add: {:?}", e))?
                    .into()),
                // Text + Text = concatenation (both are { ptr, i64 } structs).
                (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) => {
                    self.generate_text_concat(l, r)
                }
                _ => Err("Add requires two Nums or two Texts".to_string()),
            },
            BinOp::Sub => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_sub(l, r, "subtmp")
                        .map_err(|e| format!("Failed to build sub: {:?}", e))?
                        .into())
                } else {
                    Err("Sub operation requires float values".to_string())
                }
            }
            BinOp::Mul => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_mul(l, r, "multmp")
                        .map_err(|e| format!("Failed to build mul: {:?}", e))?
                        .into())
                } else {
                    Err("Mul operation requires float values".to_string())
                }
            }
            BinOp::Div => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_div(l, r, "divtmp")
                        .map_err(|e| format!("Failed to build div: {:?}", e))?
                        .into())
                } else {
                    Err("Div operation requires float values".to_string())
                }
            }
            BinOp::Eq => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l, r, "eqtmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                // Bool == Bool (both i1) compares the integer values.
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, l, r, "eqtmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                _ => Err("Eq requires two Nums or two Bools".to_string()),
            },
            BinOp::Ne => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l, r, "netmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, l, r, "netmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                _ => Err("Ne requires two Nums or two Bools".to_string()),
            },
            BinOp::Lt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLT, l, r, "lttmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Lt operation requires float values".to_string())
                }
            }
            BinOp::Le => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLE, l, r, "letmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Le operation requires float values".to_string())
                }
            }
            BinOp::Gt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGT, l, r, "gttmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Gt operation requires float values".to_string())
                }
            }
            BinOp::Ge => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGE, l, r, "getmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Ge operation requires float values".to_string())
                }
            }
            // `&&`/`||` never reach here — `generate_binop` routes them to
            // `generate_short_circuit` before operand evaluation.
            _ => Err(format!("Unsupported binary operation: {:?}", op)),
        }
    }

    /// Lower `&&`/`||` with SHORT-CIRCUIT control flow: evaluate the left operand, and
    /// only branch into the right operand when the left does not already decide the
    /// result (`false` decides `&&`; `true` decides `||`). The merged value is a phi of
    /// the deciding constant and the right operand's boolean. Shape mirrors
    /// `generate_if`.
    fn generate_short_circuit(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Logical operator outside of function".to_string())?;

        let lhs_val = self.generate_expr(left)?;
        let lhs_bool = self.value_to_boolean(lhs_val)?;
        // The left operand may itself have emitted branches; the phi's incoming edge is
        // the block we END in, not the one we started in.
        let lhs_end = self
            .builder
            .get_insert_block()
            .ok_or("Logical operator outside of a block")?;

        let rhs_bb = self.context.append_basic_block(function, "sc_rhs");
        let merge_bb = self.context.append_basic_block(function, "sc_merge");

        // `&&`: a true left falls through to the right, a false left decides.
        // `||`: a false left falls through to the right, a true left decides.
        let (true_bb, false_bb) = match op {
            BinOp::And => (rhs_bb, merge_bb),
            _ => (merge_bb, rhs_bb),
        };
        self.builder
            .build_conditional_branch(lhs_bool, true_bb, false_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        self.builder.position_at_end(rhs_bb);
        let rhs_val = self.generate_expr(right)?;
        let rhs_bool = self.value_to_boolean(rhs_val)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let rhs_end = self
            .builder
            .get_insert_block()
            .ok_or("Logical operator outside of a block")?;

        self.builder.position_at_end(merge_bb);
        // On the skipped-right edge the left operand already decided the result, so the
        // value it carries is exactly `lhs_bool` (`&&` took this edge only when it was
        // false, `||` only when it was true). No separate deciding constant is needed.
        let phi = self
            .builder
            .build_phi(self.context.bool_type(), "sctmp")
            .map_err(|e| format!("Failed to build phi: {:?}", e))?;
        phi.add_incoming(&[(&lhs_bool, lhs_end), (&rhs_bool, rhs_end)]);
        Ok(phi.as_basic_value())
    }

    // Helper to convert a value to boolean (i1)
    fn value_to_boolean(
        &mut self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) => {
                // Already an int - check if it's i1
                if i.get_type().get_bit_width() == 1 {
                    Ok(i)
                } else {
                    // Convert to i1 by comparing with 0
                    self.builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            i,
                            i.get_type().const_zero(),
                            "tobool",
                        )
                        .map_err(|e| format!("Failed to convert to bool: {:?}", e))
                }
            }
            BasicValueEnum::FloatValue(f) => {
                // Convert float to bool by comparing with 0.0
                self.builder
                    .build_float_compare(
                        inkwell::FloatPredicate::ONE, // Ordered Not Equal
                        f,
                        f.get_type().const_zero(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to convert float to bool: {:?}", e))
            }
            _ => Err("Cannot convert value to boolean".to_string()),
        }
    }

    fn generate_unary_op(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let val = self.generate_expr(expr)?;

        match op {
            UnaryOp::Neg => {
                if let BasicValueEnum::FloatValue(f) = val {
                    Ok(self
                        .builder
                        .build_float_neg(f, "negtmp")
                        .map_err(|e| format!("Failed to build neg: {:?}", e))?
                        .into())
                } else {
                    Err("Neg operation requires float value".to_string())
                }
            }
            UnaryOp::Not => {
                if let BasicValueEnum::IntValue(i) = val {
                    Ok(self
                        .builder
                        .build_not(i, "nottmp")
                        .map_err(|e| format!("Failed to build not: {:?}", e))?
                        .into())
                } else {
                    Err("Not operation requires int value".to_string())
                }
            }
        }
    }

    /// Declare (once) and return an external runtime intrinsic by its
    /// Quilon-internal name. These resolve to `#[no_mangle]` symbols in
    /// `src/runtime/intrinsics.rs` (or libc, e.g. `memcpy`) — available both to
    /// the in-process JIT and to AOT-linked executables.
    fn get_intrinsic(&self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        let ctx = self.context;
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let f64t = ctx.f64_type();
        let void = ctx.void_type();
        let fn_type = match name {
            // i8* __alloc(i64) — GC-managed allocation.
            "__alloc" => ptr.fn_type(&[i64t.into()], false),
            // void __gc_init() — initialize the Boehm GC.
            "__gc_init" => void.fn_type(&[], false),
            // void __exit(i32 code) — terminate the process with `code`. Backs the
            // `__exit(n)` primitive that `core.test`'s `assert` calls to fail. Never
            // returns (the runtime calls libc `exit`).
            "__exit" => void.fn_type(&[ctx.i32_type().into()], false),
            // i8* memcpy(i8*, i8*, i64) — libc.
            "memcpy" => ptr.fn_type(&[ptr.into(), ptr.into(), i64t.into()], false),
            // i64 __text_length(i8*, i64) — grapheme-cluster count.
            "__text_length" => i64t.fn_type(&[ptr.into(), i64t.into()], false),
            // i32 __text_cmp(i8* a, i64 alen, i8* b, i64 blen) — lexicographic byte
            // comparison, returning -1 / 0 / 1. Backs Text ==/!=/</<=/>/>=.
            "__text_cmp" => ctx
                .i32_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // i64 __write_bytes(i64 fd, i8* ptr, i64 len) — raw write, backs `write`.
            "__write_bytes" => i64t.fn_type(&[i64t.into(), ptr.into(), i64t.into()], false),
            // void __print_num_fd(i64 fd, double) — number + newline to fd.
            "__print_num_fd" => void.fn_type(&[i64t.into(), f64t.into()], false),
            // void __print_bool_fd(i64 fd, i64 b) — "true"/"false" + newline to fd.
            "__print_bool_fd" => void.fn_type(&[i64t.into(), i64t.into()], false),
            // void __print_text_fd(i64 fd, i8*) — C string + newline to fd.
            "__print_text_fd" => void.fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __argv_to_text_array(i64 argc, i8** argv) — build a `[]Text`
            // (array of `{ptr,i64}` Text structs) from the C argc/argv. Returns the
            // `[]Text` value struct (same shape as `ptr_len_struct_type`).
            "__argv_to_text_array" => self
                .ptr_len_struct_type()
                .fn_type(&[i64t.into(), ptr.into()], false),
            // { ptr, i64 } __envp_to_pairs(i8** envp) — build a `[][]Text` (array of
            // 2-element `[]Text` `[key, value]` pairs) from the C envp.
            "__envp_to_pairs" => self.ptr_len_struct_type().fn_type(&[ptr.into()], false),
            // Text methods. A `Text`/`[]Text` result is the `{ ptr, i64 }` struct; a
            // `Text` argument is passed as its (ptr, i64) fields. See `quilon-rt`.
            // { ptr, i64 } trimStart / trimEnd / toUpper / toLower (i8*, i64). `trim` is
            // composed from trimStart+trimEnd in codegen, so it has no own intrinsic.
            "__text_trim_start" | "__text_trim_end" | "__text_to_upper" | "__text_to_lower" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into()], false),
            // i64 __text_contains / __text_index_of (i8* hay, i64, i8* sub, i64).
            "__text_contains" | "__text_index_of" => {
                i64t.fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false)
            }
            // { ptr, i64 } __text_split(i8* hay, i64, i8* sep, i64) -> `[]Text`.
            "__text_split" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            // { ptr, i64 } __text_slice(i8*, i64, i64 start, i64 end).
            "__text_slice" => self
                .ptr_len_struct_type()
                .fn_type(&[ptr.into(), i64t.into(), i64t.into(), i64t.into()], false),
            // { ptr, i64 } __text_replace_all(i8* hay,i64, i8* from,i64, i8* to,i64).
            "__text_replace_all" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                ],
                false,
            ),
            // { ptr, i64 } __text_replace_n(i8* hay,i64, i8* from,i64, i8* to,i64, i64 count).
            "__text_replace_n" => self.ptr_len_struct_type().fn_type(
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                ],
                false,
            ),
            other => return Err(format!("Unknown runtime intrinsic: {}", other)),
        };
        Ok(self.module.add_function(name, fn_type, None))
    }

    /// Lower a `print`/`eprint` builtin call: render the single argument's text
    /// and write it, followed by a newline, to stdout (`print`, fd 1) or stderr
    /// (`eprint`, fd 2). Dispatches on the LLVM type of the argument: floats print
    /// as numbers, Text structs / pointers as C strings, integers (incl. bools)
    /// widen to numbers. Yields `Num` 0, so it is usable in expression position.
    fn generate_print(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "{} expects exactly 1 argument, got {}",
                name,
                args.len()
            ));
        }
        let fd = if name == "eprint" { 2 } else { 1 };
        let fd_val = self.context.i64_type().const_int(fd, false);
        let val = self.generate_expr(&args[0])?;
        let (intrinsic, arg): (&str, inkwell::values::BasicMetadataValueEnum) = match val {
            BasicValueEnum::FloatValue(f) => ("__print_num_fd", f.into()),
            // Text is { ptr data, i64 len }; print its NUL-terminated `data`.
            BasicValueEnum::StructValue(s) => {
                let data = self
                    .builder
                    .build_extract_value(s, 0, "text_data")
                    .map_err(|e| format!("Failed to extract text data: {:?}", e))?
                    .into_pointer_value();
                ("__print_text_fd", data.into())
            }
            // A bare pointer (C string) prints as text.
            BasicValueEnum::PointerValue(p) => ("__print_text_fd", p.into()),
            // A Bool (i1) prints as "true"/"false"; any wider int widens to a number.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 1 => {
                let b = self
                    .builder
                    .build_int_z_extend(i, self.context.i64_type(), "bool_ext")
                    .map_err(|e| format!("Failed to extend bool for print: {:?}", e))?;
                ("__print_bool_fd", b.into())
            }
            BasicValueEnum::IntValue(i) => {
                let f = self
                    .builder
                    .build_unsigned_int_to_float(i, self.context.f64_type(), "print_num")
                    .map_err(|e| format!("Failed to convert int for print: {:?}", e))?;
                ("__print_num_fd", f.into())
            }
            other => {
                return Err(format!(
                    "print does not support a value of type {:?}",
                    other.get_type()
                ));
            }
        };
        let print_fn = self.get_intrinsic(intrinsic)?;
        self.builder
            .build_call(print_fn, &[fd_val.into(), arg], "")
            .map_err(|e| format!("Failed to build print call: {:?}", e))?;
        // `print`/`eprint` yield Unit (`$`); their result is meaningless.
        Ok(self.unit_value().into())
    }

    /// Lower the `__exit(code)` primitive: convert the `Num` `code` to an `i32` and
    /// call the `__exit` runtime intrinsic, which terminates the process. This is the
    /// single native primitive `core.test` builds on (its `assert` calls `__exit(101)`
    /// on failure). The intrinsic never returns, but the call is left as ordinary
    /// (non-`unreachable`) flow so it composes wherever an expression is expected —
    /// e.g. a `< >` block statement or a ternary arm inside `assert` — without
    /// clashing with the surrounding construct's own terminator. The code after it is
    /// dead at runtime (the process has exited). Yields `$` (Unit).
    fn generate_exit(&mut self, args: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "__exit expects exactly 1 argument, got {}",
                args.len()
            ));
        }
        let code = self.generate_expr(&args[0])?;
        let BasicValueEnum::FloatValue(code_f) = code else {
            return Err("__exit expects a Num exit code".to_string());
        };
        let code_i32 = self
            .builder
            .build_float_to_signed_int(code_f, self.context.i32_type(), "exit_code")
            .map_err(|e| format!("Failed to convert __exit code: {:?}", e))?;
        let exit_fn = self.get_intrinsic("__exit")?;
        self.builder
            .build_call(exit_fn, &[code_i32.into()], "")
            .map_err(|e| format!("Failed to build __exit call: {:?}", e))?;
        // `__exit` never returns; yield Unit so the call composes in expression position.
        Ok(self.unit_value().into())
    }

    /// Lower the `write(content, fd)` builtin: write the raw bytes of a `Text`
    /// `content` to file descriptor `fd` (a `Num`), with no trailing newline.
    /// Yields `Num` (bytes written).
    fn generate_write(&mut self, args: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 2 {
            return Err(format!(
                "write expects exactly 2 arguments (content, fd), got {}",
                args.len()
            ));
        }
        let content = self.generate_expr(&args[0])?;
        let fd_num = self.generate_expr(&args[1])?;
        // content must be a Text { ptr data, i64 byte_len }.
        let s = match content {
            BasicValueEnum::StructValue(s) => s,
            other => {
                return Err(format!(
                    "write expects a Text content, got {:?}",
                    other.get_type()
                ));
            }
        };
        let data = self
            .builder
            .build_extract_value(s, 0, "write_data")
            .map_err(|e| format!("Failed to extract text data: {:?}", e))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(s, 1, "write_len")
            .map_err(|e| format!("Failed to extract text len: {:?}", e))?
            .into_int_value();
        let fd_float = match fd_num {
            BasicValueEnum::FloatValue(f) => f,
            other => {
                return Err(format!(
                    "write expects a Num fd, got {:?}",
                    other.get_type()
                ));
            }
        };
        let fd_i64 = self
            .builder
            .build_float_to_signed_int(fd_float, self.context.i64_type(), "write_fd")
            .map_err(|e| format!("Failed to convert fd: {:?}", e))?;
        let write_fn = self.get_intrinsic("__write_bytes")?;
        use inkwell::values::AnyValue;
        let written = self
            .builder
            .build_call(
                write_fn,
                &[fd_i64.into(), data.into(), len.into()],
                "write_n",
            )
            .map_err(|e| format!("Failed to call __write_bytes: {:?}", e))?
            .as_any_value_enum()
            .into_int_value();
        Ok(self
            .builder
            .build_signed_int_to_float(written, self.context.f64_type(), "write_ret")
            .map_err(|e| format!("Failed to convert write result: {:?}", e))?
            .into())
    }

    fn generate_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Get function name - only support direct calls for now
        let func_name = if let Expr::Ident { name, .. } = func {
            name
        } else {
            return Err("Only direct function calls supported".to_string());
        };

        // Core IO builtins, lowered to runtime intrinsics (see runtime::intrinsics).
        // `print`/`eprint` are the built-in single-arg Num/Text/Bool overloads; a
        // *user* overload of the same name (a different signature) is dispatched as a
        // mangled function below, so only use the intrinsic when no user overload
        // matches the argument types.
        match func_name.as_str() {
            "print" | "eprint" => {
                let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                let is_builtin_print = arg_types.len() == 1
                    && matches!(arg_types[0], Type::Num | Type::Text | Type::Bool);
                let has_user_match = self
                    .resolve_overload_symbol(func_name, &arg_types)
                    .is_some();
                if is_builtin_print && !has_user_match {
                    return self.generate_print(func_name, args);
                }
            }
            "write" => return self.generate_write(args),
            // `__exit(code)` — the single native primitive `core.test` builds on,
            // lowered to the `__exit` runtime intrinsic (terminates the process).
            "__exit" => return self.generate_exit(args),
            _ => {}
        }

        // Built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) — RESERVED
        // on arrays. The method applies only when the receiver (`args[0]`) is an array;
        // the oracle confirms its element type, so this never diverts a same-named user
        // overload on a non-array receiver. Method names are lowercase and so can never
        // collide with a (Capitalized) sum-constructor name — the relative order of this
        // check and the sum-constructor block below is therefore immaterial.
        if crate::ast::is_array_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Array(_)))
        {
            return self.generate_array_method(func_name, args);
        }

        // Built-in Text methods — RESERVED on `Text`, mirroring the array-method block:
        // dispatch only when the receiver (`args[0]`) is a `Text` (per the oracle), so a
        // same-named user overload on another type is never diverted. Lowercase/camelCase
        // names never collide with (Capitalized) sum constructors.
        if crate::ast::is_text_method(func_name)
            && !args.is_empty()
            && matches!(self.oracle.expr_type(&args[0]), Some(Type::Text))
        {
            return self.generate_text_method(func_name, args);
        }

        // Sum-type constructor with a payload (e.g. `Ok(x)`, `Circle(r)`, `Rect(w, h)`):
        // resolved from the variant registry built from the predefined Result and all
        // user `TypeDef::Sum` declarations.
        if let Some((tag, type_name)) = self.sum_variants.get(func_name.as_str()).cloned() {
            return self.generate_sum_constructor(tag, &type_name, args);
        }

        // A local variable bound to a closure value: call it indirectly, passing the
        // captured environment as the trailing argument. Recognized by the variable's
        // recorded closure signature (see `closure_sigs`). Checked before overload
        // dispatch — a local closure binding shadows any same-named top-level function.
        if let Some((param_tys, ret_ty)) = self.closure_sigs.get(func_name.as_str()).cloned()
            && self.variables.contains_key(func_name.as_str())
        {
            return self.generate_closure_call(func_name, &param_tys, ret_ty, args);
        }

        // Overloaded function call: dispatch to the per-signature mangled symbol chosen
        // by exact argument types (the type checker has already verified a unique match).
        let overload_symbol = if self.overloads.contains_key(func_name.as_str()) {
            let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
            self.resolve_overload_symbol(func_name, &arg_types)
        } else {
            None
        };

        // Get the function from the module. If there is no plain top-level function with this
        // name, it may be a method call: the parser desugars `recv.method(a, b)` to
        // `method(recv, a, b)`, so resolve `recv`'s named type and dispatch to `Type_method`.
        let function = if let Some(sym) = &overload_symbol {
            self.module
                .get_function(sym)
                .ok_or_else(|| format!("Overload not found: {}", sym))?
        } else {
            match self.module.get_function(func_name) {
                Some(f) => f,
                None => {
                    let mangled = args
                        .first()
                        .and_then(|recv| self.receiver_type_name(recv))
                        .map(|type_name| format!("{}_{}", type_name, func_name));
                    match mangled.and_then(|m| self.module.get_function(&m)) {
                        Some(f) => f,
                        None => return Err(format!("Function not found: {}", func_name)),
                    }
                }
            }
        };

        // Generate argument values
        let arg_values: Vec<BasicValueEnum> = args
            .iter()
            .map(|arg| self.generate_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert to BasicMetadataValueEnum for the call
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();

        // Build the call
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(|e| format!("Failed to build call: {:?}", e))?;

        Self::call_result_to_basic(call_site)
    }

    /// Call a closure value held in local variable `var_name`: extract the function and
    /// environment pointers from its `{ ptr fn, ptr env }` struct and emit an indirect
    /// call passing the source arguments followed by the environment pointer. `param_tys`
    /// / `ret_ty` are the closure's recorded signature (excluding the implicit env param).
    fn generate_closure_call(
        &mut self,
        var_name: &str,
        param_tys: &[BasicTypeEnum<'ctx>],
        ret_ty: BasicTypeEnum<'ctx>,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != param_tys.len() {
            return Err(format!(
                "closure `{}` expects {} argument(s), got {}",
                var_name,
                param_tys.len(),
                args.len()
            ));
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let closure_ty = self.closure_struct_type();

        // Load the closure struct from its slot, then split out fn and env pointers.
        let (slot, _) = *self.variables.get(var_name).expect("closure var bound");
        let closure_val = self
            .builder
            .build_load(closure_ty, slot, var_name)
            .map_err(|e| format!("Failed to load closure: {:?}", e))?
            .into_struct_value();
        let fn_ptr = self
            .builder
            .build_extract_value(closure_val, 0, "clo_fn")
            .map_err(|e| format!("Failed to extract closure fn: {:?}", e))?
            .into_pointer_value();
        let env_ptr = self
            .builder
            .build_extract_value(closure_val, 1, "clo_env")
            .map_err(|e| format!("Failed to extract closure env: {:?}", e))?
            .into_pointer_value();

        // Evaluate arguments, then append the environment pointer.
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> =
            Vec::with_capacity(args.len() + 1);
        for arg in args {
            call_args.push(self.generate_expr(arg)?.into());
        }
        call_args.push(env_ptr.into());

        // Reconstruct the callee function type: source params + trailing env ptr -> ret.
        let mut metadata_params: Vec<inkwell::types::BasicMetadataTypeEnum> =
            param_tys.iter().map(|t| (*t).into()).collect();
        metadata_params.push(ptr_ty.into());
        let fn_type = ret_ty.fn_type(&metadata_params, false);

        let call = self
            .builder
            .build_indirect_call(fn_type, fn_ptr, &call_args, "clo_call")
            .map_err(|e| format!("Failed to build indirect call: {:?}", e))?;

        Self::call_result_to_basic(call)
    }

    /// Convert a call site's result to a `BasicValueEnum`, erroring if the callee returns
    /// a non-basic (e.g. void) value. Shared by the direct (`generate_call`) and indirect
    /// closure (`generate_closure_call`) call paths so both handle return kinds identically.
    fn call_result_to_basic(
        call: inkwell::values::CallSiteValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        match call.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("call did not return a basic value".to_string()),
        }
    }

    /// Resolve the named record type of a method-call receiver, if known. Handles both a
    /// variable holding a constructed instance and a constructor expression used directly.
    fn receiver_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident { name, .. } => self.var_named_types.get(name).cloned(),
            Expr::Constructor { type_name, .. } => Some(type_name.clone()),
            _ => None,
        }
    }

    fn generate_sum_constructor(
        &mut self,
        tag: u8,
        type_name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Tagged-union value: { i8 tag, slot0, slot1, ... }.
        //
        // The slot types come from one of two sources:
        //  - USER sum types have a registered canonical layout (`sum_layouts`), sized to
        //    the widest variant, so EVERY value of the type shares one struct shape and a
        //    match arm can extract any variant's slots without going out of range:
        //      Rect(3, 4) -> { i8 1, double 3.0, double 4.0 }
        //      Circle(9)  -> { i8 0, double 9.0, double <undef> }   (slot 1 unused)
        //  - `Result` has NO registered layout: it's sized to the actual payload value,
        //    preserving the historical per-value representation across its generic,
        //    possibly-heterogeneous variants:
        //      Ok(42)       -> { i8 0, double 42.0 }
        //      NotOk("err") -> { i8 1, ptr <str> }
        //
        // Num/Bool payloads are normalized to f64. A `$` (Unit) payload is zero-sized; it
        // is stored as a zero of the slot type so the value still matches the slot/return
        // shape (e.g. `Ok($)` -> { i8 0, double 0.0 }) — the bits are never read.
        let i8_type = self.context.i8_type();
        let f64_type = self.context.f64_type();
        let registered_layout = self.sum_layouts.get(type_name).cloned();

        let tag_val = i8_type.const_int(tag as u64, false);

        // Determine each payload slot's value and type. For a registered layout, the slot
        // type is fixed by position; otherwise (Result) it follows the value, with a `$`
        // payload defaulting to the canonical `double` slot.
        let mut payload_vals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
        for (pos, arg) in args.iter().enumerate() {
            let arg_val = self.generate_expr(arg)?;
            // With a registered layout (user type), the slot type is fixed by position.
            // Without one (Result), the slot follows the value's own type so a Text/Bool
            // payload keeps its real representation — except a `$` (Unit) value, which is
            // zero-sized and defaults to the canonical `double` slot.
            let slot_ty = match registered_layout.as_ref().and_then(|l| l.get(pos).copied()) {
                Some(ty) => ty,
                None if self.expr_is_unit(arg) => f64_type.into(),
                None => self.payload_slot_type(arg_val),
            };
            payload_vals.push(self.coerce_payload(arg_val, slot_ty)?);
        }

        // Build the struct type: tag + (registered layout, or the actual payload types).
        let mut field_types: Vec<BasicTypeEnum> = vec![i8_type.into()];
        match &registered_layout {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.extend(payload_vals.iter().map(|v| v.get_type())),
        }
        let sum_struct = self.context.struct_type(&field_types, false);

        let mut agg = sum_struct.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "with_tag")
            .map_err(|e| format!("Failed to insert tag: {:?}", e))?
            .into_struct_value();
        // Fill the leading slots with this variant's payloads; trailing slots (unused by
        // this variant, in a wider registered layout) stay `undef` — they're only read by
        // an arm matching a different, wider variant, which never runs for this value.
        for (i, payload) in payload_vals.iter().enumerate() {
            agg = self
                .builder
                .build_insert_value(agg, *payload, (i + 1) as u32, "with_payload")
                .map_err(|e| format!("Failed to insert payload: {:?}", e))?
                .into_struct_value();
        }

        Ok(agg.into())
    }

    /// The slot type for a Result payload sized to its actual value: a non-`i1` integer
    /// widens to f64 (the canonical numeric payload), everything else keeps its own type.
    fn payload_slot_type(&self, value: BasicValueEnum<'ctx>) -> BasicTypeEnum<'ctx> {
        match value {
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() != 1 => {
                self.context.f64_type().into()
            }
            other => other.get_type(),
        }
    }

    /// Coerce a payload argument value to its target slot type. Integers (incl. the unit
    /// `i8`) widen to f64 for a numeric slot; a `$` (Unit) value targeting a non-`i8` slot
    /// becomes a zero of that slot type (it carries no information). Otherwise the value
    /// is stored as-is (e.g. a Text struct into a Text slot).
    fn coerce_payload(
        &self,
        value: BasicValueEnum<'ctx>,
        slot_ty: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) if slot_ty.is_float_type() => Ok(self
                .builder
                .build_unsigned_int_to_float(i, slot_ty.into_float_type(), "inttofloat")
                .map_err(|e| format!("Failed to convert payload to float: {:?}", e))?
                .into()),
            // A value already matching the slot type passes through unchanged.
            other if other.get_type() == slot_ty => Ok(other),
            // A `$` (Unit) value — the zero `i8` — carries no information; stored into a
            // differently-typed slot it becomes that slot's zero (e.g. a `$` payload in a
            // `Done($) / Pending(Text)` Text slot). The type checker guarantees concrete
            // payload types agree per position, so ANY other mismatch is an internal bug,
            // surfaced rather than silently zeroed.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 8 => Ok(zeroed(slot_ty)),
            other => Err(format!(
                "internal error: sum-type payload of type {:?} does not fit slot {:?}",
                other.get_type(),
                slot_ty
            )),
        }
    }

    fn generate_if(
        &mut self,
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let cond_val = self.generate_expr(cond)?;

        let cond_bool = if let BasicValueEnum::IntValue(i) = cond_val {
            i
        } else {
            return Err("Condition must be a boolean".to_string());
        };

        let function = self
            .current_function
            .ok_or_else(|| "If expression outside of function".to_string())?;

        // Create blocks
        let then_bb = self.context.append_basic_block(function, "then");
        let else_bb = self.context.append_basic_block(function, "else");
        let merge_bb = self.context.append_basic_block(function, "ifcont");

        // Build conditional branch
        self.builder
            .build_conditional_branch(cond_bool, then_bb, else_bb)
            .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

        // Generate then block
        self.builder.position_at_end(then_bb);
        let then_val = self.generate_expr(then_expr)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let then_bb = self.builder.get_insert_block().unwrap();

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_val = self.generate_expr(else_expr)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let else_bb = self.builder.get_insert_block().unwrap();

        // Generate merge block
        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(then_val.get_type(), "iftmp")
            .map_err(|e| format!("Failed to build phi: {:?}", e))?;
        phi.add_incoming(&[(&then_val, then_bb), (&else_val, else_bb)]);

        Ok(phi.as_basic_value())
    }

    fn generate_block(
        &mut self,
        stmts: &[crate::ast::Statement],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let mut result = self.context.f64_type().const_float(0.0).into();

        for stmt in stmts {
            match stmt {
                crate::ast::Statement::Item(item) => {
                    self.generate_item(item)?;
                }
                crate::ast::Statement::Expr(expr) => {
                    result = self.generate_expr(expr)?;
                }
            }
        }

        Ok(result)
    }

    fn generate_array(
        &mut self,
        array_expr: &Expr,
        elements: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Arrays are represented as structs: { ptr data, i64 size }
        // This allows .size field access

        if self.current_function.is_none() {
            return Err("Global arrays not yet implemented".to_string());
        }

        // A literal containing a `<-` spread (`[<-xs, 4]`) has a runtime-determined size
        // (each spread source contributes its own `.size` elements), so it takes a
        // dedicated GC-allocating path that copies each part in order.
        if elements.iter().any(|e| matches!(e, Expr::Spread { .. })) {
            return self.generate_array_spread(array_expr, elements);
        }

        let size = elements.len();

        if size == 0 {
            // Empty array - create struct with null ptr and size 0
            let ptr_type = self.context.ptr_type(AddressSpace::default());
            let i64_type = self.context.i64_type();
            let array_struct_type = self
                .context
                .struct_type(&[ptr_type.into(), i64_type.into()], false);

            let null_ptr = ptr_type.const_zero();
            let zero_size = i64_type.const_zero();

            return Ok(array_struct_type
                .const_named_struct(&[null_ptr.into(), zero_size.into()])
                .into());
        }

        // Generate all element values
        let values: Vec<BasicValueEnum> = elements
            .iter()
            .map(|e| self.generate_expr(e))
            .collect::<Result<Vec<_>, _>>()?;

        // Get element type from first element.
        let elem_type = values[0].get_type();

        // Lay the elements into a GC-allocated buffer via the shared array builder — the
        // SAME mechanism used by `+` concatenation and `<-` spread. Heap (not stack)
        // allocation is essential: an array is a `{ ptr, i64 }` value whose data must
        // outlive the current frame (e.g. when the literal is returned from a function),
        // and `build_array_from_parts` already GC-allocates. Each literal element is an
        // `Inline` part (contributing one slot).
        let parts: Vec<ArrayPart<'ctx>> = values.into_iter().map(ArrayPart::Inline).collect();
        self.build_array_from_parts(elem_type, &parts)
    }

    /// Lower an array literal that contains one or more `<-` spreads (`[<-xs, 4, <-ys]`).
    /// The result size is only known at runtime (each spread contributes its source's
    /// `.size`), so the backing storage is GC-allocated to the exact total and filled
    /// left-to-right: an inline element is stored at the running offset, a spread is a
    /// flat `memcpy` of its source's data (works for any element repr — `[]Num`, `[]Text`,
    /// nested arrays — since element storage is POD in every case). The element repr type
    /// comes from the type oracle (`[]elem`), so `[]Text` spreads copy `{ptr,len}` slots
    /// correctly, not a hardcoded `f64`.
    fn generate_array_spread(
        &mut self,
        array_expr: &Expr,
        elements: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Element repr type from the oracle (`[]elem`); fall back to f64 if the oracle
        // has no entry (IR-only codegen tests that skip type-checking).
        let elem_llvm = match self.oracle.expr_type(array_expr) {
            Some(Type::Array(elem)) => self.value_repr_type(elem)?,
            _ => self.context.f64_type().into(),
        };

        // Generate each part once, tagged spread-or-inline. A spread source lowers to a
        // `{ptr, size}` array struct; an inline element lowers to an `elem` value.
        let mut parts: Vec<ArrayPart<'ctx>> = Vec::with_capacity(elements.len());
        for elem in elements {
            if let Expr::Spread { expr: src, .. } = elem {
                parts.push(ArrayPart::Spread(self.generate_expr(src)?));
            } else {
                parts.push(ArrayPart::Inline(self.generate_expr(elem)?));
            }
        }

        self.build_array_from_parts(elem_llvm, &parts)
    }

    /// Lower `+` on arrays to a NEW GC-allocated array (neither operand mutated), in the
    /// three exact-type forms the checker dispatches (see `check_binop`):
    ///   concat:  `[]T + []T` — every element of `left` then of `right`.
    ///   append:  `[]T + T`   — every element of `left` then the single `right`.
    ///   prepend: `T + []T`   — the single `left` then every element of `right`.
    /// Each is `[<-left, <-right]` with the single-element side `Inline` instead of
    /// `Spread`, so it reuses the spread machinery (`build_array_from_parts`) — element-repr
    /// correct for `[]Num`, `[]Text`, and nested arrays via the type oracle. The
    /// concat-vs-append form is re-derived from the operands' oracle types using the SAME
    /// `types_match` the checker used (see `check_binop`), so the two sites cannot drift on
    /// what counts as "the same element type"; `[][]Num + []Num` is thus an append (`right`
    /// is one element), matching the checker.
    fn generate_array_concat(
        &mut self,
        left: &Expr,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use crate::typechecker::types_match;

        // Classify the form (which side is the whole array to splice vs. a single element)
        // and derive the element repr, all from borrowed oracle types — no `Type` clones.
        // This borrow is scoped so it ends before the `&mut self` `generate_expr` calls.
        let (elem_llvm, left_is_array, right_is_array) = {
            let (elem, left_is_array, right_is_array) =
                match (self.oracle.expr_type(left), self.oracle.expr_type(right)) {
                    // concat `[]T + []T`: both arrays of the SAME element type.
                    (Some(Type::Array(le)), Some(Type::Array(re))) if types_match(le, re) => {
                        (Some(&**le), true, true)
                    }
                    // append `[]T + T`: left is the array, right a single element.
                    (Some(Type::Array(le)), _) => (Some(&**le), true, false),
                    // prepend `T + []T`: right is the array, left a single element.
                    (_, Some(Type::Array(re))) => (Some(&**re), false, true),
                    // Unreachable via the routing guard in `generate_binop` (it only calls
                    // here when an operand's oracle type is `Array`). Defensive default.
                    _ => (None, true, true),
                };
            let elem_llvm = match elem {
                Some(t) => self.value_repr_type(t)?,
                None => self.context.f64_type().into(),
            };
            (elem_llvm, left_is_array, right_is_array)
        };

        let l = self.generate_expr(left)?;
        let r = self.generate_expr(right)?;
        let part = |is_array, v| {
            if is_array {
                ArrayPart::Spread(v)
            } else {
                ArrayPart::Inline(v)
            }
        };
        self.build_array_from_parts(
            elem_llvm,
            &[part(left_is_array, l), part(right_is_array, r)],
        )
    }

    /// Build a fresh `{ptr, size}` array by laying `parts` into a GC-allocated block:
    /// sum the parts' element counts (inline = 1, spread = its `.size`), allocate the
    /// exact backing store, then fill left-to-right — an inline element is stored at the
    /// running offset, a spread source is a flat `memcpy` of its data block. Works for
    /// any element repr (`[]Num`, `[]Text`, nested arrays) since element storage is POD
    /// in every case; `elem_llvm` supplies the stride. Shared by `<-` spread literals
    /// and `+` array concatenation.
    fn build_array_from_parts(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        parts: &[ArrayPart<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::types::BasicType;
        let i64_type = self.context.i64_type();
        let elem_size = elem_llvm
            .size_of()
            .ok_or_else(|| "array element type has no compile-time size".to_string())?;

        // Total element count: inline elements count 1 each, a spread counts its `.size`.
        let mut count = i64_type.const_zero();
        for part in parts {
            let add = match part {
                ArrayPart::Inline(_) => i64_type.const_int(1, false),
                ArrayPart::Spread(v) => self.array_size_field(*v)?,
            };
            count = self
                .builder
                .build_int_add(count, add, "concat_count")
                .map_err(|e| format!("Failed to sum array part count: {:?}", e))?;
        }

        // GC-allocate the exact `{ptr,size}` backing store (shared array helper).
        let data_ptr = self.alloc_array_data(elem_llvm, count)?;

        // Fill left-to-right, threading a running element offset.
        let memcpy_fn = self.get_intrinsic("memcpy")?;
        let mut offset = i64_type.const_zero();
        for part in parts {
            match part {
                ArrayPart::Inline(value) => {
                    let slot = unsafe {
                        self.builder
                            .build_gep(elem_llvm, data_ptr, &[offset], "concat_slot")
                            .map_err(|e| format!("Failed to index array slot: {:?}", e))?
                    };
                    self.builder
                        .build_store(slot, *value)
                        .map_err(|e| format!("Failed to store array element: {:?}", e))?;
                    offset = self
                        .builder
                        .build_int_add(offset, i64_type.const_int(1, false), "concat_off")
                        .map_err(|e| format!("Failed to advance array offset: {:?}", e))?;
                }
                ArrayPart::Spread(value) => {
                    let src_ptr = self.array_data_field(*value)?;
                    let src_size = self.array_size_field(*value)?;
                    let dest = unsafe {
                        self.builder
                            .build_gep(elem_llvm, data_ptr, &[offset], "concat_dest")
                            .map_err(|e| format!("Failed to index array dest: {:?}", e))?
                    };
                    let bytes = self
                        .builder
                        .build_int_mul(src_size, elem_size, "concat_src_bytes")
                        .map_err(|e| format!("Failed to size array copy: {:?}", e))?;
                    self.builder
                        .build_call(memcpy_fn, &[dest.into(), src_ptr.into(), bytes.into()], "")
                        .map_err(|e| format!("Failed to memcpy array source: {:?}", e))?;
                    offset = self
                        .builder
                        .build_int_add(offset, src_size, "concat_off")
                        .map_err(|e| format!("Failed to advance array offset: {:?}", e))?;
                }
            }
        }

        // Build the { ptr, size } array struct (the shared array/Text shape).
        self.array_struct(data_ptr, count)
    }

    /// Extract the data pointer (field 0) of an array `{ptr, size}` struct value.
    fn array_data_field(&self, array: BasicValueEnum<'ctx>) -> Result<PointerValue<'ctx>, String> {
        let s = array.into_struct_value();
        Ok(self
            .builder
            .build_extract_value(s, 0, "arr_data")
            .map_err(|e| format!("Failed to extract array data ptr: {:?}", e))?
            .into_pointer_value())
    }

    /// Extract the size (field 1, an i64) of an array `{ptr, size}` struct value.
    fn array_size_field(
        &self,
        array: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let s = array.into_struct_value();
        Ok(self
            .builder
            .build_extract_value(s, 1, "arr_size")
            .map_err(|e| format!("Failed to extract array size: {:?}", e))?
            .into_int_value())
    }

    /// Reorder a constructor call's `fields` into the named type's DECLARATION order so
    /// the lowered struct's slot order matches what `record_types` and the type oracle
    /// use to index fields. Falls back to the provided order if the type's field list
    /// isn't registered. (The expressions are cloned — constructor field lists are tiny.)
    fn constructor_fields_in_decl_order(
        &self,
        type_name: &str,
        fields: &[(String, Expr)],
    ) -> Vec<(String, Expr)> {
        let Some(decl_order) = self.named_type_fields.get(type_name) else {
            return fields.to_vec();
        };
        decl_order
            .iter()
            .filter_map(|fname| {
                fields
                    .iter()
                    .find(|(provided, _)| provided == fname)
                    .cloned()
            })
            .collect()
    }

    /// Materialize an inclusive range `lo <- hi` into a `[]Num` (the `{ptr, size}`
    /// array shape, same as `generate_array`). The element count is `|hi - lo| + 1`
    /// and the direction (ascending vs descending) is decided at runtime, since the
    /// ends can be dynamic: `lo <= hi` counts up (`1 <- 4` → `[1,2,3,4]`), otherwise
    /// down (`4 <- 1` → `[4,3,2,1]`). The backing storage is GC-allocated (`__alloc`)
    /// so the array may safely escape the current frame.
    fn generate_range(&mut self, start: &Expr, end: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Range must be in a function".to_string())?;

        let i64_type = self.context.i64_type();
        let f64_type = self.context.f64_type();

        // Evaluate both ends (Num = f64) and truncate to i64 endpoints.
        let lo_f = self.generate_expr(start)?.into_float_value();
        let hi_f = self.generate_expr(end)?.into_float_value();
        let lo = self
            .builder
            .build_float_to_signed_int(lo_f, i64_type, "range_lo")
            .map_err(|e| format!("Failed to convert range start: {:?}", e))?;
        let hi = self
            .builder
            .build_float_to_signed_int(hi_f, i64_type, "range_hi")
            .map_err(|e| format!("Failed to convert range end: {:?}", e))?;

        // Ascending iff lo <= hi; pick step = +1 / -1 and the inclusive span.
        let ascending = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLE, lo, hi, "range_asc")
            .map_err(|e| format!("Failed to compare range ends: {:?}", e))?;
        let one = i64_type.const_int(1, false);
        let neg_one = i64_type.const_all_ones(); // -1 in two's complement
        let step = self
            .builder
            .build_select(ascending, one, neg_one, "range_step")
            .map_err(|e| format!("Failed to select range step: {:?}", e))?
            .into_int_value();
        // |hi - lo| + 1: compute the signed delta once, then pick it or its
        // negation so the span is non-negative in either direction.
        let delta = self
            .builder
            .build_int_sub(hi, lo, "range_delta")
            .map_err(|e| format!("Failed to subtract range ends: {:?}", e))?;
        let neg_delta = self
            .builder
            .build_int_neg(delta, "range_neg_delta")
            .map_err(|e| format!("Failed to negate range delta: {:?}", e))?;
        let span_abs = self
            .builder
            .build_select(ascending, delta, neg_delta, "range_span")
            .map_err(|e| format!("Failed to select range span: {:?}", e))?
            .into_int_value();
        let count = self
            .builder
            .build_int_add(span_abs, one, "range_count")
            .map_err(|e| format!("Failed to add range count: {:?}", e))?;

        // GC-allocate count * sizeof(f64) bytes for the backing data.
        let eight = i64_type.const_int(8, false);
        let bytes = self
            .builder
            .build_int_mul(count, eight, "range_bytes")
            .map_err(|e| format!("Failed to size range alloc: {:?}", e))?;
        let alloc = self.get_intrinsic("__alloc")?;
        let alloc_call = self
            .builder
            .build_call(alloc, &[bytes.into()], "range_data")
            .map_err(|e| format!("Failed to allocate range: {:?}", e))?;
        let data_ptr = {
            use inkwell::values::AnyValue;
            alloc_call.as_any_value_enum().into_pointer_value()
        };

        // Fill loop: for i in 0..count: data[i] = (f64)(lo + i*step).
        let counter = self.create_entry_block_alloca("range_i", i64_type.into())?;
        self.builder
            .build_store(counter, i64_type.const_zero())
            .map_err(|e| format!("Failed to init range counter: {:?}", e))?;

        let header = self.context.append_basic_block(function, "range_header");
        let body = self.context.append_basic_block(function, "range_body");
        let exit = self.context.append_basic_block(function, "range_exit");

        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to branch to range header: {:?}", e))?;

        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64_type, counter, "i")
            .map_err(|e| format!("Failed to load range counter: {:?}", e))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, count, "range_cond")
            .map_err(|e| format!("Failed to build range condition: {:?}", e))?;
        self.builder
            .build_conditional_branch(cond, body, exit)
            .map_err(|e| format!("Failed to build range branch: {:?}", e))?;

        self.builder.position_at_end(body);
        // value = lo + i*step
        let i_step = self
            .builder
            .build_int_mul(i, step, "range_i_step")
            .map_err(|e| format!("Failed to scale range index: {:?}", e))?;
        let val_i = self
            .builder
            .build_int_add(lo, i_step, "range_val_i")
            .map_err(|e| format!("Failed to compute range element: {:?}", e))?;
        let val_f = self
            .builder
            .build_signed_int_to_float(val_i, f64_type, "range_val")
            .map_err(|e| format!("Failed to convert range element: {:?}", e))?;
        let elem_ptr = unsafe {
            self.builder
                .build_gep(f64_type, data_ptr, &[i], "range_elem")
                .map_err(|e| format!("Failed to index range data: {:?}", e))?
        };
        self.builder
            .build_store(elem_ptr, val_f)
            .map_err(|e| format!("Failed to store range element: {:?}", e))?;
        let next = self
            .builder
            .build_int_add(i, one, "range_next")
            .map_err(|e| format!("Failed to increment range counter: {:?}", e))?;
        self.builder
            .build_store(counter, next)
            .map_err(|e| format!("Failed to store range counter: {:?}", e))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to loop range: {:?}", e))?;

        // Build the { ptr, size } array struct (the shared array/Text shape).
        self.builder.position_at_end(exit);
        let array_struct_type = self.ptr_len_struct_type();
        let array_struct = self
            .builder
            .build_alloca(array_struct_type, "range_array")
            .map_err(|e| format!("Failed to allocate range struct: {:?}", e))?;
        let ptr_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 0, "range_ptr_field")
            .map_err(|e| format!("Failed to get range ptr field: {:?}", e))?;
        self.builder
            .build_store(ptr_field, data_ptr)
            .map_err(|e| format!("Failed to store range ptr: {:?}", e))?;
        let size_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 1, "range_size_field")
            .map_err(|e| format!("Failed to get range size field: {:?}", e))?;
        self.builder
            .build_store(size_field, count)
            .map_err(|e| format!("Failed to store range size: {:?}", e))?;
        self.builder
            .build_load(array_struct_type, array_struct, "range_array")
            .map_err(|e| format!("Failed to load range struct: {:?}", e))
    }

    /// Lower a built-in array method call (`map`/`filter`/`reduce`/`each`/`find`/`at`).
    /// `args[0]` is the receiver array; the rest are the method's arguments (a lambda
    /// for the higher-order forms, a `Num` index for `at`). A method's lambda argument is
    /// a deliberate inline specialization of the general lambda lowering: rather than
    /// emitting a closure value, its body is INLINED into the generated loop body per
    /// element (`inline_lambda`) — cheaper, and it sidesteps the unsupported
    /// higher-order-value path. The element LLVM type comes from the type oracle (the
    /// receiver's `[]elem` element type), so `[]Text`/`[]Num`/... all work.
    fn generate_array_method(
        &mut self,
        method: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let recv = &args[0];
        // Element type of the receiver array, from the oracle: `[]elem`.
        let elem_qty = match self.oracle.expr_type(recv) {
            Some(Type::Array(e)) => (**e).clone(),
            _ => return Err(format!("array method `{method}` on a non-array receiver")),
        };
        let elem_llvm = self.value_repr_type(&elem_qty)?;
        let (array_val, data_ptr, size) = self.extract_array(recv)?;

        match method {
            "map" => self.array_map(&args[1], &elem_qty, elem_llvm, data_ptr, size),
            "filter" => self.array_filter(&args[1], &elem_qty, elem_llvm, data_ptr, size),
            "reduce" => self.array_reduce(&args[1], &args[2], &elem_qty, elem_llvm, data_ptr, size),
            "each" => {
                self.array_each(&args[1], &elem_qty, elem_llvm, data_ptr, size)?;
                // Decision 19: a Unit-bodied method returns its receiver — `.each` yields
                // the array itself so it chains. Re-emit the (already-evaluated) struct.
                Ok(array_val)
            }
            "find" => self.array_find(&args[1], &elem_qty, elem_llvm, data_ptr, size),
            "at" => self.array_at(&args[1], elem_llvm, data_ptr, size),
            other => Err(format!("unknown array method `{other}`")),
        }
    }

    /// Lower a built-in `Text` method call (`args[0]` is the `Text` receiver). Each is
    /// lowered to its `quilon-rt` intrinsic; `split` yields the `[]Text` `{ptr,i64}`
    /// struct the intrinsic builds, and `indexOf` builds an `Ok(Num)`/`NotOk` `Result`.
    fn generate_text_method(
        &mut self,
        method: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        let (recv_ptr, recv_len) = self.extract_text(&args[0])?;

        // Call a struct-returning ({ptr,i64}) Text intrinsic with the given metadata args.
        let call_struct = |this: &mut Self,
                           intr: &str,
                           call_args: &[inkwell::values::BasicMetadataValueEnum<'ctx>]|
         -> Result<BasicValueEnum<'ctx>, String> {
            let f = this.get_intrinsic(intr)?;
            Ok(this
                .builder
                .build_call(f, call_args, "txt_m")
                .map_err(|e| format!("Failed to call {intr}: {:?}", e))?
                .as_any_value_enum()
                .into_struct_value()
                .into())
        };

        match method {
            "trim" => {
                // `trim` = `trimStart` then `trimEnd` (order-independent, identical to a
                // direct both-sides trim) — composed from the two intrinsics so there is
                // no separate `__text_trim`. The extra pass/allocation is fine for trim.
                let started = call_struct(
                    self,
                    "__text_trim_start",
                    &[recv_ptr.into(), recv_len.into()],
                )?
                .into_struct_value();
                let sp = self
                    .builder
                    .build_extract_value(started, 0, "trim_mid_ptr")
                    .map_err(|e| format!("Failed to extract trimStart ptr: {:?}", e))?
                    .into_pointer_value();
                let sl = self
                    .builder
                    .build_extract_value(started, 1, "trim_mid_len")
                    .map_err(|e| format!("Failed to extract trimStart len: {:?}", e))?
                    .into_int_value();
                call_struct(self, "__text_trim_end", &[sp.into(), sl.into()])
            }
            "trimStart" => call_struct(
                self,
                "__text_trim_start",
                &[recv_ptr.into(), recv_len.into()],
            ),
            "trimEnd" => call_struct(self, "__text_trim_end", &[recv_ptr.into(), recv_len.into()]),
            "toUpper" => call_struct(self, "__text_to_upper", &[recv_ptr.into(), recv_len.into()]),
            "toLower" => call_struct(self, "__text_to_lower", &[recv_ptr.into(), recv_len.into()]),
            "split" => {
                let (sp, sl) = self.extract_text(&args[1])?;
                call_struct(
                    self,
                    "__text_split",
                    &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                )
            }
            "replaceAll" => {
                // Replace every occurrence. The intrinsic aborts (exit 101) on an empty
                // `from`; there is no count.
                let (fp, fl) = self.extract_text(&args[1])?;
                let (tp, tl) = self.extract_text(&args[2])?;
                call_struct(
                    self,
                    "__text_replace_all",
                    &[
                        recv_ptr.into(),
                        recv_len.into(),
                        fp.into(),
                        fl.into(),
                        tp.into(),
                        tl.into(),
                    ],
                )
            }
            "replace" => {
                // Replace EXACTLY the first `count` occurrences. `count` is a Num,
                // truncated toward zero (as with array indices). The intrinsic aborts
                // (exit 101) on an empty `from`, count <= 0, or count > occurrences present
                // — a literal `count <= 0` / literal empty `from` / all-literal
                // count-exceeds were already rejected by the checker at compile time.
                let (fp, fl) = self.extract_text(&args[1])?;
                let (tp, tl) = self.extract_text(&args[2])?;
                let count = self.text_index_arg(&args[3], "replace_count")?;
                call_struct(
                    self,
                    "__text_replace_n",
                    &[
                        recv_ptr.into(),
                        recv_len.into(),
                        fp.into(),
                        fl.into(),
                        tp.into(),
                        tl.into(),
                        count.into(),
                    ],
                )
            }
            "slice" => {
                let start = self.text_index_arg(&args[1], "slice_start")?;
                let end = self.text_index_arg(&args[2], "slice_end")?;
                call_struct(
                    self,
                    "__text_slice",
                    &[recv_ptr.into(), recv_len.into(), start.into(), end.into()],
                )
            }
            "contains" => {
                let (sp, sl) = self.extract_text(&args[1])?;
                let f = self.get_intrinsic("__text_contains")?;
                let r = self
                    .builder
                    .build_call(
                        f,
                        &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                        "txt_contains",
                    )
                    .map_err(|e| format!("Failed to call __text_contains: {:?}", e))?
                    .as_any_value_enum()
                    .into_int_value();
                // i64 0/1 -> i1 Bool.
                Ok(self
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        r,
                        r.get_type().const_zero(),
                        "contains_bool",
                    )
                    .map_err(|e| format!("Failed to build contains bool: {:?}", e))?
                    .into())
            }
            "indexOf" => self.generate_text_index_of(recv_ptr, recv_len, &args[1]),
            other => Err(format!("unknown text method `{other}`")),
        }
    }

    /// Evaluate a `Text` expression and split it into its `(data_ptr, byte_len)` fields —
    /// the shared primitive for lowering Text-method calls, whose intrinsics take a `Text`
    /// as its two fields (mirrors the extraction in `generate_text_compare`).
    fn extract_text(
        &mut self,
        expr: &Expr,
    ) -> Result<(PointerValue<'ctx>, inkwell::values::IntValue<'ctx>), String> {
        let val = self.generate_expr(expr)?;
        let BasicValueEnum::StructValue(s) = val else {
            return Err("Text method receiver/argument must be a Text value".to_string());
        };
        let ptr = self
            .builder
            .build_extract_value(s, 0, "txt_ptr")
            .map_err(|e| format!("Failed to extract text ptr: {:?}", e))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(s, 1, "txt_len")
            .map_err(|e| format!("Failed to extract text len: {:?}", e))?
            .into_int_value();
        Ok((ptr, len))
    }

    /// Evaluate a `Num` index argument (an `f64`) and convert it to the `i64` the Text
    /// intrinsics take (used by `slice`'s start/end).
    fn text_index_arg(
        &mut self,
        expr: &Expr,
        name: &str,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let f = self.generate_expr(expr)?.into_float_value();
        self.builder
            .build_float_to_signed_int(f, self.context.i64_type(), name)
            .map_err(|e| format!("Failed to convert text index: {:?}", e))
    }

    /// Lower `Text.indexOf(sub)`: call `__text_index_of` (grapheme index or -1) and turn
    /// the result into a `Result` — `Ok(Num idx)` when >= 0, else `NotOk` — using the
    /// same `{ i8 tag, f64 }` shape `array_at`/`array_find` produce (no -1 sentinel).
    fn generate_text_index_of(
        &mut self,
        recv_ptr: PointerValue<'ctx>,
        recv_len: inkwell::values::IntValue<'ctx>,
        sub: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use inkwell::values::AnyValue;
        let (sp, sl) = self.extract_text(sub)?;
        let f = self.get_intrinsic("__text_index_of")?;
        let idx = self
            .builder
            .build_call(
                f,
                &[recv_ptr.into(), recv_len.into(), sp.into(), sl.into()],
                "txt_index_of",
            )
            .map_err(|e| format!("Failed to call __text_index_of: {:?}", e))?
            .as_any_value_enum()
            .into_int_value();

        let i64t = self.context.i64_type();
        let found = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::SGE,
                idx,
                i64t.const_zero(),
                "idx_found",
            )
            .map_err(|e| format!("Failed to compare index: {:?}", e))?;

        // No branch needed: the Ok payload (`idx` widened to f64) is safe to compute
        // unconditionally, so build both Results eagerly and `select` on `found`.
        let elem_llvm: BasicTypeEnum = self.context.f64_type().into();
        let idx_f = self
            .builder
            .build_signed_int_to_float(idx, self.context.f64_type(), "idx_as_num")
            .map_err(|e| format!("Failed to convert index to num: {:?}", e))?;
        let ok = self.build_result(elem_llvm, "Ok", idx_f.into());
        let no = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm));
        self.builder
            .build_select(found, ok, no, "idx_value")
            .map_err(|e| format!("Failed to select indexOf result: {:?}", e))
    }

    /// Evaluate an array expression and break it into `(struct_value, data_ptr, size_i64)`.
    /// The array ABI is the shared `{ ptr data, i64 size }` struct; this stores it to a
    /// temporary alloca to GEP out the two fields.
    fn extract_array(
        &mut self,
        array_expr: &Expr,
    ) -> Result<
        (
            BasicValueEnum<'ctx>,
            PointerValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ),
        String,
    > {
        let array_val = self.generate_expr(array_expr)?;
        let struct_ty = self.ptr_len_struct_type();
        let alloca = self.create_entry_block_alloca("am_array", struct_ty.into())?;
        self.builder
            .build_store(alloca, array_val)
            .map_err(|e| format!("Failed to store array: {:?}", e))?;
        let data_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "am_data_field")
            .map_err(|e| format!("Failed to GEP data field: {:?}", e))?;
        let data_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                data_field,
                "am_data",
            )
            .map_err(|e| format!("Failed to load data ptr: {:?}", e))?
            .into_pointer_value();
        let size_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 1, "am_size_field")
            .map_err(|e| format!("Failed to GEP size field: {:?}", e))?;
        let size = self
            .builder
            .build_load(self.context.i64_type(), size_field, "am_size")
            .map_err(|e| format!("Failed to load size: {:?}", e))?
            .into_int_value();
        Ok((array_val, data_ptr, size))
    }

    /// GC-allocate a `{ ptr, size }` array of `count` elements of `elem_llvm`, returning
    /// the data pointer. The caller fills it, then builds the struct via `array_struct`.
    fn alloc_array_data(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        count: inkwell::values::IntValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let elem_size = elem_llvm
            .size_of()
            .ok_or_else(|| "array element type has no compile-time size".to_string())?;
        let bytes = self
            .builder
            .build_int_mul(count, elem_size, "am_bytes")
            .map_err(|e| format!("Failed to size array alloc: {:?}", e))?;
        let alloc = self.get_intrinsic("__alloc")?;
        use inkwell::values::AnyValue;
        Ok(self
            .builder
            .build_call(alloc, &[bytes.into()], "am_alloc")
            .map_err(|e| format!("Failed to allocate array: {:?}", e))?
            .as_any_value_enum()
            .into_pointer_value())
    }

    /// Build the `{ ptr, i64 }` array struct value from a data pointer and element count.
    fn array_struct(
        &mut self,
        data_ptr: PointerValue<'ctx>,
        count: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let struct_ty = self.ptr_len_struct_type();
        let alloca = self.create_entry_block_alloca("am_out", struct_ty.into())?;
        let ptr_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "am_out_ptr")
            .map_err(|e| format!("Failed to GEP out ptr: {:?}", e))?;
        self.builder
            .build_store(ptr_field, data_ptr)
            .map_err(|e| format!("Failed to store out ptr: {:?}", e))?;
        let size_field = self
            .builder
            .build_struct_gep(struct_ty, alloca, 1, "am_out_size")
            .map_err(|e| format!("Failed to GEP out size: {:?}", e))?;
        self.builder
            .build_store(size_field, count)
            .map_err(|e| format!("Failed to store out size: {:?}", e))?;
        self.builder
            .build_load(struct_ty, alloca, "am_out")
            .map_err(|e| format!("Failed to load out struct: {:?}", e))
    }

    /// Load `data_ptr[i]` as a value of `elem_llvm` (the array element representation).
    fn load_element(
        &mut self,
        data_ptr: PointerValue<'ctx>,
        elem_llvm: BasicTypeEnum<'ctx>,
        i: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr = unsafe {
            self.builder
                .build_gep(elem_llvm, data_ptr, &[i], "am_elem_ptr")
                .map_err(|e| format!("Failed to GEP element: {:?}", e))?
        };
        self.builder
            .build_load(elem_llvm, ptr, "am_elem")
            .map_err(|e| format!("Failed to load element: {:?}", e))
    }

    /// Inline a lambda body with its parameters bound to `arg_values`. An array method's
    /// lambda is lowered inline (not as a closure value): each parameter is bound to a
    /// freshly-stored value (an alloca, like a loop variable) and the body is emitted in
    /// the current block. Saves/restores any shadowed bindings of the same names, so an
    /// inline never leaks the parameter binding past its use (and nesting is safe).
    /// `arg_values` carries each argument's Quilon type for overload mangling in the body.
    fn inline_lambda(
        &mut self,
        lambda: &Expr,
        arg_values: &[(BasicValueEnum<'ctx>, Type)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Expr::Lambda { params, body, .. } = lambda else {
            return Err("array method expects a lambda argument".to_string());
        };
        if params.len() != arg_values.len() {
            return Err(format!(
                "lambda expects {} parameter(s), got {} argument(s)",
                params.len(),
                arg_values.len()
            ));
        }
        // Save shadowed bindings to restore after inlining.
        let mut saved: Vec<SavedBinding<'ctx>> = Vec::with_capacity(params.len());
        for (param, (value, qty)) in params.iter().zip(arg_values) {
            let alloca = self.create_entry_block_alloca(&param.name, value.get_type())?;
            self.builder
                .build_store(alloca, *value)
                .map_err(|e| format!("Failed to store lambda param: {:?}", e))?;
            saved.push((
                param.name.clone(),
                self.variables.get(&param.name).copied(),
                self.var_types.get(&param.name).cloned(),
            ));
            self.variables
                .insert(param.name.clone(), (alloca, value.get_type()));
            self.var_types.insert(param.name.clone(), qty.clone());
        }
        let result = self.generate_expr(body);
        // Restore shadowed bindings.
        for (name, prev_var, prev_ty) in saved {
            match prev_var {
                Some(v) => {
                    self.variables.insert(name.clone(), v);
                }
                None => {
                    self.variables.remove(&name);
                }
            }
            match prev_ty {
                Some(t) => {
                    self.var_types.insert(name, t);
                }
                None => {
                    self.var_types.remove(&name);
                }
            }
        }
        result
    }

    /// `arr.map(f)` — a new array whose element `i` is `f(arr[i])`. The result element
    /// type is the lambda body's type (from the oracle), so `map` may change the element
    /// type (e.g. `[]Num -> []Text`).
    fn array_map(
        &mut self,
        lambda: &Expr,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_llvm = match self.lambda_body_repr(lambda) {
            Some(r) => r?,
            None => elem_llvm,
        };
        let out_ptr = self.alloc_array_data(result_llvm, size)?;
        self.array_loop(size, |this, i| {
            let elem = this.load_element(data_ptr, elem_llvm, i)?;
            let mapped = this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            let dst = unsafe {
                this.builder
                    .build_gep(result_llvm, out_ptr, &[i], "map_dst")
                    .map_err(|e| format!("Failed to GEP map dst: {:?}", e))?
            };
            this.builder
                .build_store(dst, mapped)
                .map_err(|e| format!("Failed to store mapped: {:?}", e))?;
            Ok(())
        })?;
        self.array_struct(out_ptr, size)
    }

    /// `arr.filter(pred)` — a new array of the elements for which `pred(elem)` is true,
    /// in order. The output buffer is sized to the input (worst case, all kept); the
    /// result struct reports the actual kept count.
    fn array_filter(
        &mut self,
        lambda: &Expr,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i64t = self.context.i64_type();
        let out_ptr = self.alloc_array_data(elem_llvm, size)?;
        let count_ptr = self.create_entry_block_alloca("filter_count", i64t.into())?;
        self.builder
            .build_store(count_ptr, i64t.const_zero())
            .map_err(|e| format!("Failed to init filter count: {:?}", e))?;
        self.array_loop(size, |this, i| {
            let elem = this.load_element(data_ptr, elem_llvm, i)?;
            let keep = this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            let keep_bool = this.value_to_boolean(keep)?;
            let function = this.current_function.unwrap();
            let keep_bb = this.context.append_basic_block(function, "filter_keep");
            let cont_bb = this.context.append_basic_block(function, "filter_cont");
            this.builder
                .build_conditional_branch(keep_bool, keep_bb, cont_bb)
                .map_err(|e| format!("Failed to branch filter: {:?}", e))?;
            this.builder.position_at_end(keep_bb);
            let count = this
                .builder
                .build_load(i64t, count_ptr, "filter_n")
                .map_err(|e| format!("Failed to load filter count: {:?}", e))?
                .into_int_value();
            let dst = unsafe {
                this.builder
                    .build_gep(elem_llvm, out_ptr, &[count], "filter_dst")
                    .map_err(|e| format!("Failed to GEP filter dst: {:?}", e))?
            };
            this.builder
                .build_store(dst, elem)
                .map_err(|e| format!("Failed to store kept: {:?}", e))?;
            let next = this
                .builder
                .build_int_add(count, i64t.const_int(1, false), "filter_next")
                .map_err(|e| format!("Failed to inc filter count: {:?}", e))?;
            this.builder
                .build_store(count_ptr, next)
                .map_err(|e| format!("Failed to store filter count: {:?}", e))?;
            this.builder
                .build_unconditional_branch(cont_bb)
                .map_err(|e| format!("Failed to branch filter cont: {:?}", e))?;
            this.builder.position_at_end(cont_bb);
            Ok(())
        })?;
        let count = self
            .builder
            .build_load(i64t, count_ptr, "filter_total")
            .map_err(|e| format!("Failed to load filter total: {:?}", e))?
            .into_int_value();
        self.array_struct(out_ptr, count)
    }

    /// `arr.reduce(init, (acc, x) => ...)` — fold left, threading `acc` (initialized to
    /// `init`) through the lambda for each element. The result is the final accumulator.
    fn array_reduce(
        &mut self,
        init: &Expr,
        lambda: &Expr,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let init_val = self.generate_expr(init)?;
        let acc_qty = self.infer_type(init);
        let acc_ptr = self.create_entry_block_alloca("reduce_acc", init_val.get_type())?;
        self.builder
            .build_store(acc_ptr, init_val)
            .map_err(|e| format!("Failed to init acc: {:?}", e))?;
        let acc_llvm = init_val.get_type();
        self.array_loop(size, |this, i| {
            let elem = this.load_element(data_ptr, elem_llvm, i)?;
            let acc = this
                .builder
                .build_load(acc_llvm, acc_ptr, "reduce_load")
                .map_err(|e| format!("Failed to load acc: {:?}", e))?;
            let next =
                this.inline_lambda(lambda, &[(acc, acc_qty.clone()), (elem, elem_qty.clone())])?;
            this.builder
                .build_store(acc_ptr, next)
                .map_err(|e| format!("Failed to store acc: {:?}", e))?;
            Ok(())
        })?;
        self.builder
            .build_load(acc_llvm, acc_ptr, "reduce_result")
            .map_err(|e| format!("Failed to load reduce result: {:?}", e))
    }

    /// `arr.each(f)` — run `f` on every element for side effects; the result is ignored
    /// (the receiver is returned by the caller).
    fn array_each(
        &mut self,
        lambda: &Expr,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<(), String> {
        self.array_loop(size, |this, i| {
            let elem = this.load_element(data_ptr, elem_llvm, i)?;
            this.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
            Ok(())
        })?;
        Ok(())
    }

    /// `arr.find(pred)` — `Ok(elem)` for the first element satisfying `pred`, else
    /// `NotOk($)`. Both arms produce the SAME `{ i8 tag, elem }` struct so the result
    /// has one type; the `NotOk` payload slot is zeroed (never read).
    fn array_find(
        &mut self,
        lambda: &Expr,
        elem_qty: &Type,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let result_ty = self.result_struct_type(elem_llvm);
        let result_ptr = self.create_entry_block_alloca("find_result", result_ty.into())?;
        // Default: NotOk (tag 1, zeroed payload).
        let not_ok = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm));
        self.builder
            .build_store(result_ptr, not_ok)
            .map_err(|e| format!("Failed to init find result: {:?}", e))?;

        let function = self.current_function.unwrap();
        let done_bb = self.context.append_basic_block(function, "find_done");

        // Loop with an early exit to `done_bb` on the first match.
        let i64t = self.context.i64_type();
        let counter = self.create_entry_block_alloca("find_i", i64t.into())?;
        self.builder
            .build_store(counter, i64t.const_zero())
            .map_err(|e| format!("Failed to init find counter: {:?}", e))?;
        let header = self.context.append_basic_block(function, "find_header");
        let body = self.context.append_basic_block(function, "find_body");
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to branch find header: {:?}", e))?;
        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64t, counter, "find_iv")
            .map_err(|e| format!("Failed to load find counter: {:?}", e))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, size, "find_cond")
            .map_err(|e| format!("Failed to compare find counter: {:?}", e))?;
        self.builder
            .build_conditional_branch(cond, body, done_bb)
            .map_err(|e| format!("Failed to branch find body: {:?}", e))?;
        self.builder.position_at_end(body);
        let elem = self.load_element(data_ptr, elem_llvm, i)?;
        let matched = self.inline_lambda(lambda, &[(elem, elem_qty.clone())])?;
        let matched_bool = self.value_to_boolean(matched)?;
        let found_bb = self.context.append_basic_block(function, "find_found");
        let next_bb = self.context.append_basic_block(function, "find_next");
        self.builder
            .build_conditional_branch(matched_bool, found_bb, next_bb)
            .map_err(|e| format!("Failed to branch find match: {:?}", e))?;
        // Found: store Ok(elem) and jump to done. `body` dominates `found_bb`, so the
        // `elem` already loaded above is in scope here — no need to reload it.
        self.builder.position_at_end(found_bb);
        let ok = self.build_result(elem_llvm, "Ok", elem);
        self.builder
            .build_store(result_ptr, ok)
            .map_err(|e| format!("Failed to store find Ok: {:?}", e))?;
        self.builder
            .build_unconditional_branch(done_bb)
            .map_err(|e| format!("Failed to branch find done: {:?}", e))?;
        // Next iteration.
        self.builder.position_at_end(next_bb);
        let inc = self
            .builder
            .build_int_add(i, i64t.const_int(1, false), "find_inc")
            .map_err(|e| format!("Failed to inc find counter: {:?}", e))?;
        self.builder
            .build_store(counter, inc)
            .map_err(|e| format!("Failed to store find counter: {:?}", e))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to loop find: {:?}", e))?;

        self.builder.position_at_end(done_bb);
        self.builder
            .build_load(result_ty, result_ptr, "find_value")
            .map_err(|e| format!("Failed to load find result: {:?}", e))
    }

    /// `arr.at(n)` — `Ok(arr[n])` if `0 <= n < size`, else `NotOk($)` (safe index).
    fn array_at(
        &mut self,
        index: &Expr,
        elem_llvm: BasicTypeEnum<'ctx>,
        data_ptr: PointerValue<'ctx>,
        size: inkwell::values::IntValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i64t = self.context.i64_type();
        let idx_f = self.generate_expr(index)?.into_float_value();
        let idx = self
            .builder
            .build_float_to_signed_int(idx_f, i64t, "at_idx")
            .map_err(|e| format!("Failed to convert at index: {:?}", e))?;
        // In bounds iff 0 <= idx < size.
        let ge0 = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SGE, idx, i64t.const_zero(), "at_ge0")
            .map_err(|e| format!("Failed to compare at lower bound: {:?}", e))?;
        let lt = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, idx, size, "at_lt")
            .map_err(|e| format!("Failed to compare at upper bound: {:?}", e))?;
        let in_bounds = self
            .builder
            .build_and(ge0, lt, "at_in_bounds")
            .map_err(|e| format!("Failed to and at bounds: {:?}", e))?;

        let result_ty = self.result_struct_type(elem_llvm);
        let result_ptr = self.create_entry_block_alloca("at_result", result_ty.into())?;
        let function = self.current_function.unwrap();
        let ok_bb = self.context.append_basic_block(function, "at_ok");
        let no_bb = self.context.append_basic_block(function, "at_no");
        let cont_bb = self.context.append_basic_block(function, "at_cont");
        self.builder
            .build_conditional_branch(in_bounds, ok_bb, no_bb)
            .map_err(|e| format!("Failed to branch at bounds: {:?}", e))?;
        self.builder.position_at_end(ok_bb);
        let elem = self.load_element(data_ptr, elem_llvm, idx)?;
        let ok = self.build_result(elem_llvm, "Ok", elem);
        self.builder
            .build_store(result_ptr, ok)
            .map_err(|e| format!("Failed to store at Ok: {:?}", e))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(|e| format!("Failed to branch at ok cont: {:?}", e))?;
        self.builder.position_at_end(no_bb);
        let no = self.build_result(elem_llvm, "NotOk", zeroed(elem_llvm));
        self.builder
            .build_store(result_ptr, no)
            .map_err(|e| format!("Failed to store at NotOk: {:?}", e))?;
        self.builder
            .build_unconditional_branch(cont_bb)
            .map_err(|e| format!("Failed to branch at no cont: {:?}", e))?;
        self.builder.position_at_end(cont_bb);
        self.builder
            .build_load(result_ty, result_ptr, "at_value")
            .map_err(|e| format!("Failed to load at result: {:?}", e))
    }

    /// The `{ i8 tag, elem }` struct that `find`/`at` return — a per-element-typed
    /// `Result` whose single payload slot holds the element (matching the Result-style
    /// per-value layout the pattern-match consumer extracts from field 1).
    fn result_struct_type(
        &self,
        elem_llvm: BasicTypeEnum<'ctx>,
    ) -> inkwell::types::StructType<'ctx> {
        self.context
            .struct_type(&[self.context.i8_type().into(), elem_llvm], false)
    }

    /// Build the `{ i8 tag, payload }` value that `find`/`at` return, tagged as Result
    /// variant `variant` (`"Ok"` / `"NotOk"`). The tag number is read from the shared
    /// sum-variant registry (`register_builtin_sum_types`) — the same source the
    /// pattern-match consumer uses — so construction and matching can never drift apart.
    fn build_result(
        &mut self,
        elem_llvm: BasicTypeEnum<'ctx>,
        variant: &str,
        payload: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let tag = self
            .sum_variants
            .get(variant)
            .map(|(t, _)| *t)
            .unwrap_or_else(|| panic!("Result variant `{variant}` is not registered"));
        let struct_ty = self.result_struct_type(elem_llvm);
        let tag_val = self.context.i8_type().const_int(tag as u64, false);
        let mut agg = struct_ty.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "res_tag")
            .expect("insert result tag")
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, payload, 1, "res_payload")
            .expect("insert result payload")
            .into_struct_value();
        agg.into()
    }

    /// The LLVM value-representation type of a lambda's body (its inferred result type,
    /// from the oracle), if known — used by `map` to size the output array. `None` when
    /// the oracle has no entry (IR-only tests), so the caller falls back.
    fn lambda_body_repr(&self, lambda: &Expr) -> Option<Result<BasicTypeEnum<'ctx>, String>> {
        let Expr::Lambda { body, .. } = lambda else {
            return None;
        };
        self.oracle.expr_type(body).map(|t| self.value_repr_type(t))
    }

    /// Emit a counted `for i in 0..size` loop, calling `body(self, i)` in the loop body
    /// (the builder is positioned in the body block). On return the builder sits in the
    /// loop's exit block. Shared scaffolding for the array methods that visit every
    /// element in order (`map`/`filter`/`reduce`/`each`). `find` rolls its own loop (it
    /// needs an early exit).
    fn array_loop(
        &mut self,
        size: inkwell::values::IntValue<'ctx>,
        mut body: impl FnMut(&mut Self, inkwell::values::IntValue<'ctx>) -> Result<(), String>,
    ) -> Result<(), String> {
        let i64t = self.context.i64_type();
        let function = self.current_function.unwrap();
        let counter = self.create_entry_block_alloca("am_i", i64t.into())?;
        self.builder
            .build_store(counter, i64t.const_zero())
            .map_err(|e| format!("Failed to init loop counter: {:?}", e))?;
        let header = self.context.append_basic_block(function, "am_header");
        let body_bb = self.context.append_basic_block(function, "am_body");
        let exit = self.context.append_basic_block(function, "am_exit");
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to branch loop header: {:?}", e))?;
        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64t, counter, "am_iv")
            .map_err(|e| format!("Failed to load loop counter: {:?}", e))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, size, "am_cond")
            .map_err(|e| format!("Failed to compare loop counter: {:?}", e))?;
        self.builder
            .build_conditional_branch(cond, body_bb, exit)
            .map_err(|e| format!("Failed to branch loop body: {:?}", e))?;
        self.builder.position_at_end(body_bb);
        body(self, i)?;
        let inc = self
            .builder
            .build_int_add(i, i64t.const_int(1, false), "am_inc")
            .map_err(|e| format!("Failed to inc loop counter: {:?}", e))?;
        self.builder
            .build_store(counter, inc)
            .map_err(|e| format!("Failed to store loop counter: {:?}", e))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to loop: {:?}", e))?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// Lower a record literal, routing a functional-update literal (`{<-p, x = 9}`,
    /// containing one or more `<-` spreads) to [`generate_record_update`] and an ordinary
    /// literal to [`generate_record`].
    fn generate_record_expr(
        &mut self,
        record_expr: &Expr,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if fields.iter().any(|(_, v)| matches!(v, Expr::Spread { .. })) {
            self.generate_record_update(record_expr, fields)
        } else {
            self.generate_record(fields)
        }
    }

    /// Lower a record functional-update `{<-p, x = 9, ...}`: build a NEW record whose
    /// field set / order / types come from the whole literal's oracle type (a `Named`
    /// type keeps its declared layout and methods; otherwise it is an anonymous record).
    /// Each result field's value is the explicit override if the literal supplies one
    /// (`x = 9`), else the field copied from the LAST spread source that carries it —
    /// so later entries override earlier ones, left-to-right.
    fn generate_record_update(
        &mut self,
        record_expr: &Expr,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if self.current_function.is_none() {
            return Err("Global records not yet implemented".to_string());
        }

        // Result layout (ordered fields + types) from the oracle — authoritative for both
        // the struct shape and which slot each name occupies.
        let result_fields: Vec<(String, Type)> = match self.oracle.expr_type(record_expr) {
            Some(Type::Named { fields, .. }) | Some(Type::Record(fields)) => fields.clone(),
            _ => {
                return Err(
                    "record functional-update requires type information (missing oracle entry)"
                        .to_string(),
                );
            }
        };

        // Evaluate the literal's parts in source order (left-to-right), recording for each
        // field name its LATEST provider — so precedence follows source order exactly:
        // a later entry (override OR spread) beats an earlier one, an override beats an
        // earlier spread, and a later spread beats an earlier override. (Splitting on
        // "override vs spread" instead would wrongly make an explicit field always win
        // regardless of position, e.g. `{x = 9, <-p}` must yield `p.x`, not `9`.)
        enum Provider<'v> {
            Override(BasicValueEnum<'v>),
            Spread(usize), // index into `sources`
        }
        struct Source<'v> {
            ptr: PointerValue<'v>,
            layout: Vec<(String, Type)>,
            // The source record's LLVM struct type, reconstructed once here (not per
            // field copied from it) so field GEPs just index it.
            struct_type: inkwell::types::StructType<'v>,
        }
        let mut sources: Vec<Source<'ctx>> = Vec::new();
        let mut provider: HashMap<String, Provider<'ctx>> = HashMap::new();

        for (name, value) in fields {
            if let Expr::Spread { expr: src, .. } = value {
                let layout: Vec<(String, Type)> = match self.oracle.expr_type(src) {
                    Some(Type::Named { fields, .. }) | Some(Type::Record(fields)) => fields.clone(),
                    _ => {
                        return Err("record spread source requires type information".to_string());
                    }
                };
                let fnames: Vec<String> = layout.iter().map(|(n, _)| n.clone()).collect();
                let struct_type = self.record_struct_type(&layout)?;
                let ptr = self.generate_expr(src)?.into_pointer_value();
                let idx = sources.len();
                sources.push(Source {
                    ptr,
                    layout,
                    struct_type,
                });
                for fname in fnames {
                    provider.insert(fname, Provider::Spread(idx));
                }
            } else {
                let v = self.generate_expr(value)?;
                provider.insert(name.clone(), Provider::Override(v));
            }
        }

        // Result field repr types, computed once — reused both to load copied fields and
        // to build the result struct (matching how `record_field_pointer` reconstructs it
        // later). The struct is GC-allocated so it may escape the frame.
        let field_types: Vec<BasicTypeEnum> = result_fields
            .iter()
            .map(|(_, t)| self.value_repr_type(t))
            .collect::<Result<Vec<_>, _>>()?;

        // Assemble each result field's value in result (slot) order.
        let mut field_values: Vec<BasicValueEnum<'ctx>> = Vec::with_capacity(result_fields.len());
        for (i, (fname, _)) in result_fields.iter().enumerate() {
            let field_llvm = field_types[i];
            match provider.get(fname) {
                Some(Provider::Override(v)) => field_values.push(*v),
                Some(Provider::Spread(si)) => {
                    // Copy the field out of its providing spread source.
                    let src = &sources[*si];
                    let idx = src
                        .layout
                        .iter()
                        .position(|(n, _)| n == fname)
                        .ok_or_else(|| format!("spread source missing field {}", fname))?;
                    let gep = self
                        .builder
                        .build_struct_gep(src.struct_type, src.ptr, idx as u32, "spread_field_ptr")
                        .map_err(|e| format!("Failed to GEP spread field: {:?}", e))?;
                    let loaded = self
                        .builder
                        .build_load(field_llvm, gep, fname)
                        .map_err(|e| format!("Failed to load spread field: {:?}", e))?;
                    field_values.push(loaded);
                }
                None => {
                    return Err(format!(
                        "record functional-update result field {fname} has no source"
                    ));
                }
            }
        }

        let struct_type = self.context.struct_type(&field_types, false);
        use inkwell::values::AnyValue;
        let size = struct_type
            .size_of()
            .ok_or_else(|| "record struct type has no compile-time size".to_string())?;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let record_ptr = self
            .builder
            .build_call(alloc_fn, &[size.into()], "record")
            .map_err(|e| format!("Failed to call __alloc for record: {:?}", e))?
            .as_any_value_enum()
            .into_pointer_value();
        for (i, value) in field_values.iter().enumerate() {
            let gep = self
                .builder
                .build_struct_gep(struct_type, record_ptr, i as u32, &format!("field_{}", i))
                .map_err(|e| format!("Failed to build GEP: {:?}", e))?;
            self.builder
                .build_store(gep, *value)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
        }
        Ok(record_ptr.into())
    }

    fn generate_record(
        &mut self,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if fields.is_empty() {
            // Empty record - create empty struct
            let struct_type = self.context.struct_type(&[], false);
            return Ok(struct_type.const_zero().into());
        }

        // Generate all field values
        let mut field_values: Vec<BasicValueEnum> = Vec::new();
        for (_name, expr) in fields {
            field_values.push(self.generate_expr(expr)?);
        }

        // Get field types
        let field_types: Vec<BasicTypeEnum> = field_values.iter().map(|v| v.get_type()).collect();

        // Create struct type
        let struct_type = self.context.struct_type(&field_types, false);

        // Create the struct value
        if self.current_function.is_some() {
            // GC-allocate the struct (not a stack alloca) so a record VALUE can outlive
            // the frame that built it — e.g. a record returned from a function or a user
            // operator overload (`+ = (a :: Vec, b :: Vec) -> Vec => Vec { ... }`). A
            // stack alloca would dangle once the callee returned.
            use inkwell::values::AnyValue;
            let size = struct_type
                .size_of()
                .ok_or_else(|| "record struct type has no compile-time size".to_string())?;
            let alloc_fn = self.get_intrinsic("__alloc")?;
            let record_ptr = self
                .builder
                .build_call(alloc_fn, &[size.into()], "record")
                .map_err(|e| format!("Failed to call __alloc for record: {:?}", e))?
                .as_any_value_enum()
                .into_pointer_value();

            // Store each field
            for (i, value) in field_values.iter().enumerate() {
                let gep = self
                    .builder
                    .build_struct_gep(struct_type, record_ptr, i as u32, &format!("field_{}", i))
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?;
                self.builder
                    .build_store(gep, *value)
                    .map_err(|e| format!("Failed to build store: {:?}", e))?;
            }

            Ok(record_ptr.into())
        } else {
            // For globals, we need constant values
            Err("Global records not yet implemented".to_string())
        }
    }

    /// Concatenate two `Text` values into a fresh, GC-allocated, NUL-terminated
    /// buffer and return a new `{ ptr, byte_len }` struct.
    fn generate_text_concat(
        &mut self,
        left: inkwell::values::StructValue<'ctx>,
        right: inkwell::values::StructValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i8t = self.context.i8_type();
        let i64t = self.context.i64_type();

        let field = |s: inkwell::values::StructValue<'ctx>,
                     idx: u32,
                     name: &str|
         -> Result<BasicValueEnum<'ctx>, String> {
            self.builder
                .build_extract_value(s, idx, name)
                .map_err(|e| format!("Failed to extract text field: {:?}", e))
        };
        let l_ptr = field(left, 0, "l_ptr")?.into_pointer_value();
        let l_len = field(left, 1, "l_len")?.into_int_value();
        let r_ptr = field(right, 0, "r_ptr")?.into_pointer_value();
        let r_len = field(right, 1, "r_len")?.into_int_value();

        let total = self
            .builder
            .build_int_add(l_len, r_len, "concat_len")
            .map_err(|e| format!("Failed to add lengths: {:?}", e))?;
        // +1 byte for the NUL terminator so the result is also a valid C string.
        let alloc_size = self
            .builder
            .build_int_add(total, i64t.const_int(1, false), "concat_alloc")
            .map_err(|e| format!("Failed to size alloc: {:?}", e))?;

        use inkwell::values::AnyValue;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let dest = self
            .builder
            .build_call(alloc_fn, &[alloc_size.into()], "concat_buf")
            .map_err(|e| format!("Failed to call __alloc: {:?}", e))?
            .as_any_value_enum()
            .into_pointer_value();

        let memcpy_fn = self.get_intrinsic("memcpy")?;
        self.builder
            .build_call(memcpy_fn, &[dest.into(), l_ptr.into(), l_len.into()], "")
            .map_err(|e| format!("Failed to copy left text: {:?}", e))?;
        let tail = unsafe {
            self.builder
                .build_gep(i8t, dest, &[l_len], "concat_tail")
                .map_err(|e| format!("Failed to offset into buffer: {:?}", e))?
        };
        self.builder
            .build_call(memcpy_fn, &[tail.into(), r_ptr.into(), r_len.into()], "")
            .map_err(|e| format!("Failed to copy right text: {:?}", e))?;
        let nul = unsafe {
            self.builder
                .build_gep(i8t, dest, &[total], "concat_nul")
                .map_err(|e| format!("Failed to offset NUL: {:?}", e))?
        };
        self.builder
            .build_store(nul, i8t.const_zero())
            .map_err(|e| format!("Failed to write NUL: {:?}", e))?;

        let text_ty = self.ptr_len_struct_type();
        let with_ptr = self
            .builder
            .build_insert_value(text_ty.get_undef(), dest, 0, "cat_ptr")
            .map_err(|e| format!("Failed to insert concat ptr: {:?}", e))?
            .into_struct_value();
        let text = self
            .builder
            .build_insert_value(with_ptr, total, 1, "cat_len")
            .map_err(|e| format!("Failed to insert concat len: {:?}", e))?
            .into_struct_value();
        Ok(text.into())
    }

    /// Build a direct call to an already-emitted function by symbol, given the
    /// already-generated argument values. Used to lower a resolved operator/function
    /// overload to its mangled target.
    fn build_direct_call(
        &mut self,
        symbol: &str,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .module
            .get_function(symbol)
            .ok_or_else(|| format!("Overload not found: {}", symbol))?;
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        use inkwell::values::AnyValue;
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(|e| format!("Failed to build call: {:?}", e))?;
        match call_site.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("Overloaded function does not return a basic value".to_string()),
        }
    }

    /// Lower a `Text`-vs-`Text` comparison: call `__text_cmp(aptr, alen, bptr, blen)`
    /// (returns -1/0/1, memcmp-style with the shorter string ordering first on a common
    /// prefix), then compare that i32 result against 0 with the predicate matching `op`.
    /// Backs `Text` equality and lexicographic ordering (`==`/`!=`/`<`/`<=`/`>`/`>=`).
    fn generate_text_compare(
        &mut self,
        op: BinOp,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) = (lhs, rhs) else {
            return Err("Text comparison requires two Text values".to_string());
        };
        let extract = |s: inkwell::values::StructValue<'ctx>,
                       idx: u32,
                       name: &str|
         -> Result<BasicValueEnum<'ctx>, String> {
            self.builder
                .build_extract_value(s, idx, name)
                .map_err(|e| format!("Failed to extract text field: {:?}", e))
        };
        let l_ptr = extract(l, 0, "lcmp_ptr")?.into_pointer_value();
        let l_len = extract(l, 1, "lcmp_len")?.into_int_value();
        let r_ptr = extract(r, 0, "rcmp_ptr")?.into_pointer_value();
        let r_len = extract(r, 1, "rcmp_len")?.into_int_value();

        let cmp_fn = self.get_intrinsic("__text_cmp")?;
        use inkwell::values::AnyValue;
        let cmp = self
            .builder
            .build_call(
                cmp_fn,
                &[l_ptr.into(), l_len.into(), r_ptr.into(), r_len.into()],
                "text_cmp",
            )
            .map_err(|e| format!("Failed to call __text_cmp: {:?}", e))?
            .as_any_value_enum()
            .into_int_value();

        let pred = match op {
            BinOp::Eq => inkwell::IntPredicate::EQ,
            BinOp::Ne => inkwell::IntPredicate::NE,
            BinOp::Lt => inkwell::IntPredicate::SLT,
            BinOp::Le => inkwell::IntPredicate::SLE,
            BinOp::Gt => inkwell::IntPredicate::SGT,
            BinOp::Ge => inkwell::IntPredicate::SGE,
            _ => return Err("non-comparison operator in text compare".to_string()),
        };
        let zero = cmp.get_type().const_zero();
        Ok(self
            .builder
            .build_int_compare(pred, cmp, zero, "text_cmp_res")
            .map_err(|e| format!("Failed to build text compare: {:?}", e))?
            .into())
    }

    fn generate_field_access(
        &mut self,
        expr: &Expr,
        field_name: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A record may legitimately have a field literally named `size`/`length`.
        // Resolve known record fields by NAME first (matching the type checker,
        // which dispatches on static type) so they don't collide with the Text/array
        // `.size`/`.length` struct-shape handling below. Text/array values are never
        // tracked in `record_types`, so this only diverts genuine record fields.
        let is_named_record_field = matches!(expr, Expr::Ident { name, .. }
            if self
                .record_types
                .get(name)
                .is_some_and(|fields| fields.iter().any(|f| f == field_name)));

        // Special handling for .size field on arrays
        if !is_named_record_field && field_name == "size" {
            // For arrays (which are structs {ptr, i64}), we need special handling
            // Check if it's an identifier - we can directly work with the alloca
            if let Expr::Ident { name, .. } = expr
                && let Some((var_ptr, var_type)) = self.variables.get(name).cloned()
            {
                // Check if this is a struct type (could be an array)
                if let BasicTypeEnum::StructType(struct_type) = var_type {
                    // Get field 1 (size field of array struct) directly from the alloca
                    let size_field = self
                        .builder
                        .build_struct_gep(struct_type, var_ptr, 1, "size_field")
                        .map_err(|e| format!("Failed to get size field: {:?}", e))?;

                    let size_val = self
                        .builder
                        .build_load(self.context.i64_type(), size_field, "size")
                        .map_err(|e| format!("Failed to load size: {:?}", e))?;

                    // Convert i64 to f64 (Num)
                    if let BasicValueEnum::IntValue(i) = size_val {
                        let size_f64 = self
                            .builder
                            .build_signed_int_to_float(i, self.context.f64_type(), "size_as_num")
                            .map_err(|e| format!("Failed to convert size: {:?}", e))?;

                        return Ok(size_f64.into());
                    }
                }
            }
        }

        // Text/array as a value: `.size` is the i64 length field (byte length for
        // Text); `.length` is the grapheme count (Text only — the checker rejects
        // `.length` on arrays). Handles non-identifier receivers like `("a"+"b").size`.
        if !is_named_record_field && (field_name == "size" || field_name == "length") {
            let val = self.generate_expr(expr)?;
            if let BasicValueEnum::StructValue(s) = val {
                let len = self
                    .builder
                    .build_extract_value(s, 1, "len_field")
                    .map_err(|e| format!("Failed to extract length field: {:?}", e))?
                    .into_int_value();
                if field_name == "size" {
                    return Ok(self
                        .builder
                        .build_signed_int_to_float(len, self.context.f64_type(), "size_as_num")
                        .map_err(|e| format!("Failed to convert size: {:?}", e))?
                        .into());
                }
                // `.length`: grapheme-cluster count via __text_length(data, byte_len).
                let data = self
                    .builder
                    .build_extract_value(s, 0, "data_field")
                    .map_err(|e| format!("Failed to extract data field: {:?}", e))?
                    .into_pointer_value();
                let len_fn = self.get_intrinsic("__text_length")?;
                use inkwell::values::AnyValue;
                let count = self
                    .builder
                    .build_call(len_fn, &[data.into(), len.into()], "graphemes")
                    .map_err(|e| format!("Failed to call __text_length: {:?}", e))?
                    .as_any_value_enum()
                    .into_int_value();
                return Ok(self
                    .builder
                    .build_signed_int_to_float(count, self.context.f64_type(), "length_as_num")
                    .map_err(|e| format!("Failed to convert length: {:?}", e))?
                    .into());
            }
        }

        // Regular record field access: resolve a pointer to the field inside the
        // record's memory (shared by the in-place field-write path) and load it with the
        // field's declared LLVM type from the oracle (NOT a hardcoded `f64`), so a
        // `Text`/array field reads back correctly.
        if let Some((field_ptr, field_llvm)) = self.record_field_pointer(expr, field_name)? {
            return self
                .builder
                .build_load(field_llvm, field_ptr, field_name)
                .map_err(|e| format!("Failed to load field: {:?}", e));
        }

        Err(format!(
            "Field access not fully implemented. Need type information for field '{}'",
            field_name
        ))
    }

    /// In-place field write `target := value`, where `target` is a field access
    /// `obj.field`. Computes a pointer into the existing record memory via GEP and
    /// stores `value` there — no re-allocation — so the mutation is observable
    /// through every alias of the record. Yields `$` (a unit i8), matching the
    /// type checker's `Unit` result for a field write.
    fn generate_field_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Expr::FieldAccess { expr, field, .. } = target else {
            return Err("Field-write target must be a field access".to_string());
        };
        let new_value = self.generate_expr(value)?;
        let (field_ptr, _field_llvm) = self
            .record_field_pointer(expr, field)?
            .ok_or_else(|| format!("Unknown record for field write: {}", field))?;
        self.builder
            .build_store(field_ptr, new_value)
            .map_err(|e| format!("Failed to store field: {:?}", e))?;
        Ok(self.unit_value().into())
    }

    /// Pointer to `base.field` inside the record's memory, plus the field's value-repr
    /// LLVM type — the shared primitive for both reads (`generate_field_access`) and
    /// in-place writes (`generate_field_assign`).
    ///
    /// `base` must be a record/named-type identifier (a variable such as `u`, or the
    /// method receiver `it`); the variable's alloca holds a pointer-to-struct (the
    /// record ABI). The struct's field types are recovered from the **type oracle** (the
    /// record's declared field types), mapped through `value_repr_type` so the
    /// reconstructed struct type matches exactly how `generate_record` laid it out —
    /// `Text`/array/etc. fields keep their real type instead of being treated as `f64`.
    /// The returned LLVM type is what the read site must `load` (and the write site is
    /// already type-checked to match).
    ///
    /// Nested records (`a.b.c`) are rejected by the type checker before codegen, so a
    /// single GEP level suffices. Returns `Ok(None)` when `base` isn't a tracked record
    /// (so the read path can fall through to its Text/array `.size` handling).
    fn record_field_pointer(
        &mut self,
        base: &Expr,
        field: &str,
    ) -> Result<Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>, String> {
        let Expr::Ident { name, .. } = base else {
            return Ok(None);
        };
        let Some(field_names) = self.record_types.get(name).cloned() else {
            return Ok(None);
        };
        let Some(field_idx) = field_names.iter().position(|f| f == field) else {
            return Ok(None);
        };

        // Reconstruct the record's struct type from the oracle (its declared field types,
        // in declared order) via the shared `record_struct_type`, so the GEP type matches
        // construction. Fall back to all-`f64` only if the oracle has no record type for
        // `base` (it always should for a tracked record) — preserving the historical
        // numeric layout. The loaded field's own LLVM type is then just the indexed slot.
        let struct_type = match self.oracle.expr_type(base) {
            Some(Type::Record(fields)) | Some(Type::Named { fields, .. }) => {
                let fields = fields.clone();
                self.record_struct_type(&fields)?
            }
            _ => {
                let f64t: BasicTypeEnum = self.context.f64_type().into();
                self.context
                    .struct_type(&vec![f64t; field_names.len()], false)
            }
        };
        let field_llvm = struct_type
            .get_field_type_at_index(field_idx as u32)
            .ok_or_else(|| format!("record field index {field_idx} out of range"))?;

        // The variable's alloca holds a pointer to the struct; load it.
        let (var_ptr, _) = self
            .variables
            .get(name)
            .ok_or_else(|| format!("Variable not found: {}", name))?;
        let struct_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                *var_ptr,
                "load_struct_ptr",
            )
            .map_err(|e| format!("Failed to load struct pointer: {:?}", e))?
            .into_pointer_value();

        let field_ptr = self
            .builder
            .build_struct_gep(
                struct_type,
                struct_ptr,
                field_idx as u32,
                &format!("field_{}_ptr", field),
            )
            .map_err(|e| format!("Failed to build field GEP: {:?}", e))?;
        Ok(Some((field_ptr, field_llvm)))
    }

    /// Lower an array index `array[index]`. `index_node` is the whole `Expr::Index`
    /// (used to look up the element type in the oracle — the checker records an index
    /// expression's type as its element type); `array` and `index_expr` are its parts.
    fn generate_index(
        &mut self,
        index_node: &Expr,
        array: &Expr,
        index_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Generate the array expression
        let array_val = self.generate_expr(array)?;

        // Generate the index expression
        let index_val = self.generate_expr(index_expr)?;

        // Array is a struct { ptr data, i64 size }
        // We need to:
        // 1. Extract the data pointer (field 0)
        // 2. Convert index from f64 to i64
        // 3. Use GEP to get element pointer
        // 4. Load the element

        if let BasicValueEnum::StructValue(struct_val) = array_val {
            // Store struct temporarily to access fields
            let struct_type = struct_val.get_type();
            let alloca = self
                .builder
                .build_alloca(struct_type, "temp_array")
                .map_err(|e| format!("Failed to allocate temp: {:?}", e))?;

            self.builder
                .build_store(alloca, struct_val)
                .map_err(|e| format!("Failed to store array: {:?}", e))?;

            // Get data pointer (field 0)
            let data_field = self
                .builder
                .build_struct_gep(struct_type, alloca, 0, "data_ptr_field")
                .map_err(|e| format!("Failed to get data field: {:?}", e))?;

            let data_ptr = self
                .builder
                .build_load(
                    self.context.ptr_type(AddressSpace::default()),
                    data_field,
                    "data_ptr",
                )
                .map_err(|e| format!("Failed to load data ptr: {:?}", e))?
                .into_pointer_value();

            // Convert index from f64 to i64
            let index_i64 = if let BasicValueEnum::FloatValue(f) = index_val {
                self.builder
                    .build_float_to_signed_int(f, self.context.i64_type(), "index_i64")
                    .map_err(|e| format!("Failed to convert index: {:?}", e))?
            } else {
                return Err("Index must be a number".to_string());
            };

            // Element LLVM type comes from the type oracle (the index expression's type
            // IS the element type), NOT from a hardcoded `f64` — so `Text`/array/record
            // elements load correctly. The element memory was laid out by `generate_array`
            // using this same value representation.
            let elem_llvm = self.oracle_value_type(index_node)?;

            // Use GEP (indexing by element type) to get the element pointer, then load it.
            let elem_ptr = unsafe {
                self.builder
                    .build_gep(elem_llvm, data_ptr, &[index_i64], "elem_ptr")
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?
            };

            self.builder
                .build_load(elem_llvm, elem_ptr, "elem")
                .map_err(|e| format!("Failed to load element: {:?}", e))
        } else {
            Err("Can only index into arrays".to_string())
        }
    }

    /// Lower a `match` (`scrutinee ? | pat => body ...`). `match_expr` is the whole
    /// `Expr::Match` node (used only to look up the match's result type in the oracle);
    /// `scrutinee` is the value being matched.
    fn generate_match(
        &mut self,
        match_expr: &Expr,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Evaluate the expression being matched
        let match_val = self.generate_expr(scrutinee)?;

        // Get the current function
        let function = self
            .current_function
            .ok_or_else(|| "Match expression must be in a function".to_string())?;

        // Create basic blocks for each arm and a continuation block
        let mut arm_blocks = vec![];
        let mut check_blocks = vec![];
        for i in 0..arms.len() {
            check_blocks.push(
                self.context
                    .append_basic_block(function, &format!("check_{}", i)),
            );
            arm_blocks.push(
                self.context
                    .append_basic_block(function, &format!("arm_{}", i)),
            );
        }
        let cont_block = self.context.append_basic_block(function, "match_cont");

        // The result type of the match (the common type of its arm bodies) comes from
        // the type oracle — NOT a hardcoded `f64` — so a match yielding `Text` (e.g. the
        // `Ok(text)` payload) allocates and loads a `Text` struct rather than corrupting
        // it through an f64 slot. Falls back to `f64` if the oracle didn't record it.
        let result_llvm = self.oracle_value_type(match_expr)?;
        let result_alloca = self.create_entry_block_alloca("match_result", result_llvm)?;

        // Jump to first check
        self.builder
            .build_unconditional_branch(check_blocks[0])
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        // Generate code for each arm
        for (i, arm) in arms.iter().enumerate() {
            // Position at check block
            self.builder.position_at_end(check_blocks[i]);

            // Check if pattern matches
            let matches = self.check_pattern(&arm.pattern, match_val)?;

            // Conditional branch to arm or next check
            let next_block = if i + 1 < check_blocks.len() {
                check_blocks[i + 1]
            } else {
                // Last arm - if it doesn't match, it's an error
                // For now, just go to continuation with a default value
                cont_block
            };

            self.builder
                .build_conditional_branch(matches, arm_blocks[i], next_block)
                .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

            // Generate arm body
            self.builder.position_at_end(arm_blocks[i]);

            // Bind pattern variables
            self.bind_pattern(&arm.pattern, match_val, scrutinee)?;

            let arm_val = self.generate_expr(&arm.body)?;
            self.builder
                .build_store(result_alloca, arm_val)
                .map_err(|e| format!("Failed to store result: {:?}", e))?;

            self.builder
                .build_unconditional_branch(cont_block)
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        }

        // Position at continuation block
        self.builder.position_at_end(cont_block);

        // Load the result with the match's declared result type (see `result_llvm`).
        self.builder
            .build_load(result_llvm, result_alloca, "match_result")
            .map_err(|e| format!("Failed to load result: {:?}", e))
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Wildcard always matches
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Ident { .. } => {
                // Identifier pattern always matches (binds the value)
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Number { value: num_val, .. } => {
                // Compare the value
                if let BasicValueEnum::FloatValue(fval) = value {
                    let const_val = self.context.f64_type().const_float(*num_val);
                    self.builder
                        .build_float_compare(
                            inkwell::FloatPredicate::OEQ,
                            fval,
                            const_val,
                            "num_match",
                        )
                        .map_err(|e| format!("Failed to build comparison: {:?}", e))
                } else {
                    Ok(self.context.bool_type().const_zero())
                }
            }

            Pattern::Constructor { name, .. } => {
                // Tagged-union dispatch: a value is `{ i8 tag, <payload> }`; the tag is
                // the variant's declaration index, looked up from the sum-variant
                // registry (generalizes the old hardcoded Ok=0/NotOk=1).
                let expected_tag = self
                    .sum_variants
                    .get(name.as_str())
                    .map(|(tag, _)| *tag)
                    .ok_or_else(|| format!("Unknown constructor: {}", name))?;

                // Extract tag from struct (field 0)
                if let BasicValueEnum::StructValue(struct_val) = value {
                    let tag_val = self
                        .builder
                        .build_extract_value(struct_val, 0, "tag")
                        .map_err(|e| format!("Failed to extract tag: {:?}", e))?
                        .into_int_value();

                    let expected_tag_val =
                        self.context.i8_type().const_int(expected_tag as u64, false);

                    self.builder
                        .build_int_compare(
                            inkwell::IntPredicate::EQ,
                            tag_val,
                            expected_tag_val,
                            "tag_match",
                        )
                        .map_err(|e| format!("Failed to compare tags: {:?}", e))
                } else {
                    // Not a struct - pattern doesn't match
                    Ok(self.context.bool_type().const_zero())
                }
            }
        }
    }

    /// Concrete per-value payload types for the matched constructor `variant`, read from
    /// the SCRUTINEE's oracle type. A scrutinee inferred as `Result[Ok(Text)]` (from
    /// `Ok("x")`) yields `[Text]` for `Ok`, so a payload binding can record its REAL type
    /// for overload mangling — unlike the declared `variant_payloads`, whose `Result`
    /// slots are `Generic` (which would mis-mangle to the `Num` member). `None` when the
    /// oracle has no concrete `Sum` type for the scrutinee.
    fn scrutinee_payload_types(&self, scrutinee: &Expr, variant: &str) -> Option<Vec<Type>> {
        match self.oracle.expr_type(scrutinee)? {
            Type::Sum { variants, .. } => variants
                .iter()
                .find(|v| v.name == variant)
                .map(|v| v.fields.clone()),
            _ => None,
        }
    }

    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
        scrutinee: &Expr,
    ) -> Result<(), String> {
        match pattern {
            Pattern::Ident { name, .. } => {
                // Bind the value to the identifier
                let alloca = self.create_entry_block_alloca(name, value.get_type())?;
                self.builder
                    .build_store(alloca, value)
                    .map_err(|e| format!("Failed to store pattern binding: {:?}", e))?;
                self.variables
                    .insert(name.clone(), (alloca, value.get_type()));
                Ok(())
            }

            Pattern::Constructor { name, args, .. } => {
                // Extract each payload field and bind it to the corresponding sub-pattern.
                // The value is `{ i8 tag, payload0, payload1, ... }`, so payload `i` is
                // struct field `i + 1`. Only identifier sub-patterns bind a name; others
                // (wildcards, nested constructors) are matched structurally elsewhere.
                //
                // Each payload binding records its Quilon type in `var_types` (the map
                // that mangles an overloaded call on the binding, e.g.
                // `Ok(s) => describe(s)`), taken from the first NON-generic of two ordered
                // sources:
                //  - the SCRUTINEE's oracle type, whose `Result` payload was specialized
                //    per value (`Ok("x")` => `Result[Ok(Text)]`), so `s` binds as `Text`;
                //  - else the variant's declared payloads (`variant_payloads`), concrete
                //    for a USER sum type (`Circle(Num)`) but `Generic` for `Result`.
                // A still-`Generic` payload is left untracked — an untracked binding
                // defaults to `Num` (the historical behavior), rather than mis-mangling.
                if let BasicValueEnum::StructValue(struct_val) = value {
                    let concrete = self.scrutinee_payload_types(scrutinee, name);
                    let declared = self.variant_payloads.get(name).cloned();
                    for (i, arg) in args.iter().enumerate() {
                        if let Pattern::Ident { name: arg_name, .. } = arg {
                            let payload = self
                                .builder
                                .build_extract_value(struct_val, (i + 1) as u32, "payload")
                                .map_err(|e| format!("Failed to extract payload: {:?}", e))?;
                            let alloca =
                                self.create_entry_block_alloca(arg_name, payload.get_type())?;
                            self.builder
                                .build_store(alloca, payload)
                                .map_err(|e| format!("Failed to store constructor arg: {:?}", e))?;
                            self.variables
                                .insert(arg_name.clone(), (alloca, payload.get_type()));
                            let payload_ty = [&concrete, &declared]
                                .into_iter()
                                .filter_map(|src| src.as_ref()?.get(i))
                                .find(|t| !matches!(t, Type::Generic { .. }));
                            if let Some(ty) = payload_ty {
                                self.var_types.insert(arg_name.clone(), ty.clone());
                            }
                        }
                    }
                }
                Ok(())
            }

            _ => Ok(()), // Other patterns don't bind variables
        }
    }

    /// The LLVM struct type for a record with the given (name, Quilon-type) fields, in
    /// declared order — each slot lowered through [`value_repr_type`]. This is the single
    /// definition of a record's memory layout, shared by record construction
    /// (`generate_record_update`) and field reads (`record_field_pointer`), so the two
    /// can never disagree on slot types.
    fn record_struct_type(
        &self,
        fields: &[(String, Type)],
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        let field_types: Vec<BasicTypeEnum> = fields
            .iter()
            .map(|(_, t)| self.value_repr_type(t))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.context.struct_type(&field_types, false))
    }

    /// The `{ ptr data, i64 len }` struct shared by arrays and `Text`. For `Text`,
    /// `data` is a NUL-terminated UTF-8 buffer and `len` is its byte length.
    fn ptr_len_struct_type(&self) -> inkwell::types::StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            false,
        )
    }

    /// The tagged-union LLVM struct for a sum type: `{ i8 tag, slot0, slot1, ... }`,
    /// where the slots come from the registered canonical payload layout. Falls back to
    /// the Result-style `{ i8, double }` for an unregistered name (e.g. a `-> Result`
    /// annotation reached before any user declaration), keeping the historical shape.
    fn sum_struct_type(&self, name: &str) -> inkwell::types::StructType<'ctx> {
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        match self.sum_layouts.get(name) {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.push(self.context.f64_type().into()),
        }
        self.context.struct_type(&field_types, false)
    }

    /// The tagged-union LLVM struct for a sum-typed *value* of type `Type::Sum`. A USER
    /// sum type has a registered canonical layout, so this defers to [`sum_struct_type`].
    /// The built-in `Result` has NONE (its payload is sized per value across its generic,
    /// heterogeneous variants), so its slot types are recovered from the CONCRETE
    /// (specialized) variant payloads this `Type::Sum` carries: `Result[Ok(Text)]` =>
    /// `{ i8, Text }`, so a function returning `Ok("x")` gets a return type matching the
    /// value the body actually produces.
    ///
    /// This MUST agree with `generate_sum_constructor`'s per-value Result shape: there a
    /// `Generic` slot has no value and a `$` (Unit) payload is stored into the canonical
    /// numeric `double` slot (a Unit carries no bits). So per slot we take the first field
    /// that is neither `Generic` NOR `Unit` (the checker guarantees concrete fields at a
    /// position agree) and lower it via [`value_repr_type`]; a slot that is only
    /// generic/unit/absent falls back to `double`, preserving the historical
    /// `{ i8, double }` shape for a still-generic or unit-only `Result`.
    fn sum_value_struct_type(
        &self,
        name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<inkwell::types::StructType<'ctx>, String> {
        if self.sum_layouts.contains_key(name) || variants.is_empty() {
            return Ok(self.sum_struct_type(name));
        }
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        let max_fields = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        for i in 0..max_fields {
            let concrete = variants
                .iter()
                .filter_map(|v| v.fields.get(i))
                .find(|f| !matches!(f, Type::Generic { .. } | Type::Unit));
            let slot = match concrete {
                Some(f) => self.value_repr_type(f)?,
                None => self.context.f64_type().into(),
            };
            field_types.push(slot);
        }
        Ok(self.context.struct_type(&field_types, false))
    }

    /// The single value of the Unit type (`$`), lowered as a zero `i8`. Its bits are
    /// never observed; the entry-point wrapper coerces a non-Num body to exit code 0.
    fn unit_value(&self) -> inkwell::values::IntValue<'ctx> {
        self.context.i8_type().const_int(0, false)
    }

    /// Whether `expr`'s value has type Unit (`$`). Codegen lacks the checker's full
    /// inference, so for an *unannotated* function we look at the body's tail to pick
    /// the LLVM return type: a Unit tail must be `i8`, not the `Num`/f64 default. The
    /// only Unit-producing expressions are the `$` literal and `print`/`eprint` calls
    /// (which return `$`); a block/ternary is Unit when its tail is. Other unannotated
    /// non-Num bodies (Text, Bool, ...) keep the pre-existing `Num`-default behavior.
    fn expr_is_unit(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Unit { .. } => true,
            // An in-place field write `obj.field := v` is an effect; it yields `$`.
            Expr::FieldAssign { .. } => true,
            Expr::Call { func, .. } => {
                matches!(func.as_ref(), Expr::Ident { name, .. } if name == "print" || name == "eprint")
            }
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.expr_is_unit(tail),
                _ => false,
            },
            Expr::If { then, else_, .. } => self.expr_is_unit(then) && self.expr_is_unit(else_),
            _ => false,
        }
    }

    /// The **value representation** of a Quilon type — the LLVM type that a value of
    /// `ty` is materialized as by `generate_expr` and stored inline inside a composite.
    /// Read sites that GEP/load an element/field/match-result must size it with THIS
    /// function so the type matches how the value was stored at construction. It differs
    /// from [`type_to_llvm`] in three places:
    ///   - `Array` — an array *value* is the `{ ptr, i64 }` struct `generate_array`
    ///     produces and stores inline (so a nested array `[][]T` keeps that struct as its
    ///     element), whereas `type_to_llvm` lowers `[]T` to a bare opaque pointer.
    ///   - `Record` / `Named` — a record *value* is a POINTER to its struct (the record
    ///     ABI: `generate_record` returns the alloca), not the struct by value.
    ///   - `Generic` — a payload type variable that survived to a read site (e.g. a match
    ///     whose result type was taken from a never-constructed variant's generic arm)
    ///     has no concrete LLVM type; it falls back to the canonical numeric payload
    ///     representation `f64`, matching how generic/unknown payloads are materialized
    ///     elsewhere (`payload_slot_type`). This keeps such a program compiling (it did
    ///     before the oracle existed) rather than erroring in `type_to_llvm`.
    fn value_repr_type(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Array(_) => Ok(self.ptr_len_struct_type().into()),
            Type::Record(_) | Type::Named { .. } => {
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Generic { .. } => Ok(self.context.f64_type().into()),
            _ => self.type_to_llvm(ty),
        }
    }

    /// The LLVM type a value of `ty` takes when it CROSSES a function boundary — a param
    /// or a return, for top-level functions, methods, and closures alike. An array must
    /// use its VALUE representation (the `{ ptr, i64 }` struct, so callers can `.size` /
    /// index / concatenate the result), matching how array values flow everywhere else;
    /// everything else keeps its `type_to_llvm` lowering. This is deliberately NOT the
    /// whole of [`value_repr_type`]: a `Record`/`Named` argument keeps its by-pointer ABI
    /// and a `Generic` keeps `type_to_llvm`, so only the array case diverges here. Every
    /// signature site funnels through this one method so the boundary rule lives in a
    /// single place.
    fn boundary_type(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Array(_) => self.value_repr_type(ty),
            _ => self.type_to_llvm(ty),
        }
    }

    /// The value-representation LLVM type to use when GEPing/loading the result of `expr`
    /// (an `arr[i]`, `rec.field`, or `match`), taken from the type oracle. This is the
    /// single read-site policy: ask the oracle for `expr`'s inferred type and lower it
    /// via [`value_repr_type`]; if the oracle has no entry (e.g. the IR-only codegen
    /// tests that skip the type-check pass), fall back to the historical `f64`.
    fn oracle_value_type(&self, expr: &Expr) -> Result<BasicTypeEnum<'ctx>, String> {
        match self.oracle.expr_type(expr) {
            Some(t) => self.value_repr_type(t),
            None => Ok(self.context.f64_type().into()),
        }
    }

    /// Best-effort Quilon type of `expr`, sufficient to mangle overloaded call sites.
    /// Codegen lacks the type checker's full inference, so this covers exactly the
    /// shapes that can be an overloaded argument: literals, locals/params (tracked in
    /// `var_types`), constructor results, field access on a known record, and the
    /// result types of the supported operators. Falls back to `Num` (the historical
    /// default) when it can't tell — overloaded dispatch then simply won't match and a
    /// clear "function not found" surfaces, never a silent miscompile.
    fn infer_type(&self, expr: &Expr) -> Type {
        // Prefer the type checker's authoritative type for this exact node (the oracle) —
        // the same source codegen's read sites use. It knows shapes the structural fallback
        // below can't (an `arr[i]` element, a `.split(…)`/`.replace(…)` result, a field
        // read), so an overloaded call taking one of those (e.g. `assertEq(parts[0], …)`)
        // mangles to the right member. Falls back to structural inference only when the
        // oracle has no entry — the IR-only codegen tests that skip the type-check pass.
        if let Some(ty) = self.oracle.expr_type(expr) {
            return ty.clone();
        }
        match expr {
            Expr::Number { .. } => Type::Num,
            Expr::String { .. } => Type::Text,
            Expr::Bool { .. } => Type::Bool,
            Expr::Unit { .. } => Type::Unit,
            Expr::Ident { name, .. } => {
                // A bare nullary sum-type constructor (not a bound variable) is a value
                // of its sum type.
                if let Some((_, type_name)) = self.sum_variants.get(name)
                    && !self.var_types.contains_key(name)
                {
                    return self.sum_or_named(type_name);
                }
                self.var_types.get(name).cloned().unwrap_or(Type::Num)
            }
            Expr::Constructor { type_name, .. } => self.sum_or_named(type_name),
            Expr::Call { func, args, .. } => {
                if let Expr::Ident { name, .. } = func.as_ref() {
                    // A constructor call yields its sum type.
                    if let Some((_, type_name)) = self.sum_variants.get(name) {
                        return self.sum_or_named(type_name);
                    }
                    // An overloaded function call yields its resolved member's return.
                    let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                    if let Some((_, ret)) = self.matching_overload(name, &arg_types) {
                        return ret.clone();
                    }
                    // A non-overloaded top-level function yields its declared return
                    // type — so a call result that feeds an overloaded call/operator
                    // mangles to the right member (codegen agrees with the checker).
                    if let Some(ret) = self.fn_return_types.get(name) {
                        return self.resolve_named(ret);
                    }
                }
                // Unknown callee (no declared return, e.g. an unannotated function):
                // default to Num, the historical inference default.
                Type::Num
            }
            Expr::BinOp {
                left, op, right, ..
            } => {
                // A user operator overload yields its resolved member's return type.
                let sym = op.symbol();
                if self.overloads.contains_key(sym) {
                    let arg_types = [self.infer_type(left), self.infer_type(right)];
                    if let Some((_, ret)) = self.matching_overload(sym, &arg_types) {
                        return ret.clone();
                    }
                }
                // Built-ins: comparisons/logicals yield Bool; `+` on Text yields Text;
                // arithmetic yields Num. Matches the type checker's operator results
                // closely enough to mangle a nested overloaded argument.
                match op {
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => Type::Bool,
                    BinOp::Add
                        if self.infer_type(left) == Type::Text
                            || self.infer_type(right) == Type::Text =>
                    {
                        Type::Text
                    }
                    _ => Type::Num,
                }
            }
            Expr::If { then, .. } => self.infer_type(then),
            // A `?`/`|` match's result type is whatever its arms yield. Codegen can't
            // easily unify the arms, so take the checker's recorded type from the oracle
            // (as record/spread do); this lets a local bound to a match — e.g.
            // `ok = r ? | Ok(_) => true | NotOk(_) => false` — mangle correctly when it
            // later feeds an overloaded call such as `assert(ok)`.
            Expr::Match { .. } => self.oracle.expr_type(expr).cloned().unwrap_or(Type::Num),
            // Unary `!` is logical-not (Bool); unary `-` is numeric negation (Num). So a
            // local bound to `!ok` mangles as Bool when it feeds an overloaded call.
            Expr::UnaryOp { op, .. } => match op {
                crate::ast::UnaryOp::Not => Type::Bool,
                crate::ast::UnaryOp::Neg => Type::Num,
            },
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.infer_type(tail),
                _ => Type::Num,
            },
            Expr::FieldAccess { field, .. } if field == "size" || field == "length" => Type::Num,
            // A record literal / spread — including a functional-update — takes its type
            // from the oracle (which resolves the named-vs-anonymous result of a `<-`
            // spread), so a binding to it mangles / tracks correctly.
            Expr::Record { .. } | Expr::Spread { .. } => {
                self.oracle.expr_type(expr).cloned().unwrap_or(Type::Num)
            }
            _ => Type::Num,
        }
    }

    /// Normalize a declared type annotation for `infer_type`: a bare `Named { name }`
    /// (the parser's form for a `:: SomeType` reference) becomes the canonical sum/named
    /// tag so it mangles identically to an inferred value of that type. Built-ins pass
    /// through unchanged.
    fn resolve_named(&self, ty: &Type) -> Type {
        match ty {
            Type::Named { name, .. } | Type::Sum { name, .. } => self.sum_or_named(name),
            other => other.clone(),
        }
    }

    /// The `Type` for a registered type name: a sum type if known, else a `Named`.
    fn sum_or_named(&self, name: &str) -> Type {
        if self.sum_layouts.contains_key(name) || name == "Result" {
            Type::Sum {
                name: name.to_string(),
                variants: vec![],
            }
        } else {
            Type::Named {
                name: name.to_string(),
                fields: vec![],
                methods: vec![],
            }
        }
    }

    /// If `name` is an overload set, pick the member matching `arg_types` exactly and
    /// return its mangled LLVM symbol. `None` if `name` isn't overloaded or nothing
    /// matches (the caller then falls back to its non-overloaded path).
    fn resolve_overload_symbol(&self, name: &str, arg_types: &[Type]) -> Option<String> {
        let (params, _) = self.matching_overload(name, arg_types)?;
        Some(mangle_overload(name, params))
    }

    /// The overload member of `name` whose parameter types match `arg_types` exactly
    /// (by type tag), if any. Shared by symbol resolution and return-type inference.
    fn matching_overload(&self, name: &str, arg_types: &[Type]) -> Option<&(Vec<Type>, Type)> {
        self.overloads.get(name)?.iter().find(|(params, _)| {
            params.len() == arg_types.len()
                && params
                    .iter()
                    .zip(arg_types)
                    .all(|(p, a)| type_mangle(p) == type_mangle(a))
        })
    }

    fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            // Unit (`$`) is a zero `i8` — a concrete one-inhabitant placeholder.
            Type::Unit => Ok(self.context.i8_type().into()),
            // Text is { ptr data, i64 byte_len } (same shape as an array).
            Type::Text => Ok(self.ptr_len_struct_type().into()),
            Type::Array(elem_type) => {
                // Validate the element type, but LLVM uses opaque pointers so the
                // pointee type is not encoded in the pointer itself.
                let _elem = self.type_to_llvm(elem_type)?;
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Record(fields) => {
                let field_types: Vec<BasicTypeEnum> = fields
                    .iter()
                    .map(|(_name, ty)| self.type_to_llvm(ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.context.struct_type(&field_types, false).into())
            }
            Type::Sum { name, variants } => Ok(self.sum_value_struct_type(name, variants)?.into()),
            // A `Named` reference with no fields is a parsed type annotation (e.g. a
            // function param `s :: Shape`). If it names a registered sum type, lower it
            // to that type's tagged-union struct.
            Type::Named { name, fields, .. }
                if fields.is_empty() && self.sum_layouts.contains_key(name) =>
            {
                Ok(self.sum_struct_type(name).into())
            }
            // Any other named RECORD type (a `:: SomeRecord` parameter/return, e.g. on a
            // user operator overload) is passed by pointer — record instances are
            // represented as a pointer to their struct alloca (see `generate_record`).
            Type::Named { .. } => Ok(self.context.ptr_type(AddressSpace::default()).into()),
            _ => Err(format!("Unsupported type: {:?}", ty)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::parse;

    #[test]
    fn test_simple_number() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let tokens = Lexer::tokenize("x = 42").unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        // Global variable with float value
        assert!(ir.contains("4.2") || ir.contains("42"));
    }

    #[test]
    fn test_simple_function() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        assert!(ir.contains("define"));
        assert!(ir.contains("add"));
    }

    #[test]
    fn test_local_variable() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let code = "double = x :: Num => < y = x + x y >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Local variable
        assert!(ir.contains("load")); // Variable load
        assert!(ir.contains("store")); // Variable store
        assert!(ir.contains("fadd")); // Addition
    }

    #[test]
    fn test_array() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test array in a function body - return the first element as a number
        let code = "sum = x :: Num => < arr = [x, x, x] x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Array allocation
        assert!(ir.contains("getelementptr")); // Array element access
    }

    #[test]
    fn test_function_call() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test calling a function
        let code = "
            add = (a :: Num, b :: Num) => a + b
            main = => add(3, 4)
        ";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("call")); // Function call
        assert!(ir.contains("@add")); // Call to add function
        assert!(ir.contains("fadd")); // Addition in add function
    }

    #[test]
    fn test_record() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test record creation
        let code = "make_point = (x :: Num, y :: Num) => < p = {x = x, y = y} x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Struct allocation
        assert!(ir.contains("getelementptr")); // Field access
    }

    #[test]
    fn test_field_access() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test field access
        let code = "get_x = (a :: Num, b :: Num) => < p = {x = a, y = b} p.x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("getelementptr")); // Field GEP
        assert!(ir.contains("load")); // Field load
    }

    #[test]
    fn test_method_codegen_and_dispatch() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // A named record with a method; the entry point constructs an instance and calls it.
        // All fields are Num so the field layout/access is exact.
        let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y
}

^ = () -> Num => <
  p = Point { x = 3, y = 4 }
  p.sum()
>";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        // The method is emitted as a mangled top-level function taking the receiver pointer.
        assert!(ir.contains("@Point_sum"));
        // And the call site dispatches to it.
        assert!(ir.contains("call") && ir.contains("Point_sum"));
    }

    #[test]
    fn test_method_calls_sibling_method() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // `doubled` calls the sibling method `sum` via `it.sum()` — exercises the signature
        // pre-pass (forward reference) and `it`-based dispatch.
        let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y,
  doubled = => it.sum() + it.sum()
}

^ = () -> Num => <
  p = Point { x = 10, y = 5 }
  p.doubled()
>";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("@Point_sum"));
        assert!(ir.contains("@Point_doubled"));
    }
}

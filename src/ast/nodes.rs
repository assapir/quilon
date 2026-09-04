// AST node definitions

use crate::lexer::Span;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    /// Top-level [`TEST_BLOCK_MARKER`] calls — `describe("…", () => < … >)` — kept apart
    /// from `items` because they are TEST code: `quilon test` synthesizes an entry point
    /// that runs them in order, and every other command ignores this field, which is what
    /// keeps a test suite out of a release build.
    pub test_blocks: Vec<Expression>,
}

/// The resolved name whose top-level call marks test code: `core.test`'s `describe`,
/// written `test.describe(…)` (or fully qualified) under an `<< core.test` import. There
/// is no attribute or `cfg` syntax — the call IS the marker, so a file's tests are
/// recognizable by the parser (see `Parser::parse_program`) with no annotation to keep in
/// sync; this constant is the POST-LINK spelling the checker compares against.
pub const TEST_BLOCK_MARKER: &str = "core.test.describe";

/// The resolved name of a test CASE, written `test.it("…", () => …)`. A recorded
/// assertion belongs inside one: `it` is what closes a case and tallies it, so an
/// `expect` anywhere else in a suite would set a failure mark nothing ever reports.
pub const TEST_CASE_MARKER: &str = "core.test.it";

/// The implicit receiver of a method or operator member — its subject, and an operator
/// member's left operand.
pub const RECEIVER: &str = "it";

/// The user-facing spelling of a qualified name: its last segment (`print` for
/// `core.io.print`, `Get` for `core.http.Get`). What rendered output and diagnostics that
/// speak the user's vocabulary show; the full name stays the identity everywhere else.
pub fn display_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// A module import: `<< core.io` (built-in dotted) or `<< "path/to/mod.qn"` (file path).
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    pub path: ModulePath,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModulePath {
    /// Built-in module referenced by dotted name, e.g. `core.io` -> ["core", "io"].
    BuiltinDotted(Vec<String>),
    /// User module referenced by a (relative or absolute) file path.
    FilePath(String),
}

impl ModulePath {
    /// The short name this import binds — the last dotted segment (`core.http` binds
    /// `http`), or a file import's stem (`"lib/math.qn"` binds `math`). The ONE statement
    /// of the binding rule, shared by the parser (which recognizes qualified chains as it
    /// reads) and the module loader (which builds the scope those chains resolve in).
    /// `None` when a file path has no usable stem.
    pub fn binding_name(&self) -> Option<String> {
        match self {
            ModulePath::BuiltinDotted(parts) => parts.last().cloned(),
            ModulePath::FilePath(raw) => {
                let normalized = raw.replace('\\', "/");
                std::path::Path::new(&normalized)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            }
        }
    }
}

impl std::fmt::Display for ModulePath {
    /// The path as it was written, so a diagnostic can quote the import line back.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModulePath::BuiltinDotted(parts) => write!(f, "{}", parts.join(".")),
            ModulePath::FilePath(path) => write!(f, "\"{path}\""),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum Item {
    VariableDeclaration(VariableDeclaration),
    FunctionDeclaration(FunctionDeclaration),
    TypeDeclaration(TypeDeclaration),
}

impl Item {
    /// The name this item declares, whichever kind of declaration it is.
    pub fn name(&self) -> &str {
        match self {
            Item::VariableDeclaration(declaration) => &declaration.name,
            Item::FunctionDeclaration(declaration) => &declaration.name,
            Item::TypeDeclaration(declaration) => &declaration.name,
        }
    }

    /// The declaration's source span, whichever kind of declaration it is.
    pub fn span(&self) -> &Span {
        match self {
            Item::VariableDeclaration(declaration) => &declaration.span,
            Item::FunctionDeclaration(declaration) => &declaration.span,
            Item::TypeDeclaration(declaration) => &declaration.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDeclaration {
    pub name: String,
    pub type_definition: TypeDefinition,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDefinition {
    /// A user-defined sum type: `Color = Red / Green / Blue`,
    /// `Shape = Circle(Num) / Rect(Num, Num)`. Variants are separated by `/`. An optional
    /// trailing `{ }` block carries METHODS ONLY (named methods, the render `` ` ``, and
    /// operator members) — a sum has no fields, so a field-like entry there is rejected.
    Sum {
        variants: Vec<SumVariant>,
        methods: Vec<MethodDeclaration>,
    },
    Record {
        fields: Vec<(String, Type)>,
        methods: Vec<MethodDeclaration>,
    },
}

impl TypeDefinition {
    /// The methods declared on this type, for either kind — a record's method members or
    /// a sum's `{ }` block. Lets a pass walk a type's methods without matching on the kind.
    pub fn methods(&self) -> &[MethodDeclaration] {
        match self {
            TypeDefinition::Sum { methods, .. } => methods,
            TypeDefinition::Record { methods, .. } => methods,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDeclaration {
    pub name: String,
    pub parameters: Vec<Parameter>, // Does not include implicit "it" parameter
    pub return_type: Option<Type>,
    pub body: Expression,
    /// Declared with `:=` rather than `=`: this method may mutate its receiver, and
    /// calling it requires a `:=` receiver. An `=` method is checked to make sure it
    /// does not mutate.
    pub mutating: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Item(Item),
    Expression(Expression),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariableDeclaration {
    pub mutable: bool,
    pub name: String,
    pub type_annotation: Option<Type>,
    pub value: Expression,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDeclaration {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<Type>,
    /// The `::` annotation written on the binding itself. A FUNCTION type there states the
    /// whole signature — `f :: (Num) -> Num = (n) => n + 1` — so its parameter types fill in
    /// the parameters the definition leaves unannotated and its return type is the
    /// function's; any other type states the return type alone (`f :: Num = (x :: Num) => x`).
    /// Read it through [`Self::parameter_type`] and [`Self::declared_return_type`] rather
    /// than directly: the two ways of writing a signature must never be read apart.
    pub binding_type: Option<Type>,
    pub body: Expression,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    /// Declared in a bundled corelib module (`core.io`, `core.time`, …) rather than in
    /// user code. Set by the module loader as it merges a built-in module's exports, and
    /// by the front end for a corelib file checked directly. It is what tells the inert
    /// placeholder of a compiler-provided name from a user's real definition of that name
    /// — see [`FunctionDeclaration::is_inert_corelib_placeholder`].
    pub from_corelib: bool,
    pub span: Span,
}

impl FunctionDeclaration {
    /// Whether codegen emits a module function (declaration + body) for this item at all —
    /// false for an inert corelib placeholder of a compiler-provided name, and for an `@`
    /// leaf IO primitive (both are lowered at their call sites instead). The ONE predicate
    /// the pre-declaration pass and body emission share, so a declaration is never left
    /// bodiless by the two drifting apart.
    pub fn emits_module_function(&self) -> bool {
        !self.is_inert_corelib_placeholder() && !self.name.starts_with('@')
    }
}

impl FunctionDeclaration {
    /// Whether this is the corelib's own declaration of a name the compiler provides
    /// itself (see [`BUILTIN_OVERLOADS`] and [`RENDERABLE_BUILTINS`]) — an inert placeholder
    /// that documents the signature while the real thing is a runtime intrinsic. It is
    /// ignored everywhere: neither registered as an overload member (it would duplicate the
    /// built-in one) nor type-checked or emitted. Provenance, not shape, is what marks it: a
    /// user's own `print`/`write`/`now` is a real definition however it is written, and gets
    /// the ordinary diagnostics — a name the compiler claims outright, a duplicate
    /// signature, or a member missing an annotation.
    /// Shared by the type checker and codegen so the two never disagree on what to skip.
    ///
    /// Two spellings are the same placeholder: linked in as a module the declaration is
    /// named in full (`core.io.print`), while a corelib file checked directly
    /// (`quilon check corelib/io.qn`) still carries the bare `print` — so a BARE name
    /// matches by last segment under the `from_corelib` gate. A qualified name matches
    /// only exactly, so a corelib module's own private helper that happens to share a
    /// builtin's short name (`core.cli.now`, say) stays a real function.
    pub fn is_inert_corelib_placeholder(&self) -> bool {
        self.from_corelib
            && (is_compiler_provided_name(&self.name)
                || (!self.name.contains('.')
                    && builtin_names().any(|builtin| display_name(builtin) == self.name)))
    }

    /// The parameter slots of a function type on the binding (`f :: (Num) -> Num = …`),
    /// when it has as many as the definition has parameters. A mismatched arity answers
    /// `None`: the type checker reports that against the annotation itself, and nothing
    /// else should quietly index the wrong slot.
    pub fn declared_parameters(&self) -> Option<&[Type]> {
        match &self.binding_type {
            Some(Type::Function { parameters, .. })
                if parameters.len() == self.parameters.len() =>
            {
                Some(parameters)
            }
            _ => None,
        }
    }

    /// The declared type of parameter `index`: its own annotation, else the matching slot
    /// of a function type on the binding.
    pub fn parameter_type(&self, index: usize) -> Option<&Type> {
        self.parameters[index]
            .type_annotation
            .as_ref()
            .or_else(|| self.declared_parameters().map(|slots| &slots[index]))
    }

    /// The declared return type: the `-> Type` annotation, else what the binding's `::`
    /// annotation says the result is — the return slot of a function type, or the whole
    /// annotation when it names anything else.
    pub fn declared_return_type(&self) -> Option<&Type> {
        if let Some(annotation) = &self.return_type {
            return Some(annotation);
        }
        match &self.binding_type {
            Some(Type::Function { return_type, .. }) => Some(return_type),
            other => other.as_ref(),
        }
    }
}

/// One member the compiler contributes to an overload set: the built-in's own signature.
pub struct BuiltinOverload {
    pub name: &'static str,
    pub parameters: &'static [Type],
    pub ret: Type,
}

/// One output built-in: its first argument is any renderable value, the rest a fixed
/// signature.
pub struct RenderableBuiltin {
    pub name: &'static str,
    /// The parameters AFTER the rendered one.
    pub rest: &'static [Type],
    pub ret: Type,
}

impl RenderableBuiltin {
    pub fn arity(&self) -> usize {
        1 + self.rest.len()
    }
}

/// The output built-ins. Each renders its first argument to `Text` through that type's
/// `` ` `` member — the path string interpolation takes — and writes those bytes:
/// `print`/`eprint` with a newline, to stdout/stderr; `write` to a file descriptor as they
/// are.
///
/// They have no signature per type, so the compiler claims these names at their own arity
/// and nothing extends them: a type becomes printable by defining its own `` ` ``. They
/// are `core.io`'s exports, reached as `io.print(...)` under an `<< core.io` import —
/// these are the post-link names — so a user's own bare `print` is simply a different,
/// unrelated function.
pub const RENDERABLE_BUILTINS: &[RenderableBuiltin] = &[
    RenderableBuiltin {
        name: "core.io.print",
        rest: &[],
        ret: Type::Unit,
    },
    RenderableBuiltin {
        name: "core.io.eprint",
        rest: &[],
        ret: Type::Unit,
    },
    RenderableBuiltin {
        name: "core.io.write",
        rest: &[Type::Num],
        ret: Type::Num,
    },
];

/// The output built-in `name` names, if any.
pub fn renderable_builtin(name: &str) -> Option<&'static RenderableBuiltin> {
    RENDERABLE_BUILTINS.iter().find(|b| b.name == name)
}

/// Whether a value of `ty` can be rendered to `Text` for output. Every type has a rendering
/// — its own `` ` `` member or the default for its shape — except a function, which is not a
/// value to show.
pub fn is_renderable(ty: &Type) -> bool {
    !matches!(ty, Type::Function { .. })
}

/// The corelib functions the compiler provides itself, as the members they occupy in their
/// overload sets — `core.time`'s `now` (post-link name, reached as `time.now()` under an
/// `<< core.time` import) and the internal primitives, all lowered to runtime intrinsics.
/// A module's overload sets are closed: a user's own bare `now` is an unrelated function,
/// not a member beside the built-in. The output family (`print`/`eprint`/`write`) takes no
/// signature per type and lives in [`RENDERABLE_BUILTINS`] instead.
///
/// The `__`-prefixed entries are internal primitives (`core.test`'s harness and report
/// are built on them) that no module exports and no `.qn` declares. They are members on the same terms
/// all the same, so the one rule covers them too.
///
/// This is the single table the type checker registers from, codegen mangles and dispatches
/// by, and the inert-placeholder test reads — a divergence between those would silently
/// make a user's definition unreachable, or intercept a call the checker resolved to it,
/// which is exactly the class of bug the table exists to prevent.
pub const BUILTIN_OVERLOADS: &[BuiltinOverload] = &[
    BuiltinOverload {
        name: "core.time.now",
        parameters: &[],
        ret: Type::Num,
    },
    // `core.info`'s primitives, each a constant fixed when the program is compiled. The
    // module's public members are ordinary Quilon over these — a sum type cannot be a `ret`
    // here, since `Type::Named` owns a `String`.
    BuiltinOverload {
        name: "__platform",
        parameters: &[],
        ret: Type::Text,
    },
    BuiltinOverload {
        name: "__os",
        parameters: &[],
        ret: Type::Text,
    },
    BuiltinOverload {
        name: "__quilon_version",
        parameters: &[],
        ret: Type::Text,
    },
    BuiltinOverload {
        name: "__pointer_bits",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__is_big_endian",
        parameters: &[],
        ret: Type::Bool,
    },
    BuiltinOverload {
        name: "__is_aot",
        parameters: &[],
        ret: Type::Bool,
    },
    // Terminates the process with an exit code — what `core.test`'s failing `assert`
    // calls. `__`-prefixed to mark it internal: there is no user-facing `exit`.
    BuiltinOverload {
        name: "__exit",
        parameters: &[Type::Num],
        ret: Type::Unit,
    },
    // Whether ANSI styling suits a file descriptor (it is a terminal, `NO_COLOR` is unset,
    // `TERM` is not `dumb`) — how `core.test` decides whether to color a failure report.
    // Internal for the same reason as `__exit`: raw file descriptors are not user-facing
    // surface, since the language's IO direction is `@` leaf primitives.
    BuiltinOverload {
        name: "__color_enabled",
        parameters: &[Type::Num],
        ret: Type::Bool,
    },
    // The test registry (see `is_test_registry_intrinsic`): the harness's event sink and
    // reporter, which `core.test`'s `describe`/`it` and the provided `expect` drive. Enter
    // (by name, reporting the group) and leave a `describe` group, each yielding the
    // resulting nesting depth; read that depth without moving it; ask whether a group or a
    // case is selected for the run; ask whether the running case has already failed; close a
    // case by name, reporting it and yielding its depth; read the two totals back; and
    // report the summary, yielding the run's status. `core.test` wraps the read-only ones as
    // named `.qn` functions, which is what a case reads.
    BuiltinOverload {
        name: "__test_suite_enter",
        parameters: &[Type::Text],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_suite_leave",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_suite_selected",
        parameters: &[Type::Text],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_case_selected",
        parameters: &[Type::Text],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_depth",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_case_failing",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_case_finish",
        parameters: &[Type::Text],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_summary",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_passed",
        parameters: &[],
        ret: Type::Num,
    },
    BuiltinOverload {
        name: "__test_failed",
        parameters: &[],
        ret: Type::Num,
    },
];

/// The two assertion entry points the compiler provides. Both take the value under test and
/// a matcher (see [`MATCHERS`]); they differ only in what a failure does — `assert` reports
/// and exits, `expect` reports, marks the case failed, and lets the suite carry on.
pub const ASSERT: &str = "assert";
pub const EXPECT: &str = "expect";

/// The matchers the compiler provides, in the only position they mean anything: the second
/// argument of an [`ASSERT`]/[`EXPECT`] call. Elsewhere these are ordinary names, free for a
/// program to use.
///
/// Compiler-provided rather than written in `.qn` because a matcher holds a value of the type
/// under test, which without generics would need one matcher type per value type. `not` takes
/// a matcher and negates it; the rest take the value they compare against, or nothing.
pub const MATCHERS: &[&str] = &["equals", "contains", "not", "isOk", "isNotOk"];

/// Whether `name` is `assert` or `expect` — a call the compiler lowers itself.
pub fn is_assertion(name: &str) -> bool {
    name == ASSERT || name == EXPECT
}

/// Whether `name` is one of the provided [`MATCHERS`].
pub fn is_matcher(name: &str) -> bool {
    MATCHERS.contains(&name)
}

/// The sum variant `isOk()` / `isNotOk()` asks about, or `None` for a matcher that reads no
/// variant. Shared by the checker (which requires the value's type to carry that variant) and
/// codegen (which compares against its tag), so the two can never disagree — and named
/// exhaustively, so a matcher added later has to say what it reads rather than inheriting
/// `NotOk`.
pub fn matcher_variant(matcher: &str) -> Option<&'static str> {
    match matcher {
        "isOk" => Some("Ok"),
        "isNotOk" => Some("NotOk"),
        _ => None,
    }
}

/// The prefix marking a test-registry primitive.
const TEST_REGISTRY_PREFIX: &str = "__test_";

/// Whether `name` is one of the test registry's primitives — the event sink and reporter
/// behind `core.test`'s `describe` and `it`, listed among the [`BUILTIN_OVERLOADS`] above.
/// The registry counts, nests, and renders each event per the reporter `quilon test` chose
/// (see `docs/corelib/test/README.md`).
///
/// Every one takes `Text` and `Num` arguments only and yields a `Num`, which is what lets
/// codegen lower the whole family through this one predicate and
/// [`builtin_parameters`]. `__`-prefixed and exported by no module for the same reason as
/// `__exit`: they are the harness's plumbing, not user-facing surface.
pub fn is_test_registry_intrinsic(name: &str) -> bool {
    name.starts_with(TEST_REGISTRY_PREFIX)
}

/// The parameter types the built-in overload member `name` declares, or `None` if the
/// compiler provides no such member.
pub fn builtin_parameters(name: &str) -> Option<&'static [Type]> {
    BUILTIN_OVERLOADS
        .iter()
        .find(|member| member.name == name)
        .map(|member| member.parameters)
}

/// Every name the compiler provides itself, across both builtin tables — the one scan the
/// name predicates below share, so a new table never means a new hand-rolled loop.
fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_OVERLOADS
        .iter()
        .map(|member| member.name)
        .chain(RENDERABLE_BUILTINS.iter().map(|builtin| builtin.name))
}

/// Whether the compiler provides `name` itself (by its exact, post-link spelling). For the
/// bare-named internal primitives (`__exit`, `__test_*`) a single user definition of the
/// name already forms an overload set; the module-qualified names cannot be declared by a
/// user at all.
pub fn is_compiler_provided_name(name: &str) -> bool {
    builtin_names().any(|builtin| builtin == name)
}

/// The arity the built-in `name` claims, or `None` if the compiler provides no `name`. What
/// a built-in claims of a call is an arity question: an overload set's members all share one
/// arity, and an output built-in claims every call at its own.
pub fn builtin_arity(name: &str) -> Option<usize> {
    BUILTIN_OVERLOADS
        .iter()
        .find(|member| member.name == name)
        .map(|member| member.parameters.len())
        .or_else(|| renderable_builtin(name).map(RenderableBuiltin::arity))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub type_annotation: Option<Type>,
    pub span: Span,
}

/// The reserved built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`).
/// When the receiver of one of these is an array, both the type checker and codegen
/// resolve the compiler-provided built-in ahead of any user overload or sum
/// constructor — so this predicate is the single source of truth shared by both passes
/// (a divergence would be a bug). Method names are lowercase, so they never collide with
/// (Capitalized) sum-constructor names.
pub fn is_array_method(name: &str) -> bool {
    matches!(name, "map" | "filter" | "reduce" | "each" | "find" | "at")
}

/// The reserved built-in `Text` methods (`split`/`trim`/`replace`/`contains`/
/// `indexOf`/`slice`/`at`/`graphemes`/`toUpper`/`toLower`). Like [`is_array_method`],
/// these are resolved ahead of any user overload when the receiver is a `Text`, so this
/// predicate is the single source of truth shared by the type checker and codegen.
/// Method names are lowercase/camelCase, so they never collide with (Capitalized)
/// sum-constructor names. (`at` is also an array method; the receiver's type decides.)
pub fn is_text_method(name: &str) -> bool {
    qn_text_impl(name).is_some()
        || matches!(
            name,
            "trimStart"
                | "trimEnd"
                | "indexOf"
                | "slice"
                | "at"
                | "graphemes"
                | "toUpper"
                | "toLower"
        )
}

/// The `core.text` function implementing the Text method `name` — its QUALIFIED name, as
/// the link's rename leaves it — or `None` for a method that is a native primitive (or no
/// Text method at all). The composable methods are written in Quilon (`corelib/text.qn`)
/// over the primitives; a member call to one lowers to a plain call of the named function
/// with the receiver as the first argument. Shared by codegen (the lowering),
/// reachability (a mention of the method reaches the implementation), and the module
/// loader (a program mentioning one gets `core.text` merged in).
pub fn qn_text_impl(name: &str) -> Option<&'static str> {
    Some(match name {
        "split" => "core.text.split",
        "trim" => "core.text.trim",
        "contains" => "core.text.contains",
        "replace" => "core.text.replace",
        "replaceAll" => "core.text.replaceAll",
        "repeat" => "core.text.repeat",
        _ => return None,
    })
}

/// The name of the built-in call-site record type, written `Site` in a signature.
pub const SITE_TYPE_NAME: &str = "Site";

/// The built-in `Site` record's fields, in declaration order — the layout every stage
/// agrees on: the checker registers this type, and codegen fills one in at a call site
/// (see `CodeGenerator::site_value`) in exactly this order.
///
/// `excerpt` is the text of the line the call sits on and `width` how many characters of
/// it the call spans, which is what lets a failure message underline the call with a
/// caret run — the same rendering compiler diagnostics use.
///
/// `line`, `column`, and `width` are always at least 1, so arithmetic on them (a lead of
/// `column - 1` spaces, say) is always well defined. A call the compiler has no source for
/// — a program assembled in memory rather than read from a file — is signalled by an EMPTY
/// `file`, not by a zero position.
pub fn site_fields() -> Vec<(String, Type)> {
    vec![
        ("file".to_string(), Type::Text),
        ("line".to_string(), Type::Num),
        ("column".to_string(), Type::Num),
        ("excerpt".to_string(), Type::Text),
        ("width".to_string(), Type::Num),
    ]
}

/// The built-in `Site` type as the checker resolves it: a named record with
/// [`site_fields`] and no methods.
pub fn site_type() -> Type {
    Type::Named {
        name: SITE_TYPE_NAME.to_string(),
        fields: Rc::new(site_fields()),
        methods: Rc::new(Vec::new()),
    }
}

/// Whether `ty` is the built-in `Site` record — the marker that makes a parameter
/// receive its CALLER's location. A trailing `Site` parameter left off at a call site is
/// filled in by the compiler with that call's `file:line:column`; passing one explicitly
/// forwards the caller's own site instead, which is how a location propagates through a
/// chain of wrappers (a check of your own forwarding its `site` to `failAt`).
pub fn is_site_type(ty: &Type) -> bool {
    matches!(ty, Type::Named { name, .. } if name == SITE_TYPE_NAME)
}

/// Whether `parameters` ends in a `Site` parameter — i.e. the callee wants its caller's
/// location, and a call may leave that last argument off.
pub fn takes_call_site(parameters: &[Type]) -> bool {
    parameters.last().is_some_and(is_site_type)
}

/// Whether a call passing `arg_count` arguments to a callee with `parameters` has its call site
/// FILLED IN: the callee's last parameter is a `Site` and the call left exactly that one
/// argument off.
///
/// The single statement of the filling rule. Every pass that must agree on it comes through
/// here — the checker's arity check and overload matching, codegen's argument lowering, and
/// tail-call detection (a self-call that omits its own trailing `Site` is still a self-call,
/// and must still become a loop).
pub fn fills_call_site(parameters: &[Type], arg_count: usize) -> bool {
    parameters.len() == arg_count + 1 && takes_call_site(parameters)
}

/// The parameters a CALLER sees: a trailing `Site` is filled in by the compiler, so it is
/// never part of the signature a call has to satisfy. Used wherever a signature is shown to
/// a person (an arity error, a candidate list), so no diagnostic asks for an argument the
/// language does not let anyone pass.
pub fn visible_parameters(parameters: &[Type]) -> &[Type] {
    match takes_call_site(parameters) {
        true => &parameters[..parameters.len() - 1],
        false => parameters,
    }
}

/// Whether a callee with `parameters` accepts a call passing `args` arguments: either an
/// exact match, or one argument short of a trailing `Site` the compiler fills in. `matches`
/// compares one parameter against one argument, so an argument is whatever the caller knows
/// about it — a resolved type, a mangling tag, or a type not settled yet.
pub fn parameters_accept<A>(
    parameters: &[Type],
    args: &[A],
    matches: impl Fn(&Type, &A) -> bool,
) -> bool {
    if parameters.len() != args.len() && !fills_call_site(parameters, args.len()) {
        return false;
    }
    parameters
        .iter()
        .zip(args.iter())
        .all(|(p, a)| matches(p, a))
}

/// The reserved built-in `Map` methods (`get`/`has`/`set`/`keys`/`values`/`each`).
/// (`size` is a field, like an array's `.size`, not a method.) Like [`is_array_method`],
/// these are resolved ahead of any user overload when the receiver is a `Map`, so this
/// predicate is the single source of truth shared by the type checker and codegen.
pub fn is_map_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "has" | "set" | "remove" | "keys" | "values" | "each"
    )
}

/// The reserved built-in `Set` methods (`has`/`add`/`items`/`each`). (`size` is a field.)
/// The single source of truth shared by the type checker and codegen, like the array and
/// map method predicates.
pub fn is_set_method(name: &str) -> bool {
    matches!(name, "has" | "add" | "remove" | "items" | "each")
}

/// One piece of an interpolated string (`Expression::Interpolation`): either literal text or a
/// hole expression to render and splice in.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpolationPart {
    Literal(String),
    Hole(Expression),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    // Literals
    Number {
        value: f64,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    },
    /// A string with interpolation holes: literal chunks interleaved with hole
    /// expressions, e.g. `"hi `user.name`!"`. Renders each hole to `Text` via its `` ` ``
    /// operator and concatenates. A plain (hole-free) literal stays an `Expression::String`.
    Interpolation {
        parts: Vec<InterpolationPart>,
        span: Span,
    },
    Bool {
        value: bool,
        span: Span,
    },

    // The unit value `$` — the sole inhabitant of the `Unit` type.
    Unit {
        span: Span,
    },

    // Variables
    Identifier {
        name: String,
        span: Span,
    },

    // Binary operations
    BinaryOperator {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
        span: Span,
    },

    // Unary operations
    UnaryOperator {
        operator: UnaryOperator,
        expression: Box<Expression>,
        span: Span,
    },

    // Function call. `member_call` marks the `recv.name(args)` form, which the parser
    // desugars to `name(recv, args)` with `recv` as the first argument. That form resolves
    // against the RECEIVER's type alone — a built-in method of `recv`'s type, or a method
    // the record/sum declares — never against the top-level namespace.
    Call {
        function: Box<Expression>,
        arguments: Vec<Expression>,
        member_call: bool,
        span: Span,
    },

    // Function literal (lambda / closure): `x => x + 1`, `(a, b) => a + b`, `() => 0`.
    // A first-class value, distinct from a top-level `FunctionDeclaration`. When its body
    // references names bound in an enclosing scope, those are *captured*: a name bound
    // with `=` is captured by value (read-only copy), one bound with `:=` is captured
    // by reference (a shared, mutable GC cell). Capture is inferred entirely from the
    // binding operator — there is no capture list. Closures are monomorphic in M3:
    // parameters/captures are concrete-typed; generic closures are deferred to M4.
    Lambda {
        parameters: Vec<Parameter>,
        return_type: Option<Type>,
        body: Box<Expression>,
        span: Span,
    },

    // Block
    Block {
        statements: Vec<Statement>,
        span: Span,
    },

    // If expression (ternary)
    If {
        condition: Box<Expression>,
        then: Box<Expression>,
        else_: Box<Expression>,
        span: Span,
    },

    // Pattern match
    Match {
        expression: Box<Expression>,
        arms: Vec<MatchArm>,
        span: Span,
    },

    // Field access
    FieldAccess {
        expression: Box<Expression>,
        field: String,
        span: Span,
    },

    // In-place field write: `obj.field := value`. `target` is a `FieldAccess`;
    // it mutates the existing record memory in place rather than re-binding a
    // name. Only allowed when `obj`'s binding is mutable (`:=`); the type checker
    // enforces this. (Nested records aren't representable yet, so the type checker
    // rejects deeper paths like `a.b.c := …` before codegen.)
    FieldAssign {
        target: Box<Expression>,
        value: Box<Expression>,
        span: Span,
    },

    // Array indexing
    Index {
        expression: Box<Expression>,
        index: Box<Expression>,
        span: Span,
    },

    // In-place array element write: `arr[i] := value`. `target` is an `Index`; it
    // mutates the existing array memory in place rather than re-binding a name. Only
    // allowed when `arr`'s binding is mutable (`:=`); the type checker enforces this,
    // and the index is checked exactly like a read.
    IndexAssign {
        target: Box<Expression>,
        value: Box<Expression>,
        span: Span,
    },

    // Array literal
    Array {
        elements: Vec<Expression>,
        span: Span,
    },

    // Map literal, pipe-fenced: `[|"a" => 1, "b" => 2|]`. Empty is `[|=>|]`.
    // Each entry is a (key, value) expression pair. Iteration order is unspecified.
    MapLiteral {
        entries: Vec<(Expression, Expression)>,
        span: Span,
    },

    // Set literal, pipe-fenced: `[|"a", "b"|]`. Empty is `[||]`. The fence keeps a set
    // literal distinct from an array literal (`[1, 2, 3]`). Iteration order unspecified.
    SetLiteral {
        elements: Vec<Expression>,
        span: Span,
    },

    // Record literal
    Record {
        fields: Vec<(String, Expression)>,
        span: Span,
    },

    // Type constructor (e.g., User { name = "Alice", age = 30 })
    Constructor {
        type_name: String,
        fields: Vec<(String, Expression)>,
        span: Span,
    },

    // Inclusive range `lo <- hi`: materialized `[]Num` sugar. `1 <- 4` is
    // `[1, 2, 3, 4]`; when `lo > hi` it descends (`4 <- 1` is `[4, 3, 2, 1]`).
    // There is no distinct Range type — the result IS a `[]Num`, so it composes
    // with array ops / `.size` / indexing.
    Range {
        start: Box<Expression>,
        end: Box<Expression>,
        span: Span,
    },

    // Prefix spread `<-source`, valid ONLY as an element of an array literal
    // (`[<-xs, 4]`) or a field of a record literal (`{<-p, x = 9}`). It splices
    // every element of a source array (array context), or every field of a source
    // record (record functional-update), into the surrounding literal. Disambiguated
    // from the infix range `lo <- hi` purely by position: a `<-` that BEGINS a literal
    // element/field is a spread; a `<-` between two complete expressions is a range.
    Spread {
        expression: Box<Expression>,
        span: Span,
    },
}

impl Expression {
    pub fn span(&self) -> &Span {
        match self {
            Expression::Number { span, .. } => span,
            Expression::String { span, .. } => span,
            Expression::Interpolation { span, .. } => span,
            Expression::Bool { span, .. } => span,
            Expression::Unit { span, .. } => span,
            Expression::Identifier { span, .. } => span,
            Expression::BinaryOperator { span, .. } => span,
            Expression::UnaryOperator { span, .. } => span,
            Expression::Call { span, .. } => span,
            Expression::Lambda { span, .. } => span,
            Expression::Block { span, .. } => span,
            Expression::If { span, .. } => span,
            Expression::Match { span, .. } => span,
            Expression::FieldAccess { span, .. } => span,
            Expression::FieldAssign { span, .. } => span,
            Expression::Index { span, .. } => span,
            Expression::IndexAssign { span, .. } => span,
            Expression::Array { span, .. } => span,
            Expression::MapLiteral { span, .. } => span,
            Expression::SetLiteral { span, .. } => span,
            Expression::Record { span, .. } => span,
            Expression::Constructor { span, .. } => span,
            Expression::Range { span, .. } => span,
            Expression::Spread { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expression,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Identifier {
        name: String,
        span: Span,
    },
    Number {
        value: f64,
        span: Span,
    },
    Constructor {
        name: String,
        arguments: Vec<Pattern>,
        span: Span,
    },
    Wildcard {
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> &Span {
        match self {
            Pattern::Identifier { span, .. } => span,
            Pattern::Number { span, .. } => span,
            Pattern::Constructor { span, .. } => span,
            Pattern::Wildcard { span } => span,
        }
    }

    /// Whether this pattern matches every value of its type: `_`, or a name binding it.
    /// One arm of these covers a whole match, and one of them as a constructor's payload
    /// sub-pattern is what keeps tag-only dispatch honest — the checker asks both questions
    /// here so they cannot drift apart, or from codegen's own always-true test.
    pub fn is_irrefutable(&self) -> bool {
        matches!(self, Pattern::Identifier { .. } | Pattern::Wildcard { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Set intersection, written `+-` / `-+` (symmetric). Distinct from `Add`/`Sub`; only
    // ever applied to `Set` operands (there is no numeric intersection).
    SetIntersect,
    Eq,
    Ne,
    // `<` and `>` double as block delimiters; the parser disambiguates them as
    // comparison operators in operand position (a bare `>` only outside a `< >`
    // block — see `match_comparison`).
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOperator {
    /// The operator's source symbol, which doubles as its overload-set name (an
    /// operator is just a named overload set under the hood). Shared by the type
    /// checker and codegen so a user operator overload is keyed identically in both.
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOperator::Add => "+",
            BinaryOperator::Sub => "-",
            BinaryOperator::Mul => "*",
            BinaryOperator::Div => "/",
            BinaryOperator::Mod => "%",
            BinaryOperator::SetIntersect => "+-",
            BinaryOperator::Eq => "==",
            BinaryOperator::Ne => "!=",
            BinaryOperator::Lt => "<",
            BinaryOperator::Le => "<=",
            BinaryOperator::Gt => ">",
            BinaryOperator::Ge => ">=",
            BinaryOperator::And => "&&",
            BinaryOperator::Or => "||",
        }
    }
}

/// A literal `Num` expression's value: a `Number`, or a negated one (`-2` parses as
/// `Neg(2)`). `None` for anything computed. Shared by the type checker, which rejects a
/// bad literal where a built-in has a contract, and codegen, which emits a constant
/// instead of the runtime check that literal has already passed.
pub fn literal_number(expression: &Expression) -> Option<f64> {
    match expression {
        Expression::Number { value, .. } => Some(*value),
        Expression::UnaryOperator {
            operator: UnaryOperator::Neg,
            expression,
            ..
        } => match expression.as_ref() {
            Expression::Number { value, .. } => Some(-value),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `name` is an operator symbol — and thus always an overload set, never a
/// plain value binding. Shared by the type checker and the code generator so both
/// agree on exactly which names are operators (the binary operator symbols).
pub fn is_operator_symbol(name: &str) -> bool {
    matches!(
        name,
        "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Num,
    Text,
    Bool,
    // The unit type, written `$`. Has exactly one value (also `$`). Used for
    // side-effecting expressions/functions whose result is meaningless.
    Unit,
    Array(Box<Type>),
    // Built-in parametric collections (like `[]T`, NOT user generics):
    // `Map(key, value)` = `[|K => V|]`; `Set(elem)` = `[|T|]`.
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Record(Vec<(String, Type)>), // For anonymous records
    /// A user-declared record type. Its `fields` and `methods` are behind an `Rc` because
    /// a `Type` is cloned once per expression that has this type — into the type table, out
    /// of it in codegen, through every inference step — and the declaration itself never
    /// changes after the checker builds it. Sharing turns each of those clones from a deep
    /// copy of the whole field list (a `String` allocation per field name, plus the nested
    /// field types) into a reference-count bump.
    Named {
        name: String,
        fields: Rc<Vec<(String, Type)>>,
        methods: Rc<Vec<String>>, // Method names (bodies stored elsewhere)
    },
    Generic {
        name: String,
        arguments: Vec<Type>,
    },
    Function {
        parameters: Vec<Type>,
        return_type: Box<Type>,
    },
    // Sum types (algebraic data types)
    Sum {
        name: String,
        variants: Vec<SumVariant>,
    },
}

impl Type {
    /// A named type known only by its name, with no field or method list — what the
    /// parser produces for a capitalized annotation before the checker substitutes the
    /// declaration, and what codegen uses where only the name matters. An empty field
    /// list is the marker for "not yet resolved", so the checker tests for it.
    pub fn named_ref(name: impl Into<String>) -> Type {
        Type::Named {
            name: name.into(),
            fields: Rc::new(Vec::new()),
            methods: Rc::new(Vec::new()),
        }
    }

    /// Whether this type carries an unresolved payload type variable (`Type::Generic`)
    /// anywhere — in practice only the built-in `Result`'s `Ok(T)`/`NotOk(E)`. Used by
    /// the type checker (to refine a generic return annotation) and codegen (to defer a
    /// generic return to the oracle's concrete body type).
    pub fn contains_generic(&self) -> bool {
        match self {
            Type::Generic { .. } => true,
            Type::Array(inner) => inner.contains_generic(),
            Type::Map(k, v) => k.contains_generic() || v.contains_generic(),
            Type::Set(inner) => inner.contains_generic(),
            Type::Sum { variants, .. } => variants
                .iter()
                .any(|v| v.fields.iter().any(Type::contains_generic)),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SumVariant {
    pub name: String,
    pub fields: Vec<Type>,
}

/// A short, user-facing label for a type (`Num`, `Text`, `[]Text`, a user type's name).
/// Shared by the type checker's overload diagnostics and codegen's entry-point
/// signature diagnostic, so both render types the same way. A not-yet-concrete
/// `Generic` (an unresolved sum payload such as the `T` in `Ok(T)`) renders as
/// `<unknown>`.
pub fn type_label(ty: &Type) -> String {
    match ty {
        Type::Num => "Num".to_string(),
        Type::Text => "Text".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Unit => "$".to_string(),
        Type::Array(elem) => format!("[]{}", type_label(elem)),
        Type::Map(k, v) => format!("[|{} => {}|]", type_label(k), type_label(v)),
        Type::Set(elem) => format!("[|{}|]", type_label(elem)),
        Type::Named { name, .. } | Type::Sum { name, .. } => name.clone(),
        Type::Function {
            parameters,
            return_type,
        } => {
            let rendered: Vec<String> = parameters.iter().map(type_label).collect();
            format!("({}) -> {}", rendered.join(", "), type_label(return_type))
        }
        Type::Generic { .. } => "<unknown>".to_string(),
        other => format!("{:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_test_registry_primitive_takes_text_or_num_and_yields_a_num() {
        // Codegen lowers the whole family through one path, which only works while they all
        // keep to the parameter types it knows how to pass and the one result it reads.
        let registry: Vec<&BuiltinOverload> = BUILTIN_OVERLOADS
            .iter()
            .filter(|member| is_test_registry_intrinsic(member.name))
            .collect();
        assert!(!registry.is_empty(), "the registry has no members at all");
        for member in registry {
            for parameter in member.parameters {
                assert!(
                    matches!(parameter, Type::Text | Type::Num),
                    "`{}` takes a {parameter:?}, which codegen cannot pass",
                    member.name
                );
            }
            assert_eq!(member.ret, Type::Num, "`{}` must yield a Num", member.name);
        }
    }
}

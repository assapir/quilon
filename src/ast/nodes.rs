// AST node definitions

use crate::lexer::Span;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

/// A module import: `<< core.io` (built-in dotted) or `<< "path/to/mod.ql"` (file path).
/// NOTE: parsing of imports is implemented in Workstream B1; for now `imports` is always empty.
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

#[derive(Debug, Clone, PartialEq)]
// The `*Decl` suffix mirrors the AST node names (VarDecl/FunctionDecl/TypeDecl);
// renaming would churn the whole codebase for no clarity gain.
#[allow(clippy::enum_variant_names)]
pub enum Item {
    VarDecl(VarDecl),
    FunctionDecl(FunctionDecl),
    TypeDecl(TypeDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub type_def: TypeDef,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    /// A user-defined sum type: `Color = Red / Green / Blue`,
    /// `Shape = Circle(Num) / Rect(Num, Num)`. Variants are separated by `/`.
    Sum(Vec<SumVariant>),
    Record {
        fields: Vec<(String, Type)>,
        methods: Vec<MethodDecl>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub name: String,
    pub params: Vec<Param>, // Does not include implicit "it" parameter
    pub return_type: Option<Type>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Item(Item),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub mutable: bool,
    pub name: String,
    pub type_annotation: Option<Type>,
    pub value: Expr,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Expr,
    /// `>>`-marked top-level items are exported from their module (Workstream B1).
    pub exported: bool,
    pub span: Span,
}

impl FunctionDecl {
    /// Whether this is the inert `core.io` `print`/`eprint` placeholder: a single
    /// UNannotated parameter with an inert body. The compiler fully provides
    /// `print`/`eprint` as built-in overloads (lowered to runtime intrinsics), so the
    /// placeholder is ignored everywhere — neither registered as a user overload nor
    /// type-checked / emitted. A genuine user `print`/`eprint` overload has fully
    /// annotated parameters and is therefore NOT a placeholder. Shared by the type
    /// checker and codegen so the two never disagree on what to skip.
    pub fn is_inert_io_placeholder(&self) -> bool {
        (self.name == "print" || self.name == "eprint")
            && self.params.len() == 1
            && self.params[0].type_annotation.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
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
/// `indexOf`/`slice`/`toUpper`/`toLower`). Like [`is_array_method`], these are
/// resolved ahead of any user overload when the receiver is a `Text`, so this
/// predicate is the single source of truth shared by the type checker and codegen.
/// Method names are lowercase/camelCase, so they never collide with (Capitalized)
/// sum-constructor names.
pub fn is_text_method(name: &str) -> bool {
    matches!(
        name,
        "split"
            | "trim"
            | "trimStart"
            | "trimEnd"
            | "replace"
            | "replaceAll"
            | "contains"
            | "indexOf"
            | "slice"
            | "toUpper"
            | "toLower"
            | "repeat"
    )
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
/// chain of wrappers (`assertEq` -> `assert`).
pub fn is_site_type(ty: &Type) -> bool {
    matches!(ty, Type::Named { name, .. } if name == SITE_TYPE_NAME)
}

/// Whether `params` ends in a `Site` parameter — i.e. the callee wants its caller's
/// location, and a call may leave that last argument off.
pub fn takes_call_site(params: &[Type]) -> bool {
    params.last().is_some_and(is_site_type)
}

/// Whether a call passing `arg_count` arguments to a callee with `params` has its call site
/// FILLED IN: the callee's last parameter is a `Site` and the call left exactly that one
/// argument off.
///
/// The single statement of the filling rule. Every pass that must agree on it comes through
/// here — the checker's arity check and overload matching, codegen's argument lowering, and
/// tail-call detection (a self-call that omits its own trailing `Site` is still a self-call,
/// and must still become a loop).
pub fn fills_call_site(params: &[Type], arg_count: usize) -> bool {
    params.len() == arg_count + 1 && takes_call_site(params)
}

/// The parameters a CALLER sees: a trailing `Site` is filled in by the compiler, so it is
/// never part of the signature a call has to satisfy. Used wherever a signature is shown to
/// a person (an arity error, a candidate list), so no diagnostic asks for an argument the
/// language does not let anyone pass.
pub fn visible_params(params: &[Type]) -> &[Type] {
    match takes_call_site(params) {
        true => &params[..params.len() - 1],
        false => params,
    }
}

/// Whether a callee with `params` accepts a call passing `args` argument types: either an
/// exact match, or one argument short of a trailing `Site` the compiler fills in. `matches`
/// compares one parameter against one argument (the checker and codegen each pass their
/// own comparison — resolved types vs. mangling tags).
pub fn params_accept(
    params: &[Type],
    args: &[Type],
    matches: impl Fn(&Type, &Type) -> bool,
) -> bool {
    if params.len() != args.len() && !fills_call_site(params, args.len()) {
        return false;
    }
    params.iter().zip(args.iter()).all(|(p, a)| matches(p, a))
}

/// The reserved built-in `Map` methods (`get`/`has`/`set`/`keys`/`values`/`each`).
/// (`size` is a field, like an array's `.size`, not a method.) Like [`is_array_method`],
/// these are resolved ahead of any user overload when the receiver is a `Map`, so this
/// predicate is the single source of truth shared by the type checker and codegen.
pub fn is_map_method(name: &str) -> bool {
    matches!(name, "get" | "has" | "set" | "keys" | "values" | "each")
}

/// The reserved built-in `Set` methods (`has`/`add`/`items`/`each`). (`size` is a field.)
/// The single source of truth shared by the type checker and codegen, like the array and
/// map method predicates.
pub fn is_set_method(name: &str) -> bool {
    matches!(name, "has" | "add" | "items" | "each")
}

/// One piece of an interpolated string (`Expr::Interpolation`): either literal text or a
/// hole expression to render and splice in.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Lit(String),
    Hole(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
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
    /// operator and concatenates. A plain (hole-free) literal stays an `Expr::String`.
    Interpolation {
        parts: Vec<InterpPart>,
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
    Ident {
        name: String,
        span: Span,
    },

    // Binary operations
    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
        span: Span,
    },

    // Unary operations
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },

    // Function call
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },

    // Function literal (lambda / closure): `x => x + 1`, `(a, b) => a + b`, `() => 0`.
    // A first-class value, distinct from a top-level `FunctionDecl`. When its body
    // references names bound in an enclosing scope, those are *captured*: a name bound
    // with `=` is captured by value (read-only copy), one bound with `:=` is captured
    // by reference (a shared, mutable GC cell). Capture is inferred entirely from the
    // binding operator — there is no capture list. Closures are monomorphic in M3:
    // params/captures are concrete-typed; generic closures are deferred to M4.
    Lambda {
        params: Vec<Param>,
        return_type: Option<Type>,
        body: Box<Expr>,
        span: Span,
    },

    // Pipeline
    Pipeline {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },

    // Block
    Block {
        stmts: Vec<Statement>,
        span: Span,
    },

    // If expression (ternary)
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
        span: Span,
    },

    // Pattern match
    Match {
        expr: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },

    // Field access
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        span: Span,
    },

    // In-place field write: `obj.field := value`. `target` is a `FieldAccess`;
    // it mutates the existing record memory in place rather than re-binding a
    // name. Only allowed when `obj`'s binding is mutable (`:=`); the type checker
    // enforces this. (Nested records aren't representable yet, so the type checker
    // rejects deeper paths like `a.b.c := …` before codegen.)
    FieldAssign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },

    // Array indexing
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },

    // Array literal
    Array {
        elements: Vec<Expr>,
        span: Span,
    },

    // Map literal, pipe-fenced: `[|"a" => 1, "b" => 2|]`. Empty is `[|=>|]`.
    // Each entry is a (key, value) expression pair. Iteration order is unspecified.
    MapLit {
        entries: Vec<(Expr, Expr)>,
        span: Span,
    },

    // Set literal, pipe-fenced: `[|"a", "b"|]`. Empty is `[||]`. The fence keeps a set
    // literal distinct from an array literal (`[1, 2, 3]`). Iteration order unspecified.
    SetLit {
        elements: Vec<Expr>,
        span: Span,
    },

    // Record literal
    Record {
        fields: Vec<(String, Expr)>,
        span: Span,
    },

    // Type constructor (e.g., User { name = "Alice", age = 30 })
    Constructor {
        type_name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },

    // Inclusive range `lo <- hi`: materialized `[]Num` sugar. `1 <- 4` is
    // `[1, 2, 3, 4]`; when `lo > hi` it descends (`4 <- 1` is `[4, 3, 2, 1]`).
    // There is no distinct Range type — the result IS a `[]Num`, so it composes
    // with array ops / `.size` / indexing.
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },

    // Prefix spread `<-source`, valid ONLY as an element of an array literal
    // (`[<-xs, 4]`) or a field of a record literal (`{<-p, x = 9}`). It splices
    // every element of a source array (array context), or every field of a source
    // record (record functional-update), into the surrounding literal. Disambiguated
    // from the infix range `lo <- hi` purely by position: a `<-` that BEGINS a literal
    // element/field is a spread; a `<-` between two complete expressions is a range.
    Spread {
        expr: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Number { span, .. } => span,
            Expr::String { span, .. } => span,
            Expr::Interpolation { span, .. } => span,
            Expr::Bool { span, .. } => span,
            Expr::Unit { span, .. } => span,
            Expr::Ident { span, .. } => span,
            Expr::BinOp { span, .. } => span,
            Expr::UnaryOp { span, .. } => span,
            Expr::Call { span, .. } => span,
            Expr::Lambda { span, .. } => span,
            Expr::Pipeline { span, .. } => span,
            Expr::Block { span, .. } => span,
            Expr::If { span, .. } => span,
            Expr::Match { span, .. } => span,
            Expr::FieldAccess { span, .. } => span,
            Expr::FieldAssign { span, .. } => span,
            Expr::Index { span, .. } => span,
            Expr::Array { span, .. } => span,
            Expr::MapLit { span, .. } => span,
            Expr::SetLit { span, .. } => span,
            Expr::Record { span, .. } => span,
            Expr::Constructor { span, .. } => span,
            Expr::Range { span, .. } => span,
            Expr::Spread { span, .. } => span,
        }
    }

    /// Desugar a pipeline `left |> right` into the equivalent call, injecting
    /// `left` as the FIRST argument of the right-hand call:
    ///   `x |> f`      => `f(x)`
    ///   `x |> f(a, b)` => `f(x, a, b)`
    /// Used by both the type checker and codegen so the two never diverge.
    pub fn desugar_pipeline(left: &Expr, right: &Expr, span: &Span) -> Expr {
        let (func, mut args) = match right {
            Expr::Call { func, args, .. } => ((**func).clone(), args.clone()),
            other => (other.clone(), Vec::new()),
        };
        args.insert(0, left.clone());
        Expr::Call {
            func: Box::new(func),
            args,
            span: span.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Ident {
        name: String,
        span: Span,
    },
    Number {
        value: f64,
        span: Span,
    },
    Constructor {
        name: String,
        args: Vec<Pattern>,
        span: Span,
    },
    Wildcard {
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> &Span {
        match self {
            Pattern::Ident { span, .. } => span,
            Pattern::Number { span, .. } => span,
            Pattern::Constructor { span, .. } => span,
            Pattern::Wildcard { span } => span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
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

impl BinOp {
    /// The operator's source symbol, which doubles as its overload-set name (an
    /// operator is just a named overload set under the hood). Shared by the type
    /// checker and codegen so a user operator overload is keyed identically in both.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::SetIntersect => "+-",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
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
pub enum UnaryOp {
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
        args: Vec<Type>,
    },
    Function {
        params: Vec<Type>,
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
        Type::Generic { .. } => "<unknown>".to_string(),
        other => format!("{:?}", other),
    }
}

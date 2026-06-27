// AST node definitions

use crate::lexer::Span;

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
    #[allow(dead_code)]
    Alias(Type),
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

    // Sum type constructor call (e.g., Some(42), OK("value"), NotOK).
    // Reserved AST surface: constructor calls currently flow through `Call`, so this
    // variant is matched-but-not-yet-built. Kept for the planned dedicated lowering.
    #[allow(dead_code)]
    SumConstructor {
        variant: String,
        args: Vec<Expr>,
        span: Span,
    },

    // For loop (for pattern <- collection => body)
    ForLoop {
        collection: Box<Expr>,
        pattern: ForPattern,
        body: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Number { span, .. } => span,
            Expr::String { span, .. } => span,
            Expr::Bool { span, .. } => span,
            Expr::Unit { span, .. } => span,
            Expr::Ident { span, .. } => span,
            Expr::BinOp { span, .. } => span,
            Expr::UnaryOp { span, .. } => span,
            Expr::Call { span, .. } => span,
            Expr::Pipeline { span, .. } => span,
            Expr::Block { span, .. } => span,
            Expr::If { span, .. } => span,
            Expr::Match { span, .. } => span,
            Expr::FieldAccess { span, .. } => span,
            Expr::FieldAssign { span, .. } => span,
            Expr::Index { span, .. } => span,
            Expr::Array { span, .. } => span,
            Expr::Record { span, .. } => span,
            Expr::Constructor { span, .. } => span,
            Expr::SumConstructor { span, .. } => span,
            Expr::ForLoop { span, .. } => span,
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

/// Pattern for for loops - supports both `item` and `(item, index)`
#[derive(Debug, Clone, PartialEq)]
pub enum ForPattern {
    /// Single binding: item
    Item { name: String, span: Span },
    /// Tuple binding: (item, index)
    ItemIndex {
        item: String,
        index: String,
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
    Record(Vec<(String, Type)>), // For anonymous records
    Named {
        name: String,
        fields: Vec<(String, Type)>,
        methods: Vec<String>, // Method names (bodies stored elsewhere)
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

#[derive(Debug, Clone, PartialEq)]
pub struct SumVariant {
    pub name: String,
    pub fields: Vec<Type>,
}

// AST node definitions

use crate::lexer::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    VarDecl(VarDecl),
    FunctionDecl(FunctionDecl),
    TypeDecl(TypeDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub type_def: TypeDef,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    Sum(Vec<SumVariant>),
    Record(Vec<(String, Type)>),
    Alias(Type),
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
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Expr,
    pub span: Span,
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
    Number { value: f64, span: Span },
    String { value: String, span: Span },
    Bool { value: bool, span: Span },
    
    // Variables
    Ident { name: String, span: Span },
    
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
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::Number { span, .. } => span,
            Expr::String { span, .. } => span,
            Expr::Bool { span, .. } => span,
            Expr::Ident { span, .. } => span,
            Expr::BinOp { span, .. } => span,
            Expr::UnaryOp { span, .. } => span,
            Expr::Call { span, .. } => span,
            Expr::Pipeline { span, .. } => span,
            Expr::Block { span, .. } => span,
            Expr::If { span, .. } => span,
            Expr::Match { span, .. } => span,
            Expr::FieldAccess { span, .. } => span,
            Expr::Array { span, .. } => span,
            Expr::Record { span, .. } => span,
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
    Ident { name: String, span: Span },
    Number { value: f64, span: Span },
    Constructor {
        name: String,
        args: Vec<Pattern>,
        span: Span,
    },
    Wildcard { span: Span },
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
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Num,
    String,
    Bool,
    Array(Box<Type>),
    Record(Vec<(String, Type)>),
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

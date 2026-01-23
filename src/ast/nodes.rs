// AST node definitions

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    VarDecl(VarDecl),
    FunctionDecl(FunctionDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub mutable: bool,
    pub name: String,
    pub type_annotation: Option<Type>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub type_annotation: Option<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // Literals
    Number(f64),
    String(String),
    Bool(bool),
    
    // Variables
    Ident(String),
    
    // Binary operations
    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    
    // Unary operations
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    
    // Function call
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    
    // Pipeline
    Pipeline {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    
    // Block
    Block(Vec<Expr>),
    
    // If expression (ternary)
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    
    // Pattern match
    Match {
        expr: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    
    // Field access
    FieldAccess {
        expr: Box<Expr>,
        field: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Ident(String),
    Constructor {
        name: String,
        args: Vec<Pattern>,
    },
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
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
}

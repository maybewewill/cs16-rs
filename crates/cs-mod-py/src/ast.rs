//! Python 3 Abstract Syntax Tree nodes for `cs-mod-py`.

#[derive(Debug, Clone)]
pub struct Program {
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, Default)]
pub struct Params {
    pub positional: Vec<Param>,
    pub vararg: Option<String>,
    pub kwarg: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExceptHandler {
    pub exc_type: Option<Expr>,
    pub name: Option<String>,
    pub body: Vec<Stmt>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum Target {
    Name(String),
    Attr {
        value: Box<Expr>,
        attr: String,
    },
    Subscript {
        value: Box<Expr>,
        index: Box<Expr>,
    },
    Tuple(Vec<Target>),
    Starred(Box<Target>),
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Import(Vec<(String, Option<String>)>),
    FromImport {
        module: String,
        names: Vec<(String, Option<String>)>,
        star: bool,
    },
    FuncDef {
        name: String,
        params: Params,
        decorators: Vec<Expr>,
        body: Vec<Stmt>,
    },
    ClassDef {
        name: String,
        bases: Vec<Expr>,
        decorators: Vec<Expr>,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Assign {
        targets: Vec<Target>,
        value: Expr,
    },
    AugAssign {
        target: Target,
        op: BinOp,
        value: Expr,
    },
    If {
        test: Expr,
        body: Vec<Stmt>,
        elifs: Vec<(Expr, Vec<Stmt>)>,
        orelse: Vec<Stmt>,
    },
    While {
        test: Expr,
        body: Vec<Stmt>,
        orelse: Vec<Stmt>,
    },
    For {
        target: Target,
        iter: Expr,
        body: Vec<Stmt>,
        orelse: Vec<Stmt>,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        orelse: Vec<Stmt>,
        finalbody: Vec<Stmt>,
    },
    Raise(Option<Expr>),
    Assert {
        test: Expr,
        msg: Option<Expr>,
    },
    Delete(Vec<Target>),
    Global(Vec<String>),
    Nonlocal(Vec<String>),
    Pass,
    Break,
    Continue,
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub line: usize,
    pub kind: ExprKind,
}

#[derive(Debug, Clone)]
pub enum FStringPart {
    Literal(String),
    Formatted {
        expr: Expr,
        format_spec: Option<String>,
        conversion: Option<char>,
    },
}

#[derive(Debug, Clone)]
pub struct Comprehension {
    pub target: Target,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum CallArg {
    Positional(Expr),
    Starred(Expr),
    Keyword(String, Expr),
    KwStarred(Expr),
}

#[derive(Debug, Clone)]
pub enum SlicePart {
    Index(Expr),
    Slice {
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    In,
    NotIn,
    Is,
    IsNot,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    FString(Vec<FStringPart>),
    Name(String),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    Dict(Vec<(Expr, Expr)>),
    ListComp {
        elt: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    SetComp {
        elt: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    DictComp {
        key: Box<Expr>,
        value: Box<Expr>,
        generators: Vec<Comprehension>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    BoolOp {
        op: BoolOp,
        values: Vec<Expr>,
    },
    Compare {
        left: Box<Expr>,
        ops: Vec<CmpOp>,
        comparators: Vec<Expr>,
    },
    IfExp {
        test: Box<Expr>,
        body: Box<Expr>,
        orelse: Box<Expr>,
    },
    Lambda {
        params: Params,
        body: Box<Expr>,
    },
    Call {
        func: Box<Expr>,
        args: Vec<CallArg>,
    },
    Attribute {
        value: Box<Expr>,
        attr: String,
    },
    Subscript {
        value: Box<Expr>,
        slice: Box<SlicePart>,
    },
    Named {
        target: String,
        value: Box<Expr>,
    },
    Starred(Box<Expr>),
}

//! Abstract Syntax Tree for `CodeSceneScript` (`cs1`).

use std::sync::Arc;

use crate::model::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<TopItem>,
}

#[derive(Debug, Clone)]
pub enum TopItem {
    Fn(FnDecl),
    GlobalLet {
        name: String,
        value: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
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

#[derive(Debug, Clone)]
pub enum LValue {
    Var {
        name: String,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: String,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        value: Expr,
        span: Span,
    },
    Assign {
        target: LValue,
        op: AssignOp,
        value: Expr,
        span: Span,
    },
    If {
        cond: Expr,
        then_block: Vec<Stmt>,
        else_block: Option<Vec<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    ForRange {
        var: String,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Vec<Stmt>,
        span: Span,
    },
    ForEach {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
    Block {
        stmts: Vec<Stmt>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Null(Span),
    Bool(bool, Span),
    Num(f64, Span),
    Str(Arc<str>, Span),
    Ident(String, Span),
    List(Vec<Expr>, Span),
    Map(Vec<(String, Expr)>, Span),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Call {
        callee: String,
        args: Vec<Expr>,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: String,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Self::Null(s)
            | Self::Bool(_, s)
            | Self::Num(_, s)
            | Self::Str(_, s)
            | Self::Ident(_, s)
            | Self::List(_, s)
            | Self::Map(_, s)
            | Self::Unary { span: s, .. }
            | Self::Binary { span: s, .. }
            | Self::Call { span: s, .. }
            | Self::MethodCall { span: s, .. }
            | Self::Field { span: s, .. }
            | Self::Index { span: s, .. } => *s,
        }
    }
}

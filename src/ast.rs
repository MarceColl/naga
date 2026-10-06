//! The program as parsed. Positions are 1-based lines and UTF-16 columns.
use crate::value::{Name, Value};
use indexmap::IndexMap;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Eq, Ne, Lt, Gt, Le, Ge, Within, Cat, Add, Sub, Over, Join, Meet, Diff, Apply, Mul, Div, Rem,
}

impl BinOp {
    pub fn glyph(self) -> &'static str {
        use BinOp::*;
        match self {
            Eq => "==", Ne => "≠", Lt => "<", Gt => ">", Le => "≤", Ge => "≥", Within => "⊑", Cat => "++",
            Add => "+", Sub => "-", Over => "⫽", Join => "⊔", Meet => "⊓", Diff => "∆", Apply => "⍎",
            Mul => "*", Div => "/", Rem => "%",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Len, Sum, Min, Max, Abs, Round, Floor, Ceil, Head, Tail, Last, Push, Reverse, Range, Str, Keys,
}

impl Builtin {
    pub const ALL: [(&'static str, Builtin); 16] = [
        ("len", Builtin::Len), ("sum", Builtin::Sum), ("min", Builtin::Min), ("max", Builtin::Max),
        ("abs", Builtin::Abs), ("round", Builtin::Round), ("floor", Builtin::Floor), ("ceil", Builtin::Ceil),
        ("head", Builtin::Head), ("tail", Builtin::Tail), ("last", Builtin::Last), ("push", Builtin::Push),
        ("reverse", Builtin::Reverse), ("range", Builtin::Range), ("str", Builtin::Str), ("keys", Builtin::Keys),
    ];
    pub fn named(n: &str) -> Option<Builtin> {
        Self::ALL.iter().find(|(k, _)| *k == n).map(|(_, b)| *b)
    }
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, b)| *b == self).unwrap().0
    }
}

#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Lit(Value),
    Now,
    Field(Name),
    /// `^name`, `up` records out.
    Outer(usize, Name),
    Get(Box<Expr>, Name),
    Proj(Box<Expr>, Vec<Name>),
    List(Vec<Expr>),
    Rec(Vec<(Name, Expr)>),
    Interp(Vec<Part>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Call(Builtin, Vec<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Has,
    Take,
    Absent,
}

#[derive(Clone, Debug)]
pub struct PatEntry {
    pub name: Name,
    pub mode: Mode,
    pub sub: Option<Vec<PatEntry>>,
    pub lit: Option<Value>,
}

/// `name` or `name: expr`; a bare name means `true`.
#[derive(Clone, Debug)]
pub struct FieldInit {
    pub name: Name,
    pub expr: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum Param {
    Lit(Value),
    Expr(Expr),
}

/// A where pattern and guard as one SQL condition.
#[derive(Clone, Debug)]
pub struct Where {
    pub sql: String,
    pub params: Vec<Param>,
}

pub type Body = Rc<[Stmt]>;

#[derive(Clone, Debug)]
pub enum StmtKind {
    Call(Name),
    Add(Vec<FieldInit>),
    Del(Vec<Name>),
    Pick(Vec<Name>),
    Set(Name, Expr),
    Inc(Name, f64),
    Rename(Name, Name),
    Wrap(Name, Vec<Name>),
    Unwrap(Name),
    Over(Vec<FieldInit>),
    Unify(Vec<FieldInit>),
    Focus(Name, Body),
    Par(Vec<Body>),
    Say(Expr),
    Raise(Expr),
    Show,
    Break,
    Hole(String),
    Table { table: Name, op: Name, pattern: Option<Vec<PatEntry>>, guard: Option<Expr>, compiled: Option<Where> },
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

impl Stmt {
    /// The reference implementation's statement kind names, for traces.
    pub fn kind_name(&self) -> &'static str {
        use StmtKind::*;
        match &self.kind {
            Call(_) => "call", Add(_) => "add", Del(_) => "del", Pick(_) => "pick", Set(..) => "set", Inc(..) => "inc",
            Rename(..) => "rename", Wrap(..) => "wrap", Unwrap(_) => "unwrap", Over(_) => "over", Unify(_) => "unify",
            Focus(..) => "focus", Par(_) => "par", Say(_) => "say", Raise(_) => "raise", Show => "show", Break => "break",
            Hole(_) => "hole", Table { .. } => "tcall",
        }
    }
    pub fn name(&self) -> Option<&Name> {
        match &self.kind {
            StmtKind::Call(n) | StmtKind::Set(n, _) | StmtKind::Inc(n, _) => Some(n),
            _ => None,
        }
    }
}

/// Every statement, depth first: a statement before the ones nested in it.
pub fn walk<'a>(body: &'a [Stmt], f: &mut impl FnMut(&'a Stmt)) {
    for s in body {
        f(s);
        match &s.kind {
            StmtKind::Focus(_, b) => walk(b, f),
            StmtKind::Par(bs) => bs.iter().for_each(|b| walk(b, f)),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct Clause {
    pub pattern: Option<Vec<PatEntry>>,
    pub guard: Option<Expr>,
    pub guard_src: Option<String>,
    pub body: Body,
    pub line: usize,
    pub head_src: String,
    /// The clause's source chunk: the checker caches by its text and reports lines relative to it.
    pub chunk_line: usize,
    pub chunk_text: String,
}

#[derive(Clone, Debug)]
pub struct Word {
    pub name: Name,
    pub clauses: Vec<Clause>,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColType {
    Int,
    Num,
    Text,
    Bool,
    Const,
    Json,
}

impl ColType {
    pub const ALL: [(&'static str, ColType); 6] =
        [("Int", ColType::Int), ("Num", ColType::Num), ("Text", ColType::Text), ("Bool", ColType::Bool), ("Const", ColType::Const), ("Json", ColType::Json)];
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(_, c)| *c == self).unwrap().0
    }
    pub fn sql(self) -> &'static str {
        match self {
            ColType::Int | ColType::Bool => "INTEGER",
            ColType::Num => "REAL",
            _ => "TEXT",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Col {
    pub name: Name,
    pub ty: ColType,
    pub opt: bool,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub name: Name,
    pub single: Name,
    pub cols: Vec<Col>,
    pub line: usize,
}

impl Table {
    pub fn col(&self, n: &str) -> Option<&Col> {
        self.cols.iter().find(|c| &*c.name == n)
    }
}

pub const TABLE_OPS: [&str; 6] = ["all", "get", "where", "insert", "update", "delete"];

#[derive(Clone, Debug)]
pub struct Seed {
    pub table: Name,
    pub expr: Expr,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

#[derive(Clone, Debug)]
pub struct Hole {
    pub line: usize,
    pub note: String,
    pub word: Name,
}

#[derive(Debug, Default)]
pub struct Program {
    /// In order of first definition.
    pub words: IndexMap<Name, Word>,
    pub tables: IndexMap<Name, Table>,
    pub seeds: Vec<Seed>,
    pub errors: Vec<Diag>,
    pub warnings: Vec<Diag>,
    pub holes: Vec<Hole>,
    pub lines: Vec<String>,
}

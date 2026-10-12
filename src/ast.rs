//! The program as parsed. Positions are 1-based lines and scalar columns.
use crate::value::{Name, Value};
use indexmap::IndexMap;
use std::sync::Arc;

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
    Call(&'static crate::funcs::Func, Vec<Expr>),
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

pub type Body = Arc<[Stmt]>;

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
    /// Just past the statement's last token.
    pub end: Pos,
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

/// Names and raw values: path params, query fields.
pub type Pairs = Vec<(String, String)>;

pub const VERBS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// A route field's type: a column type, a list of one, or a nested record.
#[derive(Clone, Debug)]
pub enum FieldTy {
    Col(ColType),
    List(Box<FieldTy>),
    Rec(Vec<FieldDecl>),
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub name: Name,
    pub ty: FieldTy,
    pub opt: bool,
}

impl std::fmt::Display for FieldTy {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FieldTy::Col(c) => write!(f, "{}", c.name()),
            FieldTy::List(t) => write!(f, "[{t}]"),
            FieldTy::Rec(fs) => write!(f, "{{{}}}", fs.iter().map(|x| x.show(": ")).collect::<Vec<_>>().join(", ")),
        }
    }
}

impl FieldDecl {
    /// `name: Type?` in a body, `name:Type?` in a path or query.
    pub fn show(&self, sep: &str) -> String {
        format!("{}{sep}{}{}", self.name, self.ty, if self.opt { "?" } else { "" })
    }
}

#[derive(Clone, Debug)]
pub enum Seg {
    Lit(String),
    Param(FieldDecl),
}

/// `route VERB /path/:id:Int?q:Text {body: Type} = word`: params land at the top level, the body in `body`.
#[derive(Clone, Debug)]
pub struct Route {
    pub verb: Name,
    /// As written, before the query: `/accounts/:id:Int`.
    pub path: String,
    pub segs: Vec<Seg>,
    pub query: Vec<FieldDecl>,
    pub body: Option<Vec<FieldDecl>>,
    pub word: Name,
    pub line: usize,
}

impl Route {
    /// The path as axum matches it: `/accounts/{id}`.
    pub fn pattern(&self) -> String {
        self.join(|c| format!("{{{}}}", c.name))
    }
    /// The path without param names: `/a/{x}` and `/a/{y}` are one path to the matcher, whatever the verb.
    pub fn shape(&self) -> String {
        self.join(|_| "{}".into())
    }
    fn join(&self, param: impl Fn(&FieldDecl) -> String) -> String {
        let segs = self.segs.iter().map(|s| match s {
            Seg::Lit(l) => l.clone(),
            Seg::Param(c) => param(c),
        });
        format!("/{}", segs.collect::<Vec<_>>().join("/"))
    }
    /// The declaration without its word: `POST /accounts/:id:Int?q:Text? {name: Text}`.
    pub fn decl(&self) -> String {
        let query = self.query.iter().map(|q| q.show(":")).collect::<Vec<_>>().join("&");
        let body = self.body.clone().map(|fs| format!(" {}", FieldTy::Rec(fs)));
        format!("{} {}{}{query}{}", self.verb, self.path, if query.is_empty() { "" } else { "?" }, body.unwrap_or_default())
    }
    /// The path params of a concrete path such as `/accounts/7`, when it is one of this route's.
    pub fn bind(&self, path: &str) -> Option<Pairs> {
        let parts: Vec<&str> = path.strip_prefix('/')?.split('/').filter(|x| !x.is_empty()).collect();
        if parts.len() != self.segs.len() {
            return None;
        }
        let mut out = vec![];
        for (seg, part) in self.segs.iter().zip(parts) {
            match seg {
                Seg::Lit(l) if l.eq_ignore_ascii_case(part) => {}
                Seg::Lit(_) => return None,
                Seg::Param(d) => out.push((d.name.to_string(), decode(part, false))),
            }
        }
        Some(out)
    }
    pub fn path_params(&self) -> impl Iterator<Item = &FieldDecl> {
        self.segs.iter().filter_map(|s| match s {
            Seg::Param(c) => Some(c),
            Seg::Lit(_) => None,
        })
    }
    /// Every field the route puts at the top level of the record, besides `body` and `time`.
    pub fn params(&self) -> impl Iterator<Item = &FieldDecl> {
        self.path_params().chain(&self.query)
    }
}

/// `%xx` escapes as their bytes; in a query, `+` is a space too.
fn decode(s: &str, query: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' if query => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One request of a run: `VERB /path?query [body] → status [json]`.
#[derive(Clone, Debug)]
pub struct Request {
    pub verb: Name,
    /// Concrete, with its query: `/transactions?account=5`.
    pub path: String,
    pub body: Option<Expr>,
    pub status: u16,
    pub json: Option<Expr>,
    pub line: usize,
}

/// `run "name"` and its requests, sent in order against a freshly seeded database.
#[derive(Clone, Debug)]
pub struct Run {
    pub name: String,
    /// The time its clock starts at: `run "x" at "2026-10-07T12:00:00Z"`.
    pub at: Option<f64>,
    pub requests: Vec<Request>,
    pub line: usize,
}

impl Program {
    /// The route that answers `verb path`, with the path's params and its query.
    pub fn route_for(&self, verb: &str, path: &str) -> Option<(usize, Pairs, Pairs)> {
        bind(&self.routes, verb, path)
    }

    /// The live page at `path`, with the path's params and its query.
    pub fn live_for(&self, path: &str) -> Option<(usize, Pairs, Pairs)> {
        bind(&self.lives, "GET", path)
    }
}

fn bind(routes: &[Route], verb: &str, path: &str) -> Option<(usize, Pairs, Pairs)> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let query: Pairs = query.split('&').filter(|q| !q.is_empty()).map(|q| q.split_once('=').unwrap_or((q, ""))).map(|(k, v)| (decode(k, true), decode(v, true))).collect();
    routes.iter().enumerate().filter(|(_, r)| &*r.verb == verb).find_map(|(i, r)| Some((i, r.bind(path)?, query.clone())))
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
    pub routes: Vec<Route>,
    /// Live pages: `live /counter = counter`, served as GET.
    pub lives: Vec<Route>,
    pub runs: Vec<Run>,
    pub errors: Vec<Diag>,
    pub warnings: Vec<Diag>,
    pub holes: Vec<Hole>,
    pub lines: Vec<String>,
}

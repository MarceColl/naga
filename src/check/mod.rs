//! The signature checker. Every word gets one signature, built from its body and the
//! signatures of the words it calls. Nothing runs: callee signatures are applied to symbolic
//! shapes `{..r, known fields}`. Signatures are cached by content, Salsa-style: a group of
//! mutually recursive words is rechecked only when its text or a callee's signature changes.
mod apply;
mod expr;
mod sig;
mod stmt;
mod ty;
mod world;

use crate::ast::{FieldDecl, FieldTy, Pos, Program, StmtKind, walk};
use crate::value::Name;
use indexmap::IndexMap;
pub use sig::Sig;
use std::collections::HashMap;
use std::rc::Rc;
use ty::{Origin, Shape};
use world::{At, World};

/// What one analysis pass needs and collects.
pub struct Cx<'a> {
    p: &'a Program,
    sigs: &'a HashMap<Name, Rc<Sig>>,
    /// Signatures of the group being solved.
    local: HashMap<Name, Rc<Sig>>,
    /// The statement being analysed: preconditions found there remember it.
    here: Option<At>,
    /// None at the entry, where the run starts from an empty record.
    word: Option<Name>,
    ci: usize,
    chunk_line: usize,
    errors: Vec<(Option<At>, Diag)>,
    report_again: bool,
    /// The statement being analysed, start and end.
    span: Option<(At, At)>,
    /// Worlds seen around statements, waiting for the clause's shared preconditions.
    pending: Vec<(bool, At, World)>,
    points: IndexMap<(bool, At), Vec<Point>>,
}

impl Cx<'_> {
    fn at(&self, pos: Pos) -> Option<At> {
        let word = self.word.clone()?;
        Some(At { word, ci: self.ci, line: pos.line + 1 - self.chunk_line, col: pos.col })
    }
    fn err(&mut self, pos: Pos, msg: String) {
        let at = self.at(pos);
        self.errors.push((at, msg.into()));
    }
    /// The live worlds just before (or after) a statement; only words record them.
    fn point(&mut self, after: bool, pos: Pos, ws: &[World]) {
        if let Some(at) = self.at(pos) {
            self.pending.extend(ws.iter().filter(|w| w.live()).map(|w| (after, at.clone(), w.clone())));
        }
    }
    /// Callers check a clause's preconditions before entering it, so they hold at every step inside.
    fn flush(&mut self, extra: &IndexMap<ty::Key, world::Fact>) {
        for (after, at, mut w) in std::mem::take(&mut self.pending) {
            if !extra.is_empty() {
                let mut facts = extra.clone();
                facts.extend(w.facts);
                w.facts = facts;
            }
            let pt = Point { text: world::fmt_w(Some(&w), &w.shape, 0), lifted: w.lifted };
            let list = self.points.entry((after, at)).or_default();
            if !list.iter().any(|x| x.text == pt.text) {
                list.push(pt);
            }
        }
    }
}

/// The record's shape around a statement, as the editor shows it. `lifted`: it depends on a
/// callee's dispatch rather than on what the word itself knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub text: String,
    pub lifted: bool,
}

/// An error as the checker finds it: a title, and for the kinds that can say more, the code it
/// points at and the paths that lead to it. Positions are relative to their clause, so a
/// solved group's errors stay valid when it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Diag {
    pub title: String,
    pub body: Option<std::rc::Rc<Explained>>,
}

impl From<String> for Diag {
    fn from(title: String) -> Diag {
        Diag { title, body: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Explained {
    /// Code the message names: start, end, and the text there.
    pub refs: Vec<(At, At, String)>,
    pub lines: Vec<Vec<Part>>,
    /// Code that would fix it.
    pub fix: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    /// The ref with this index, by its text.
    Ref(usize),
    /// A record's shape.
    Shape(String),
    /// This step takes it.
    Ok,
    /// This step fails on it.
    No,
}

/// A ref placed in the program: lines from 1, columns from 0, end excluded.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub text: String,
}

/// The explained part of an error, placed in the program.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub refs: Vec<Span>,
    pub lines: Vec<Vec<Part>>,
    pub fix: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Error {
    pub line: usize,
    pub col: usize,
    pub msg: String,
    pub detail: Option<std::rc::Rc<Detail>>,
}

/// One case of a signature, as text.
#[derive(Clone, Debug)]
pub struct Case {
    pub input: String,
    pub out: Vec<String>,
    pub fail: Option<String>,
    pub guard: bool,
}

#[derive(Default)]
pub struct Stats {
    pub recomputed: Vec<Name>,
    pub reused: Vec<Name>,
}

pub struct Checked {
    pub errors: Vec<Error>,
    /// Shapes before and after each statement, by its (line, col).
    pub before: HashMap<(usize, usize), Vec<Point>>,
    pub after: HashMap<(usize, usize), Vec<Point>>,
    pub signatures: HashMap<Name, Vec<Case>>,
    pub stats: Stats,
}

struct Solved {
    sigs: HashMap<Name, Rc<Sig>>,
    /// Lines relative to each clause's chunk, so a solved group stays valid when it moves.
    errors: Vec<(At, Diag)>,
    points: IndexMap<(bool, At), Vec<Point>>,
}

/// Keeps solved groups between checks of successive versions of a program.
#[derive(Default)]
pub struct Checker {
    memo: IndexMap<String, Rc<Solved>>,
}

fn tarjan(names: &[Name], edges: &HashMap<Name, Vec<Name>>) -> Vec<Vec<Name>> {
    struct T<'a> {
        edges: &'a HashMap<Name, Vec<Name>>,
        index: usize,
        idx: HashMap<Name, usize>,
        low: HashMap<Name, usize>,
        stack: Vec<Name>,
        on: std::collections::HashSet<Name>,
        out: Vec<Vec<Name>>,
    }
    impl T<'_> {
        fn strong(&mut self, v: &Name) {
            self.idx.insert(v.clone(), self.index);
            self.low.insert(v.clone(), self.index);
            self.index += 1;
            self.stack.push(v.clone());
            self.on.insert(v.clone());
            for w in self.edges.get(v).cloned().unwrap_or_default() {
                if !self.idx.contains_key(&w) {
                    self.strong(&w);
                    let l = self.low[v].min(self.low[&w]);
                    self.low.insert(v.clone(), l);
                } else if self.on.contains(&w) {
                    let l = self.low[v].min(self.idx[&w]);
                    self.low.insert(v.clone(), l);
                }
            }
            if self.low[v] == self.idx[v] {
                let mut comp = vec![];
                loop {
                    let w = self.stack.pop().unwrap();
                    self.on.remove(&w);
                    comp.push(w.clone());
                    if &w == v {
                        break;
                    }
                }
                self.out.push(comp);
            }
        }
    }
    let mut t = T { edges, index: 0, idx: HashMap::new(), low: HashMap::new(), stack: vec![], on: Default::default(), out: vec![] };
    for n in names {
        if !t.idx.contains_key(n) {
            t.strong(n);
        }
    }
    t.out
}

fn uses_tables(text: &str) -> bool {
    ["all", "get", "where", "insert", "update", "delete"].iter().any(|op| {
        let pat = format!(".{op}");
        text.match_indices(&pat).any(|(i, _)| !text[i + pat.len()..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'))
    })
}

fn solve_group(p: &Program, comp: &[Name], sigs: &HashMap<Name, Rc<Sig>>) -> Solved {
    let mut local: HashMap<Name, Rc<Sig>> = comp.iter().map(|n| (n.clone(), Rc::new(Sig::Bottom))).collect();
    let (mut errors, mut points) = (vec![], IndexMap::new());
    for _ in 0..25 {
        let mut cx = Cx { p, sigs, local: local.clone(), here: None, word: None, ci: 0, chunk_line: 0, errors: vec![], report_again: true, span: None, pending: vec![], points: IndexMap::new() };
        let mut next = vec![];
        let mut changed = false;
        for n in comp {
            cx.word = Some(n.clone());
            let sig = cx.analyze_word(&p.words[n]);
            if sig::sig_key(Some(&sig)) != sig::sig_key(local.get(n).map(|s| &**s)) {
                changed = true;
            }
            next.push((n.clone(), Rc::new(sig)));
        }
        local.extend(next);
        errors = cx.errors.into_iter().map(|(at, m)| (at.expect("errors inside a word have a place"), m)).collect();
        points = cx.points;
        if !changed {
            break;
        }
    }
    Solved { sigs: local, errors, points }
}

/// Where a run starts: the entry from an empty record, a route from the record it declares.
struct CallSite<'a> {
    word: &'a Name,
    input: Shape,
    /// Names the site in errors found inside the word; `unplaced` prefixes the others, at `line`.
    label: String,
    unplaced: String,
    line: usize,
    /// A route must leave {status, json}.
    reply: bool,
}

fn field(d: &FieldDecl) -> (Name, ty::Field) {
    (d.name.clone(), ty::Field { ty: decl_ty(&d.ty), opt: d.opt })
}

/// What a route's router guarantees: nested records are closed.
fn decl_ty(t: &FieldTy) -> ty::Ty {
    match t {
        FieldTy::Col(c) => ty::col_type(*c),
        FieldTy::List(t) => ty::list(decl_ty(t)),
        FieldTy::Rec(ds) => ty::rec(Shape::closed(ds.iter().map(field).collect())),
    }
}

fn call_sites<'a>(p: &'a Program, entry: Option<&'a Name>) -> Vec<CallSite<'a>> {
    let entry = entry.filter(|e| p.words.contains_key(*e)).map(|e| CallSite {
        word: e,
        input: Shape::open(Origin::Closed),
        label: e.to_string(),
        unplaced: format!("running {e} from an empty record"),
        line: p.words[e].clauses[0].chunk_line,
        reply: false,
    });
    let routes = p.routes.iter().filter(|r| p.words.contains_key(&r.word)).map(|r| {
        let mut fields: IndexMap<Name, ty::Field> = r.params().map(field).collect();
        if let Some(decls) = &r.body {
            fields.insert("body".into(), ty::Field { ty: decl_ty(&FieldTy::Rec(decls.clone())), opt: false });
        }
        fields.insert("time".into(), ty::Field { ty: ty::Ty::Text, opt: false });
        let label = format!("route {} {}", r.verb, r.path);
        CallSite { word: &r.word, input: Shape::closed(fields), unplaced: label.clone(), label, line: r.line, reply: true }
    });
    entry.into_iter().chain(routes).collect()
}

impl Checker {
    pub fn check(&mut self, p: &Program, entry: Option<&Name>) -> Checked {
        let names: Vec<Name> = p.words.keys().cloned().collect();
        let mut edges: HashMap<Name, Vec<Name>> = HashMap::new();
        for (name, w) in &p.words {
            let mut cs: indexmap::IndexSet<Name> = Default::default();
            for c in &w.clauses {
                walk(&c.body, &mut |s| {
                    if let StmtKind::Call(n) = &s.kind
                        && p.words.contains_key(n) {
                            cs.insert(n.clone());
                        }
                });
            }
            edges.insert(name.clone(), cs.into_iter().collect());
        }
        let texts = |n: &Name| p.words[n].clauses.iter().map(|c| c.chunk_text.clone()).collect::<Vec<_>>();
        let tables_key = format!("{:?}", p.tables.values().map(|t| (&t.name, &t.single, t.cols.iter().map(|c| (&c.name, c.ty.name(), c.opt)).collect::<Vec<_>>())).collect::<Vec<_>>());
        let mut sigs: HashMap<Name, Rc<Sig>> = HashMap::new();
        let mut stats = Stats::default();
        let mut results: Vec<Rc<Solved>> = vec![];
        for mut comp in tarjan(&names, &edges) {
            ty::sort16(&mut comp);
            let mut ext: Vec<&Name> = comp.iter().flat_map(|n| edges[n].iter()).filter(|c| !comp.contains(c)).collect::<indexmap::IndexSet<_>>().into_iter().collect();
            ty::sort16(&mut ext);
            let tables = comp.iter().any(|n| texts(n).iter().any(|t| uses_tables(t)));
            let key = format!(
                "{:?}\u{0}{:?}\u{0}{}",
                comp.iter().map(|n| (n, texts(n))).collect::<Vec<_>>(),
                ext.iter().map(|c| (c, sig::sig_key(sigs.get(*c).map(|s| &**s)))).collect::<Vec<_>>(),
                if tables { tables_key.as_str() } else { "" }
            );
            let r = match self.memo.get(&key) {
                Some(r) => {
                    stats.reused.extend(comp.iter().cloned());
                    r.clone()
                }
                None => {
                    let r = Rc::new(solve_group(p, &comp, &sigs));
                    self.memo.insert(key, r.clone());
                    if self.memo.len() > 2000 {
                        self.memo.shift_remove_index(0);
                    }
                    stats.recomputed.extend(comp.iter().cloned());
                    r
                }
            };
            for n in &comp {
                sigs.insert(n.clone(), r.sigs[n].clone());
            }
            results.push(r);
        }
        // relative positions → lines in the current text
        let abs = |at: &At| p.words[&at.word].clauses[at.ci].chunk_line + at.line - 1;
        let mut errors: Vec<Error> = vec![];
        let place = |d: &Diag| {
            d.body.as_ref().map(|b| {
                let refs = b.refs.iter().map(|(a, e, text)| Span { line: abs(a), col: a.col, end_line: abs(e), end_col: e.col, text: text.clone() }).collect();
                Rc::new(Detail { refs, lines: b.lines.clone(), fix: b.fix.clone() })
            })
        };
        let mut push = |line: usize, col: usize, msg: String, detail: Option<Rc<Detail>>| {
            if !errors.iter().any(|e| e.line == line && e.col == col && e.msg == msg) {
                errors.push(Error { line, col, msg, detail });
            }
        };
        let (mut before, mut after) = (HashMap::new(), HashMap::new());
        for r in &results {
            for (at, d) in &r.errors {
                push(abs(at), at.col, d.title.clone(), place(d));
            }
            for ((is_after, at), pts) in &r.points {
                if p.words.contains_key(&at.word) {
                    let target = if *is_after { &mut after } else { &mut before };
                    target.insert((abs(at), at.col), pts.clone());
                }
            }
        }
        for site in call_sites(p, entry) {
            let mut top = Cx { p, sigs: &sigs, local: HashMap::new(), here: None, word: None, ci: 0, chunk_line: 0, errors: vec![], report_again: false, span: None, pending: vec![], points: IndexMap::new() };
            let outs = top.apply_sig(World::new(site.input), sigs.get(site.word).cloned(), site.word, Pos { line: 1, col: 0 });
            // a route answers with whatever status and json its word leaves
            for w in outs.iter().filter(|w| site.reply && w.live()) {
                for f in ["status", "json"] {
                    match world::lookup(w, &w.shape, f) {
                        world::Look::Present(..) => {}
                        world::Look::Opt(_) => top.errors.push((None, format!("{} might finish without {f}", site.word).into())),
                        _ => top.errors.push((None, format!("{} can finish without {f}", site.word).into())),
                    }
                }
            }
            for (at, d) in top.errors {
                let msg = d.title.clone();
                match at.filter(|a| p.words.contains_key(&a.word)) {
                    Some(at) => {
                        let chained = msg.find(" (").is_some_and(|i| msg[i..].contains(" ← "));
                        let msg = if chained { format!("{msg}, when {} runs", site.label) } else { format!("{msg} (when {} runs)", site.label) };
                        push(abs(&at), at.col, msg, place(&d));
                    }
                    None => push(site.line, 0, format!("{}: {msg}", site.unplaced), place(&d)),
                }
            }
        }
        // a failure explained once and reported again plainly (by a caller, or another pass)
        // keeps the explanation
        let explained: Vec<(usize, usize, String)> = errors.iter().filter(|e| e.detail.is_some()).map(|e| (e.line, e.col, e.msg.clone())).collect();
        errors.retain(|e| {
            let head = e.msg.split(" — ").next().unwrap_or(&e.msg);
            e.detail.is_some() || !explained.iter().any(|(l, c, m)| (*l, *c) == (e.line, e.col) && m.contains(head))
        });
        errors.sort_by_key(|e| (e.line, e.col));
        let signatures = sigs
            .iter()
            .map(|(n, s)| {
                let cases = match &**s {
                    Sig::Bottom => vec![],
                    Sig::Groups(gs) => gs
                        .iter()
                        .flat_map(|g| {
                            g.cases.iter().map(|c| Case {
                                input: c.in_text.clone(),
                                out: c.out_text.iter().fold(vec![], |mut v: Vec<String>, t| {
                                    if !v.contains(t) {
                                        v.push(t.clone());
                                    }
                                    v
                                }),
                                fail: if c.fail_reported { Some("stops with an error (see Notes)".into()) } else { c.fail.clone() },
                                guard: g.guard,
                            })
                        })
                        .collect(),
                };
                (n.clone(), cases)
            })
            .collect();
        Checked { errors, before, after, signatures, stats }
    }
}

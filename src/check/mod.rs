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
    errors: Vec<(Option<At>, String)>,
    report_again: bool,
}

impl Cx<'_> {
    fn at(&self, pos: Pos) -> Option<At> {
        let word = self.word.clone()?;
        Some(At { word, ci: self.ci, line: pos.line + 1 - self.chunk_line, col: pos.col })
    }
    fn err(&mut self, pos: Pos, msg: String) {
        let at = self.at(pos);
        self.errors.push((at, msg));
    }
}

#[derive(Clone, Debug)]
pub struct Error {
    pub line: usize,
    pub col: usize,
    pub msg: String,
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
    pub signatures: HashMap<Name, Vec<Case>>,
    pub stats: Stats,
}

struct Solved {
    sigs: HashMap<Name, Rc<Sig>>,
    /// Lines relative to each clause's chunk, so a solved group stays valid when it moves.
    errors: Vec<(At, String)>,
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
    let mut errors = vec![];
    for _ in 0..25 {
        let mut cx = Cx { p, sigs, local: local.clone(), here: None, word: None, ci: 0, chunk_line: 0, errors: vec![], report_again: true };
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
        if !changed {
            break;
        }
    }
    Solved { sigs: local, errors }
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
        let mut push = |line: usize, col: usize, msg: String| {
            if !errors.iter().any(|e| e.line == line && e.col == col && e.msg == msg) {
                errors.push(Error { line, col, msg });
            }
        };
        for r in &results {
            for (at, msg) in &r.errors {
                push(abs(at), at.col, msg.clone());
            }
        }
        for site in call_sites(p, entry) {
            let mut top = Cx { p, sigs: &sigs, local: HashMap::new(), here: None, word: None, ci: 0, chunk_line: 0, errors: vec![], report_again: false };
            let outs = top.apply_sig(World::new(site.input), sigs.get(site.word).cloned(), site.word, Pos { line: 1, col: 0 });
            // a route answers with whatever status and json its word leaves
            for w in outs.iter().filter(|w| site.reply && w.live()) {
                for f in ["status", "json"] {
                    match world::lookup(w, &w.shape, f) {
                        world::Look::Present(..) => {}
                        world::Look::Opt(_) => top.errors.push((None, format!("{} might finish without {f}", site.word))),
                        _ => top.errors.push((None, format!("{} can finish without {f}", site.word))),
                    }
                }
            }
            for (at, msg) in top.errors {
                match at.filter(|a| p.words.contains_key(&a.word)) {
                    Some(at) => {
                        let chained = msg.find(" (").is_some_and(|i| msg[i..].contains(" ← "));
                        let msg = if chained { format!("{msg}, when {} runs", site.label) } else { format!("{msg} (when {} runs)", site.label) };
                        push(abs(&at), at.col, msg);
                    }
                    None => push(site.line, 0, format!("{}: {msg}", site.unplaced)),
                }
            }
        }
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
        Checked { errors, signatures, stats }
    }
}

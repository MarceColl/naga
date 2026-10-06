//! Everything the editor knows, recomputed from the program text: steps and their ranges,
//! definitions, holes, errors, the checker's shapes, and the runs.
use naga::ast::{Clause, Pos, Program, Stmt, StmtKind};
use naga::check::{Checked, Checker, Point};
use naga::runs::{Outcome, run};
use naga::syntax::parse;
use naga::value::Name;
use std::collections::HashMap;

/// A range of characters: from `pos` up to, not including, `end`. Lines are 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub pos: Pos,
    pub end: Pos,
}

impl Range {
    pub fn contains(&self, line: usize, col: usize) -> bool {
        (self.pos.line, self.pos.col) <= (line, col) && (line, col) < (self.end.line, self.end.col)
    }
}

/// Where a step sits: directly in a clause's body, or inside another step (`@f( … )`, a branch of `( a ‖ b )`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parent {
    Clause(Name, usize),
    Step(usize, usize),
}

#[derive(Clone, Debug)]
pub struct Step {
    pub id: usize,
    pub stmt: Stmt,
    pub range: Range,
    pub parent: Parent,
    /// Position among its siblings.
    pub idx: usize,
    pub depth: usize,
    pub word: Name,
}

impl Step {
    pub fn call(&self) -> Option<&Name> {
        match &self.stmt.kind {
            StmtKind::Call(n) => Some(n),
            _ => None,
        }
    }
    pub fn table(&self) -> Option<&Name> {
        match &self.stmt.kind {
            StmtKind::Table { table, .. } => Some(table),
            _ => None,
        }
    }
    pub fn hole(&self) -> Option<&str> {
        match &self.stmt.kind {
            StmtKind::Hole(note) => Some(note),
            _ => None,
        }
    }
}

/// A clause's head: the word's name at the start of `line`.
#[derive(Clone, Debug)]
pub struct Def {
    pub name: Name,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Error {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

pub struct Analysis {
    pub text: String,
    pub lines: Vec<Vec<char>>,
    pub program: Program,
    pub checked: Option<Checked>,
    pub errors: Vec<Error>,
    pub runs: Vec<Outcome>,
    pub steps: Vec<Step>,
    by_id: HashMap<usize, usize>,
    pub defs: Vec<Def>,
    /// The innermost step under each character, 0 for none.
    pub cells: Vec<Vec<usize>>,
}

impl Analysis {
    pub fn new(text: &str) -> Analysis {
        let lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        let program = parse(text);
        let mut errors: Vec<Error> = program.errors.iter().map(|e| Error { line: e.line, col: e.col, msg: e.msg.clone() }).collect();
        let (mut checked, mut runs) = (None, vec![]);
        if errors.is_empty() {
            let entry = program.words.contains_key("main").then(|| Name::from("main"));
            let c = Checker::default().check(&program, entry.as_ref());
            for e in &c.errors {
                if !errors.iter().any(|x| x.line == e.line && x.msg == e.msg) {
                    errors.push(Error { line: e.line, col: e.col, msg: e.msg.clone() });
                }
            }
            checked = Some(c);
            runs = program.runs.iter().map(|r| run(&program, r)).collect();
        }
        let mut an = Analysis { text: text.into(), lines, program, checked, errors, runs, steps: vec![], by_id: HashMap::new(), defs: vec![], cells: vec![] };
        let words: Vec<(Name, Vec<Clause>)> = an.program.words.values().map(|w| (w.name.clone(), w.clauses.clone())).collect();
        for (name, clauses) in words {
            for (ci, c) in clauses.iter().enumerate() {
                an.defs.push(Def { name: name.clone(), line: c.line });
                an.walk(&c.body, Parent::Clause(name.clone(), ci), 0, &name);
            }
        }
        an.cells = an.cells();
        an
    }

    fn walk(&mut self, body: &[Stmt], parent: Parent, depth: usize, word: &Name) {
        for (idx, st) in body.iter().enumerate() {
            let id = self.steps.len() + 1;
            let range = Range { pos: st.pos, end: st.end };
            self.by_id.insert(id, self.steps.len());
            self.steps.push(Step { id, stmt: st.clone(), range, parent: parent.clone(), idx, depth, word: word.clone() });
            match &st.kind {
                StmtKind::Focus(_, b) => self.walk(b, Parent::Step(id, 0), depth + 1, word),
                StmtKind::Par(bs) => bs.iter().enumerate().for_each(|(bi, b)| self.walk(b, Parent::Step(id, bi), depth + 1, word)),
                _ => {}
            }
        }
    }

    /// Innermost step per character: deeper steps paint over shallower ones; a step's own
    /// leading and trailing spaces stay unclaimed.
    fn cells(&self) -> Vec<Vec<usize>> {
        let mut cells: Vec<Vec<usize>> = self.lines.iter().map(|l| vec![0; l.len()]).collect();
        let mut order: Vec<&Step> = self.steps.iter().collect();
        order.sort_by_key(|s| s.depth);
        for s in order {
            let r = s.range;
            for ln in r.pos.line..=r.end.line {
                let Some(row) = cells.get_mut(ln - 1) else { continue };
                let text = &self.lines[ln - 1];
                let a = if ln == r.pos.line { r.pos.col } else { 0 };
                let b = if ln == r.end.line { r.end.col.min(row.len()) } else { row.len() };
                for i in a..b {
                    if text[i] != ' ' || (i > a && i + 1 < b) {
                        row[i] = s.id;
                    }
                }
            }
        }
        cells
    }

    pub fn step(&self, id: usize) -> Option<&Step> {
        self.by_id.get(&id).map(|&i| &self.steps[i])
    }

    pub fn holes(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| s.hole().is_some())
    }

    /// The step's siblings, itself included, in order.
    pub fn siblings(&self, s: &Step) -> Vec<&Step> {
        let mut v: Vec<&Step> = self.steps.iter().filter(|x| x.parent == s.parent).collect();
        v.sort_by_key(|x| x.idx);
        v
    }

    pub fn def_at(&self, line: usize) -> Option<&Def> {
        self.defs.iter().find(|d| d.line == line)
    }

    /// Every table's rows as seeded, even while the program doesn't parse.
    pub fn seeded(&self) -> naga::runs::Tables {
        if let Some(r) = self.runs.first() {
            return r.seed.clone();
        }
        let all: Vec<Name> = self.program.tables.keys().cloned().collect();
        let store = naga::store::Store::new(&self.program).ok().and_then(|mut s| s.seed(&self.program, &all).ok().map(|_| s));
        store.and_then(|mut s| s.snapshot(&self.program).ok()).unwrap_or_default()
    }

    /// The shapes a hole receives: what its word's callers pass, or what the checker saw at it.
    pub fn arrives(&self, hole: &Step) -> Vec<String> {
        let Some(c) = &self.checked else { return vec![] };
        let mut out: Vec<String> = vec![];
        for s in self.steps.iter().filter(|s| s.call() == Some(&hole.word)) {
            for t in best(c.before.get(&(s.range.pos.line, s.range.pos.col))) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        if out.is_empty() {
            out = best(c.before.get(&(hole.range.pos.line, hole.range.pos.col)));
        }
        out
    }

    pub fn passing(&self) -> usize {
        self.runs.iter().filter(|r| r.verdict == naga::runs::Verdict::Pass).count()
    }
}

/// Non-lifted shapes, or the most detailed lifted one when that's all there is.
pub fn best(pts: Option<&Vec<Point>>) -> Vec<String> {
    let Some(pts) = pts.filter(|p| !p.is_empty()) else { return vec![] };
    let real: Vec<&Point> = pts.iter().filter(|p| !p.lifted).collect();
    let mut out: Vec<String> = vec![];
    let use_ = if real.is_empty() { vec![pts.iter().max_by_key(|p| p.text.len()).unwrap()] } else { real };
    for p in use_ {
        if !out.contains(&p.text) {
            out.push(p.text.clone());
        }
    }
    out
}


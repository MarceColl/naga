//! The stepping machine. A state is immutable: the subject record, a stack of frames, a tick.
//! A session keeps every state, so a run can be stepped back and replayed.
use crate::ast::*;
use crate::eval::{Env, eval};
use crate::store::Store;
use crate::value::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Frame {
    Seq { body: Body, i: usize, lvl: usize },
    /// `@field( … )` on a record.
    Focus { field: Name, saved: Arc<Rec>, lvl: usize },
    /// `@field( … )` on a list: the body runs once per item.
    Map { field: Name, saved: Arc<Rec>, items: Arc<Vec<Value>>, results: Vec<Value>, body: Body, lvl: usize },
    /// `( a ‖ b )`: each branch runs on `base`, then their changes merge.
    Par { base: Arc<Rec>, branches: Vec<Body>, results: Vec<Arc<Rec>>, lvl: usize },
}

impl Frame {
    fn lvl(&self) -> usize {
        match self {
            Frame::Seq { lvl, .. } | Frame::Focus { lvl, .. } | Frame::Map { lvl, .. } | Frame::Par { lvl, .. } => *lvl,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Done,
    Error,
    Hole,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Done => "done",
            Status::Error => "error",
            Status::Hole => "hole",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub msg: String,
    pub details: Vec<String>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct State {
    pub subj: Arc<Rec>,
    pub k: Vec<Frame>,
    pub tick: usize,
    /// How much of the session's output this state has produced.
    pub out_len: usize,
    pub status: Status,
    pub brk: bool,
    pub error: Option<Failure>,
    pub hole: Option<String>,
}

impl State {
    pub fn next_stmt(&self) -> Option<&Stmt> {
        match self.k.last() {
            Some(Frame::Seq { body, i, .. }) if self.status != Status::Done => body.get(*i),
            _ => None,
        }
    }
    pub fn top_lvl(&self) -> isize {
        match self.k.last() {
            Some(f) if self.status != Status::Done => f.lvl() as isize,
            _ => -1,
        }
    }
    /// Where the next step runs: `@card`, `@lines[2]`, `‖ 1/2`.
    pub fn context(&self) -> Vec<String> {
        self.k
            .iter()
            .filter_map(|f| match f {
                Frame::Focus { field, .. } => Some(format!("@{field}")),
                Frame::Map { field, results, .. } => Some(format!("@{field}[{}]", results.len())),
                Frame::Par { results, branches, .. } => Some(format!("‖ {}/{}", results.len() + 1, branches.len())),
                Frame::Seq { .. } => None,
            })
            .collect()
    }
}

fn outer_chain(k: &[Frame]) -> Vec<Arc<Rec>> {
    k.iter()
        .rev()
        .filter_map(|f| match f {
            Frame::Focus { saved, .. } | Frame::Map { saved, .. } => Some(saved.clone()),
            _ => None,
        })
        .collect()
}

fn wrap_item(x: &Value) -> Arc<Rec> {
    match x {
        Value::Rec(r) => r.clone(),
        x => Arc::new(rec1("it", x.clone())),
    }
}

fn with(r: &Rec, k: &Name, v: Value) -> Arc<Rec> {
    let mut n = r.clone();
    n.insert(k.clone(), v);
    Arc::new(n)
}

// ---------- clause selection ----------
fn match_entries(entries: &[PatEntry], r: &Rec) -> Result<Vec<Name>, String> {
    let mut take = vec![];
    for e in entries {
        let v = r.get(&e.name);
        if e.mode == Mode::Absent {
            if v.is_some() {
                return Err(format!("has {}", e.name));
            }
            continue;
        }
        let Some(v) = v else { return Err(format!("needs {}", e.name)) };
        if let Some(sub) = &e.sub {
            let Value::Rec(inner) = v else { return Err(format!("{} is {}, not a record", e.name, v.type_name())) };
            match_entries(sub, inner).map_err(|why| format!("{}: {why}", e.name))?;
        }
        if let Some(lit) = &e.lit
            && !eqv(v, lit) {
                return Err(format!("{} is {}, not {}", e.name, v.show(), lit.show()));
            }
        if e.mode == Mode::Take {
            take.push(e.name.clone());
        }
    }
    Ok(take)
}

fn try_clause(c: &Clause, env: &Env) -> Result<Vec<Name>, String> {
    let take = match &c.pattern {
        Some(p) => match_entries(p, env.subj)?,
        None => vec![],
    };
    if let Some(g) = &c.guard {
        match eval(g, env) {
            Err(e) => return Err(format!("guard failed: {}", e.msg)),
            Ok(v) if !v.truthy() => return Err(format!("when {} is false", c.guard_src.as_deref().unwrap_or(""))),
            _ => {}
        }
    }
    Ok(take)
}

/// The first clause that matches, and the fields it takes out; or why each one missed.
pub fn select<'a>(w: &'a Word, env: &Env) -> Result<(&'a Clause, Vec<Name>), Vec<String>> {
    let mut why = vec![];
    for c in &w.clauses {
        match try_clause(c, env) {
            Ok(take) => return Ok((c, take)),
            Err(e) => why.push(format!("{} — {e}", c.head_src)),
        }
    }
    Err(why)
}

// ---------- stepping ----------
/// Pops finished frames: closes focuses, moves maps and branches to their next item.
fn settle(mut k: Vec<Frame>, mut subj: Arc<Rec>) -> Result<(Vec<Frame>, Arc<Rec>), RunError> {
    while let Some(top) = k.pop() {
        match top {
            Frame::Seq { ref body, i, .. } if i < body.len() => {
                k.push(top);
                break;
            }
            Frame::Seq { .. } => {}
            Frame::Focus { field, saved, .. } => subj = with(&saved, &field, Value::Rec(subj)),
            Frame::Map { field, saved, items, mut results, body, lvl } => {
                let val = if items[results.len()].as_rec().is_some() {
                    Value::Rec(subj)
                } else {
                    match subj.get("it") {
                        Some(v) => v.clone(),
                        None => return fail(format!("@{field}( … ) over plain values must leave a field named it")),
                    }
                };
                results.push(val);
                if results.len() < items.len() {
                    subj = wrap_item(&items[results.len()]);
                    k.push(Frame::Map { field, saved, items, results, body: body.clone(), lvl });
                    k.push(Frame::Seq { body, i: 0, lvl: lvl + 1 });
                    continue;
                }
                subj = with(&saved, &field, list(results));
            }
            Frame::Par { base, branches, mut results, lvl } => {
                results.push(subj);
                if results.len() < branches.len() {
                    let body = branches[results.len()].clone();
                    subj = base.clone();
                    k.push(Frame::Par { base, branches, results, lvl });
                    k.push(Frame::Seq { body, i: 0, lvl: lvl + 1 });
                    continue;
                }
                subj = merge_branches(&base, &results)?;
            }
        }
    }
    Ok((k, subj))
}

fn merge_branches(base: &Rec, results: &[Arc<Rec>]) -> Result<Arc<Rec>, RunError> {
    // field → (branch, new value or None for deleted)
    let mut changes: indexmap::IndexMap<Name, (usize, Option<Value>)> = Default::default();
    let mut note = |key: &Name, bi: usize, v: Option<Value>| -> Result<(), RunError> {
        if let Some((prev, _)) = changes.get(key)
            && *prev != bi {
                return fail(format!("both sides of ‖ change {key} (side {} and side {})", prev + 1, bi + 1));
            }
        changes.insert(key.clone(), (bi, v));
        Ok(())
    };
    for (bi, r) in results.iter().enumerate() {
        for (key, v) in r.iter() {
            if base.get(key).is_none_or(|b| !eqv(b, v)) {
                note(key, bi, Some(v.clone()))?;
            }
        }
        for key in base.keys() {
            if !r.contains_key(key) {
                note(key, bi, None)?;
            }
        }
    }
    let mut out = base.clone();
    for (key, (_, v)) in changes {
        match v {
            Some(v) => {
                out.insert(key, v);
            }
            None => {
                out.shift_remove(&key);
            }
        }
    }
    Ok(Arc::new(out))
}

pub struct Runtime<'a> {
    pub log: &'a mut Vec<String>,
    pub store: Option<&'a mut Store>,
    pub store_error: Option<&'a str>,
    /// What effectful functions read: the real clock for requests, a fixed one for runs.
    pub clock: crate::funcs::Clock,
}

pub fn initial(p: &Program, entry: &Name, subj: Arc<Rec>) -> State {
    let line = p.words.get(entry).map_or(0, |w| w.line);
    let call = Stmt { kind: StmtKind::Call(entry.clone()), pos: Pos { line, col: 0 }, end: Pos { line, col: entry.chars().count() } };
    State {
        subj,
        k: vec![Frame::Seq { body: Arc::from(vec![call]), i: 0, lvl: 0 }],
        tick: 0,
        out_len: 0,
        status: Status::Ready,
        brk: false,
        error: None,
        hole: None,
    }
}

/// How deep calls may nest.
const DEPTH: usize = 2000;

pub fn step(p: &Program, s: &State, rt: &mut Runtime) -> State {
    if s.status != Status::Ready {
        return s.clone();
    }
    let mut k = s.k.clone();
    let Some(Frame::Seq { body, i, lvl }) = k.pop() else { unreachable!("settled stacks end in a sequence") };
    let stmt = body[i].clone();
    if i + 1 < body.len() {
        k.push(Frame::Seq { body, i: i + 1, lvl });
    }
    let after = State { tick: s.tick + 1, brk: false, ..s.clone() };
    if let StmtKind::Hole(note) = &stmt.kind {
        return State { status: Status::Hole, hole: Some(note.clone()), ..after };
    }
    match exec(p, s, &stmt, k, lvl + 1, rt) {
        Ok((k, subj, brk)) => {
            let out_len = if matches!(stmt.kind, StmtKind::Say(_) | StmtKind::Show) { rt.log.len() } else { s.out_len };
            State { status: if k.is_empty() { Status::Done } else { Status::Ready }, subj, k, brk, out_len, ..after }
        }
        Err(e) => State { status: Status::Error, error: Some(Failure { msg: e.msg, details: e.details, line: stmt.pos.line }), ..after },
    }
}

fn exec(p: &Program, s: &State, stmt: &Stmt, mut k: Vec<Frame>, lvl: usize, rt: &mut Runtime) -> Result<(Vec<Frame>, Arc<Rec>, bool), RunError> {
    let outer = outer_chain(&s.k);
    let ctx = crate::funcs::Ctx::new(rt.clock, s.tick);
    let env = Env { subj: &s.subj, now: s.tick as f64, outer: &outer, ctx: &ctx };
    let mut subj = s.subj.clone();
    let mut brk = false;
    let need = |subj: &Rec, n: &Name| if subj.contains_key(n) { Ok(()) } else { fail(format!("needs field {n}")) };
    let value_of = |f: &FieldInit| f.expr.as_ref().map_or(Ok(Value::Bool(true)), |e| eval(e, &env));
    match &stmt.kind {
        StmtKind::Call(name) => {
            let Some(w) = p.words.get(name) else { return fail(format!("{name} isn't defined")) };
            let (c, take) = select(w, &env).map_err(|details| RunError { msg: format!("no clause of {} matches {}", w.name, show_rec(&s.subj)), details })?;
            if !take.is_empty() {
                let mut n = (*subj).clone();
                take.iter().for_each(|t| {
                    n.shift_remove(t);
                });
                subj = Arc::new(n);
            }
            // a call with steps after it grows the stack (a last call doesn't): one that keeps
            // calling itself first would never end, and each step would get slower
            if k.len() >= DEPTH {
                return fail(format!("{name} is {DEPTH} calls deep: does it change something before calling itself again?"));
            }
            k.push(Frame::Seq { body: c.body.clone(), i: 0, lvl });
        }
        StmtKind::Add(fs) => {
            let mut n = (*subj).clone();
            for f in fs {
                if n.contains_key(&f.name) {
                    return fail(format!("{} is already there — use {}=… to change it", f.name, f.name));
                }
                n.insert(f.name.clone(), value_of(f)?);
            }
            subj = Arc::new(n);
        }
        StmtKind::Del(names) => {
            let mut n = (*subj).clone();
            for f in names {
                if n.shift_remove(f).is_none() {
                    return fail(format!("needs field {f}"));
                }
            }
            subj = Arc::new(n);
        }
        StmtKind::Pick(names) => {
            for n in names {
                need(&subj, n)?;
            }
            subj = Arc::new(subj.iter().filter(|(k, _)| names.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect());
        }
        StmtKind::Set(name, e) => {
            need(&subj, name)?;
            subj = with(&subj, name, eval(e, &env)?);
        }
        StmtKind::Rename(from, to) => {
            need(&subj, from)?;
            if subj.contains_key(to) {
                return fail(format!("{to} is already there — {from}→{to} won't overwrite it"));
            }
            subj = Arc::new(subj.iter().map(|(k, v)| (if k == from { to.clone() } else { k.clone() }, v.clone())).collect());
        }
        StmtKind::Wrap(field, names) => {
            if subj.contains_key(field) {
                return fail(format!("{field} is already there — ⊂{field}{{…}} makes a new field"));
            }
            for n in names {
                need(&subj, n)?;
            }
            let mut n: Rec = subj.iter().filter(|(k, _)| !names.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
            n.insert(field.clone(), rec(names.iter().map(|x| (x.clone(), subj[x].clone())).collect()));
            subj = Arc::new(n);
        }
        StmtKind::Unwrap(field) => {
            need(&subj, field)?;
            let Value::Rec(inner) = &subj[field] else {
                return fail(format!("⊃{field} needs a record, but {field} is {}", subj[field].described()));
            };
            let mut n = (*subj).clone();
            n.shift_remove(field);
            for (key, v) in inner.iter() {
                if n.contains_key(key) {
                    return fail(format!("⊃{field}: {key} is already there"));
                }
                n.insert(key.clone(), v.clone());
            }
            subj = Arc::new(n);
        }
        StmtKind::Over(fs) => {
            let mut n = (*subj).clone();
            for f in fs {
                n.insert(f.name.clone(), value_of(f)?);
            }
            subj = Arc::new(n);
        }
        StmtKind::Unify(fs) => {
            if subj.contains_key("conflict") {
                return fail("conflict is already there — handle it before the next ⊔{…}");
            }
            let vals: Rec = fs.iter().map(|f| Ok((f.name.clone(), value_of(f)?))).collect::<Result<_, RunError>>()?;
            let (merged, conflict) = unify(&subj, &vals);
            subj = if conflict.is_empty() { Arc::new(merged) } else { with(&subj, &"conflict".into(), rec(conflict)) };
        }
        StmtKind::Inc(name, by) => {
            need(&subj, name)?;
            let Value::Num(v) = subj[name] else { return fail(format!("{name} is {}, not a number", subj[name].described())) };
            subj = with(&subj, name, Value::Num(v + by));
        }
        StmtKind::Focus(field, body) => {
            need(&subj, field)?;
            match subj[field].clone() {
                Value::Rec(r) => {
                    k.push(Frame::Focus { field: field.clone(), saved: subj, lvl });
                    k.push(Frame::Seq { body: body.clone(), i: 0, lvl: lvl + 1 });
                    subj = r;
                }
                Value::List(items) => {
                    if !items.is_empty() {
                        let first = wrap_item(&items[0]);
                        k.push(Frame::Map { field: field.clone(), saved: subj, items, results: vec![], body: body.clone(), lvl });
                        k.push(Frame::Seq { body: body.clone(), i: 0, lvl: lvl + 1 });
                        subj = first;
                    }
                }
                v => return fail(format!("@{field}( … ) needs a record or a list, but {field} is {}", v.described())),
            }
        }
        StmtKind::Par(branches) => {
            k.push(Frame::Par { base: subj.clone(), branches: branches.clone(), results: vec![], lvl });
            k.push(Frame::Seq { body: branches[0].clone(), i: 0, lvl: lvl + 1 });
        }
        StmtKind::Say(e) => {
            let t = eval(e, &env)?.text();
            rt.log.push(t);
        }
        StmtKind::Show => rt.log.push(show_rec(&subj)),
        StmtKind::Raise(e) => return fail(eval(e, &env)?.text()),
        StmtKind::Break => brk = true,
        StmtKind::Hole(_) => unreachable!(),
        StmtKind::Table { table, op, compiled, .. } => {
            let Some(store) = rt.store.as_deref_mut() else {
                return fail(rt.store_error.unwrap_or("tables need a database, and none is loaded"));
            };
            let Some(t) = p.tables.get(table) else { return fail(format!("there's no table called {table}")) };
            store.tick = s.tick as i64;
            let free = |subj: &Rec, keys: &[&str]| {
                for key in keys {
                    if subj.contains_key(*key) {
                        return fail(format!("{key} is already there — take it out first with -{{{key}}}"));
                    }
                }
                Ok(())
            };
            let id_of = |subj: &Rec| -> Result<f64, RunError> {
                need(subj, &"id".into())?;
                match &subj["id"] {
                    Value::Num(n) if crate::num::is_int(*n) => Ok(*n),
                    v => fail(format!("id must be a whole number, got {}", v.described())),
                }
            };
            let found = |r: Option<Value>, name: &Name| match r {
                Some(v) => (name.clone(), v),
                None => ("missing".into(), Value::Bool(true)),
            };
            match &**op {
                "all" => {
                    free(&subj, &[&t.name])?;
                    subj = with(&subj, &t.name, list(store.all(t)?));
                }
                "where" => {
                    let Some(w) = compiled else { return fail(format!("this {}.where couldn't be turned into SQL — see Notes", t.name)) };
                    free(&subj, &[&t.name])?;
                    subj = with(&subj, &t.name, list(store.find(t, w, &env)?));
                }
                "get" => {
                    let id = id_of(&subj)?;
                    free(&subj, &[&t.single, "missing"])?;
                    let (key, v) = found(store.get(t, id, false)?, &t.single);
                    subj = with(&subj, &key, v);
                }
                "insert" => {
                    need(&subj, &t.single)?;
                    let r = store.insert(t, &subj[&t.single])?;
                    subj = with(&subj, &t.single, r.unwrap_or(Value::Nil));
                }
                "update" => {
                    let id = id_of(&subj)?;
                    need(&subj, &"changes".into())?;
                    free(&subj, &[&t.single, "missing"])?;
                    let ch = &subj["changes"];
                    let ch = if is_delta(ch) { rec(delta_changes(ch.as_rec().unwrap())) } else { ch.clone() };
                    let r = store.update(t, id, &ch)?;
                    let mut n = (*subj).clone();
                    n.shift_remove("changes");
                    let (key, v) = found(r, &t.single);
                    subj = with(&n, &key, v);
                }
                "delete" => {
                    let id = id_of(&subj)?;
                    free(&subj, &["deleted", "missing"])?;
                    let key: Name = if store.delete(t, id)? { "deleted".into() } else { "missing".into() };
                    subj = with(&subj, &key, Value::Bool(true));
                }
                _ => return fail(format!("{table}.{op} isn't a table word")),
            }
        }
    }
    let (k, subj) = settle(k, subj)?;
    Ok((k, subj, brk))
}

// ---------- sessions: stepping and time travel ----------
const HIST_MAX: usize = 60000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Into,
    Over,
    Out,
    Continue,
}

pub struct Session<'p> {
    p: &'p Program,
    hist: Vec<State>,
    cur: usize,
    dropped: usize,
    pub log: Vec<String>,
    pub store: Option<Store>,
    store_error: Option<String>,
    pub pause: &'static str,
}

impl<'p> Session<'p> {
    pub fn new(p: &'p Program, entry: &Name) -> Session<'p> {
        let mut store = None;
        let mut store_error = None;
        let mut err: Option<(usize, String)> = None;
        if !p.tables.is_empty() {
            let all: Vec<Name> = p.tables.keys().cloned().collect();
            let res = Store::new(p).map_err(|e| (p.tables[0].line, e)).and_then(|s| store.insert(s).seed(p, &all));
            if let Err((line, e)) = res {
                store_error = Some(e.msg.clone());
                err = Some((line, e.msg));
            }
        }
        let mut init = initial(p, entry, Arc::new(Rec::new()));
        if let Some((line, msg)) = err {
            init.status = Status::Error;
            init.error = Some(Failure { msg, details: vec![], line });
        }
        let pause = if init.status == Status::Error { "error" } else { "start" };
        Session { p, hist: vec![init], cur: 0, dropped: 0, log: vec![], store, store_error, pause }
    }

    pub fn state(&self) -> &State {
        &self.hist[self.cur]
    }

    /// Steps taken so far, counting history that was dropped.
    pub fn index(&self) -> usize {
        self.cur + self.dropped
    }

    pub fn forward(&mut self) -> &State {
        if self.cur + 1 < self.hist.len() {
            self.cur += 1;
            return self.state();
        }
        if self.state().status != Status::Ready {
            return self.state();
        }
        let mut rt = Runtime { log: &mut self.log, store: self.store.as_mut(), store_error: self.store_error.as_deref(), clock: Default::default() };
        let n = step(self.p, &self.hist[self.cur], &mut rt);
        self.hist.push(n);
        self.cur += 1;
        if self.hist.len() > HIST_MAX {
            let drop = 10000;
            self.hist.drain(..drop);
            self.cur -= drop;
            self.dropped += drop;
        }
        self.state()
    }

    pub fn back(&mut self) {
        self.cur = self.cur.saturating_sub(1);
        self.pause = "back";
    }

    pub fn seek(&mut self, i: usize) {
        self.cur = i.saturating_sub(self.dropped).min(self.hist.len() - 1);
        self.pause = "seek";
    }

    pub fn restart(&mut self) {
        self.cur = 0;
        self.pause = "start";
    }

    pub fn run(&mut self, mode: Step, bps: &[usize], budget: usize) {
        let start = self.state();
        if start.status != Status::Ready {
            return;
        }
        let l = start.top_lvl();
        for n in 1.. {
            let s = self.forward();
            let reason = match () {
                _ if s.status != Status::Ready => s.status.name(),
                _ if s.brk => "break",
                _ if mode == Step::Into => "step",
                _ if mode == Step::Over && s.top_lvl() <= l => "step",
                _ if mode == Step::Out && s.top_lvl() < l => "step",
                _ if s.next_stmt().is_some_and(|x| bps.contains(&x.pos.line)) => "breakpoint",
                _ if n >= budget => "budget",
                _ => continue,
            };
            self.pause = reason;
            return;
        }
    }
}

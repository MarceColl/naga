//! A run, recorded once and then stepped through: every machine state of every request, in
//! order, against one freshly seeded store, as `crate::runs` sends them.
use crate::editor::analysis::{Analysis, Range};
use crate::machine::{self, Runtime, Status};
use crate::serve::{Req, request};
use crate::store::Store;
use crate::value::Name;
use std::rc::Rc;

const BUDGET: usize = 50_000;

#[derive(Clone)]
pub struct Snap {
    /// Which request of the run.
    pub req: usize,
    pub st: machine::State,
}

#[derive(Clone)]
pub struct Debug {
    pub run: usize,
    pub hist: Rc<Vec<Snap>>,
    /// What each request was, as written: `GET /transactions/4`.
    pub reqs: Rc<Vec<String>>,
    /// Everything said, across requests; a state's `out_len` counts into it.
    pub log: Rc<Vec<String>>,
    pub at: usize,
    /// Lines that stop `continue`, `over` and `out`.
    pub bps: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Into,
    Over,
    Out,
    Back,
    Continue,
    Restart,
}

pub fn record(an: &Analysis, run: usize, bps: Vec<usize>) -> Result<Debug, String> {
    let p = &an.program;
    let r = p.runs.get(run).ok_or(format!("there are {} runs — debug 1 to debug {}", p.runs.len(), p.runs.len()))?;
    let all: Vec<Name> = p.tables.keys().cloned().collect();
    let mut store = Store::new(p).map_err(|e| e.msg)?;
    store.seed(p, &all).map_err(|(_, e)| e.msg)?;
    let (mut hist, mut log, mut reqs) = (vec![], vec![], vec![]);
    for (i, q) in r.requests.iter().enumerate() {
        reqs.push(format!("{} {}", q.verb, q.path));
        let Some((route, path, query)) = p.route_for(&q.verb, &q.path) else { continue };
        let lit = |e: &crate::ast::Expr| crate::eval::eval(e, &crate::eval::Env { subj: &Default::default(), now: 0.0, outer: &[] });
        let body = q.body.as_ref().and_then(|b| lit(b).ok()).map_or_else(String::new, |b| crate::value::to_plain_json(&b));
        let Ok(subj) = request(&p.routes[route], &Req { route, path, query, body }) else { continue };
        let mut s = machine::initial(p, &p.routes[route].word, std::sync::Arc::new(subj));
        let mut rt = Runtime { log: &mut log, store: Some(&mut store), store_error: None };
        hist.push(Snap { req: i, st: s.clone() });
        for _ in 0..BUDGET {
            if s.status != Status::Ready {
                break;
            }
            s = machine::step(p, &s, &mut rt);
            hist.push(Snap { req: i, st: s.clone() });
        }
        if s.status != Status::Done {
            break;
        }
    }
    if hist.is_empty() {
        return Err("none of this run's requests reach a word".into());
    }
    Ok(Debug { run, hist: Rc::new(hist), reqs: Rc::new(reqs), log: Rc::new(log), at: 0, bps })
}

impl Debug {
    pub fn now(&self) -> &Snap {
        &self.hist[self.at]
    }

    /// The statement about to run, or the one that just stopped the run.
    pub fn current(&self) -> Option<Range> {
        let s = &self.now().st;
        let stmt = match s.status {
            Status::Error | Status::Hole => self.at.checked_sub(1).and_then(|i| self.hist[i].st.next_stmt()),
            _ => s.next_stmt(),
        }?;
        Some(Range { pos: stmt.pos, end: stmt.end })
    }

    /// Where `m` leads from here: stepping stops at the end, at a `break`, and (except
    /// stepping into or back) at a breakpoint.
    pub fn go(&self, m: Move) -> usize {
        let last = self.hist.len() - 1;
        let lvl = self.now().st.top_lvl();
        let stop = |j: usize| {
            let s = &self.hist[j].st;
            let at_bp = s.next_stmt().is_some_and(|x| self.bps.contains(&x.pos.line));
            let done = s.status != Status::Ready || s.brk || at_bp || self.hist[j].req != self.now().req;
            match m {
                Move::Over => done || s.top_lvl() <= lvl,
                Move::Out => done || s.top_lvl() < lvl,
                _ => done,
            }
        };
        match m {
            Move::Into => (self.at + 1).min(last),
            Move::Back => self.at.saturating_sub(1),
            Move::Restart => 0,
            _ => (self.at + 1..=last).find(|&j| stop(j)).unwrap_or(last),
        }
    }
}

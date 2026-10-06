//! Runs: the requests a program declares, sent in order against a freshly seeded database
//! through the same path `naga serve` takes.
use crate::ast::*;
use crate::eval::{Env, eval};
use crate::machine::Status;
use crate::serve::{Req, handle};
use crate::store::Store;
use crate::value::*;
use indexmap::IndexMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    /// Every answer is the expected one.
    #[default]
    Pass,
    /// Some status or json differs.
    Fail,
    /// A request failed with a 500 or never got an answer.
    Error,
    /// A request reached a hole.
    Waits,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Error => "error",
            Verdict::Waits => "waits",
        }
    }
}

pub type Tables = IndexMap<Name, Vec<Value>>;

/// What a run did. `said` and `want` hold one `status json` line per request sent.
#[derive(Debug, Default)]
pub struct Outcome {
    pub verdict: Verdict,
    pub said: Vec<String>,
    pub want: Vec<String>,
    pub detail: Option<String>,
    pub seed: Tables,
    pub end: Tables,
    pub sql: Vec<String>,
}

impl Outcome {
    fn stop(mut self, v: Verdict, detail: String) -> Outcome {
        self.verdict = v;
        self.detail = Some(detail);
        self
    }
}

fn literal(e: &Expr) -> Result<Value, String> {
    eval(e, &Env { subj: &Rec::new(), now: 0.0, outer: &[] }).map_err(|e| e.msg)
}

/// The message of an `{"error": …}` answer.
fn reason(json: &str) -> String {
    match from_json(json) {
        Some(Value::Rec(r)) => r.get("error").map_or_else(|| json.to_string(), Value::text),
        _ => json.to_string(),
    }
}

/// JSON as a client sees it: constants become strings.
fn wire(v: &Value) -> Value {
    from_json(&to_plain_json(v)).unwrap_or(Value::Nil)
}

pub fn run(p: &Program, r: &Run) -> Outcome {
    let out = Outcome::default();
    let all: Vec<Name> = p.tables.keys().cloned().collect();
    let store = Store::new(p).map_err(|e| e.msg).and_then(|mut s| s.seed(p, &all).map(|_| s).map_err(|(_, e)| e.msg));
    let mut store = match store {
        Ok(s) => s,
        Err(e) => return out.stop(Verdict::Error, e),
    };
    let mut out = Outcome { seed: store.snapshot(p).unwrap_or_default(), ..out };
    store.log.clear();
    for q in &r.requests {
        let Some((route, path, query)) = p.route_for(&q.verb, &q.path) else {
            out = out.stop(Verdict::Error, format!("{} {} matches no route", q.verb, q.path));
            break;
        };
        let (body, want) = match (q.body.as_ref().map(literal).transpose(), q.json.as_ref().map(literal).transpose()) {
            (Ok(b), Ok(w)) => (b.map_or_else(String::new, |b| to_plain_json(&b)), w.map(|w| wire(&w))),
            (Err(e), _) | (_, Err(e)) => {
                out = out.stop(Verdict::Error, e);
                break;
            }
        };
        let h = handle(p, &mut store, &Req { route, path, query, body });
        out.sql.extend(h.sql);
        out.said.push(format!("{} {}", h.status, h.json));
        out.want.push(format!("{} {}", q.status, want.as_ref().map_or_else(|| "…".into(), to_plain_json)));
        match h.stop {
            Status::Hole => {
                out = out.stop(Verdict::Waits, reason(&h.json));
                break;
            }
            _ if h.status == 500 => {
                out = out.stop(Verdict::Error, reason(&h.json));
                break;
            }
            _ => {}
        }
        let same = h.status == q.status && want.is_none_or(|w| from_json(&h.json).is_some_and(|got| eqv(&got, &w)));
        if !same {
            out.verdict = Verdict::Fail;
        }
    }
    out.end = store.snapshot(p).unwrap_or_default();
    out
}

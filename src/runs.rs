//! Runs: the requests a program declares, sent in order against a freshly seeded database
//! through the same path `naga serve` takes.
use crate::ast::*;
use crate::eval::{Env, eval};
use crate::machine::Status;
use crate::serve::{Req, error, reply, request, run_word};
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

/// Where a step of a run starts: the word it runs and the record it runs on. A request is
/// read as `naga serve` reads it; `LIVE` mounts a page on its params; `EVENT` runs the page
/// on the state its last step left (`page`), with the event.
pub fn start(p: &Program, q: &Request, page: Option<&(Name, Rec)>) -> Result<(Name, Result<Rec, String>), String> {
    let job = |routes: &[Route], (i, path, query): (usize, Pairs, Pairs), body: String| {
        let r = &routes[i];
        (r.word.clone(), request(r, &Req { route: i, path, query, body }))
    };
    match &*q.verb {
        "LIVE" => Ok(job(&p.lives, p.live_for(&q.path).ok_or(format!("LIVE {} matches no live page", q.path))?, String::new())),
        "EVENT" => {
            let (word, state) = page.ok_or("an EVENT goes to the page mounted before it: start with LIVE /path")?;
            let mut subj = state.clone();
            subj.insert("time".into(), Value::Text(crate::serve::now_utc().into()));
            subj.insert("event".into(), q.body.as_ref().map(literal).transpose()?.unwrap_or(Value::Nil));
            Ok((word.clone(), Ok(subj)))
        }
        _ => {
            let route = p.route_for(&q.verb, &q.path).ok_or(format!("{} {} matches no route", q.verb, q.path))?;
            let body = q.body.as_ref().map(literal).transpose()?.map_or_else(String::new, |b| to_plain_json(&b));
            Ok(job(&p.routes, route, body))
        }
    }
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
    let mut page: Option<(Name, Rec)> = None;
    for q in &r.requests {
        let (word, subj) = match start(p, q, page.as_ref()) {
            Ok(x) => x,
            Err(e) => {
                out = out.stop(Verdict::Error, e);
                break;
            }
        };
        let want = match q.json.as_ref().map(literal).transpose() {
            Ok(w) => w,
            Err(e) => {
                out = out.stop(Verdict::Error, e);
                break;
            }
        };
        let ran = subj.map(|subj| run_word(p, &mut store, &word, subj));
        if let Ok(ran) = &ran {
            out.sql.extend(ran.sql.iter().cloned());
            if ran.stop == Status::Hole {
                out = out.stop(Verdict::Waits, ran.out.clone().err().unwrap_or_default());
                break;
            }
        }
        let same = if matches!(&*q.verb, "LIVE" | "EVENT") {
            // a page's step: the state it left has what the run expects
            let state = match ran.and_then(|r| r.out).and_then(|subj| crate::live::split(&subj)) {
                Ok((state, _)) => state,
                Err(e) => {
                    out = out.stop(Verdict::Error, e);
                    break;
                }
            };
            out.said.push(show_rec(&state));
            out.want.push(want.as_ref().map_or_else(|| "…".into(), Value::show));
            let same = want.is_none_or(|w| subsumes(&w, &Value::Rec(state.clone().into())));
            page = Some((word, state));
            same
        } else {
            let (status, json) = match ran {
                Err(msg) => (400, error(&msg)),
                Ok(ran) => match ran.out.and_then(|subj| reply(&subj)) {
                    Ok(x) => x,
                    Err(msg) => {
                        out = out.stop(Verdict::Error, msg);
                        break;
                    }
                },
            };
            let want = want.map(|w| wire(&w));
            out.said.push(format!("{status} {json}"));
            if status == 500 {
                out.want.push(format!("{} …", q.status));
                out = out.stop(Verdict::Error, reason(&json));
                break;
            }
            out.want.push(format!("{} {}", q.status, want.as_ref().map_or_else(|| "…".into(), to_plain_json)));
            status == q.status && want.is_none_or(|w| from_json(&json).is_some_and(|got| eqv(&got, &w)))
        };
        if !same {
            out.verdict = Verdict::Fail;
        }
    }
    out.end = store.snapshot(p).unwrap_or_default();
    out
}

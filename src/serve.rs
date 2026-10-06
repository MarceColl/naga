//! `naga serve`: axum in front, one interpreter thread behind. Values are `Rc`, so the program,
//! its store and every record stay on that thread; a request crosses over as strings and its
//! answer comes back as a status and JSON text.
use crate::ast::*;
use crate::machine::{Runtime, Status, initial, step};
use crate::num::json_str;
use crate::store::Store;
use crate::syntax::parse;
use crate::value::*;
use axum::Router;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::routing::{MethodFilter, on};
use std::rc::Rc;
use std::sync::mpsc;
use tokio::sync::oneshot;

const BUDGET: usize = 200_000;

type Pairs = Vec<(String, String)>;

/// One request, as the interpreter receives it.
struct Job {
    route: usize,
    path: Pairs,
    query: Pairs,
    body: String,
    reply: oneshot::Sender<(u16, String)>,
}

/// Parses `src`, opens the store at `db` (creating and seeding new tables) and starts the
/// interpreter. The router forwards every declared route to it.
pub fn start(src: String, db: std::path::PathBuf) -> Result<Router, String> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("naga".into())
        .stack_size(256 << 20)
        .spawn(move || {
            let p = parse(&src);
            let mut store = match open(&p, &db) {
                Ok(s) => s,
                Err(e) => return drop(ready_tx.send(Err(e))),
            };
            let routes: Vec<(String, String)> = p.routes.iter().map(|r| (r.verb.to_string(), r.pattern())).collect();
            let _ = ready_tx.send(Ok(routes));
            for job in rx {
                let a = answer(&p, &mut store, &job);
                let _ = job.reply.send(a);
            }
        })
        .map_err(|e| e.to_string())?;
    let routes = ready_rx.recv().map_err(|_| "the interpreter stopped while starting".to_string())??;
    Ok(routes.into_iter().enumerate().fold(Router::new(), |app, (route, (verb, pattern))| {
        let tx = tx.clone();
        let handler = move |Path(path): Path<Pairs>, Query(query): Query<Pairs>, body: String| async move {
            let (reply, answer) = oneshot::channel();
            let sent = tx.send(Job { route, path, query, body, reply }).is_ok();
            let (status, json) = match sent {
                true => answer.await.unwrap_or_else(|_| (500, error("the interpreter stopped"))),
                false => (500, error("the interpreter stopped")),
            };
            (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, "application/json")], json)
        };
        app.route(&pattern, on(filter(&verb), handler))
    }))
}

fn filter(verb: &str) -> MethodFilter {
    match verb {
        "GET" => MethodFilter::GET,
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "PATCH" => MethodFilter::PATCH,
        _ => MethodFilter::DELETE,
    }
}

fn open(p: &Program, db: &std::path::Path) -> Result<Store, String> {
    if !p.errors.is_empty() {
        return Err(p.errors.iter().map(|e| format!("parse {}:{} {}", e.line, e.col, e.msg)).collect::<Vec<_>>().join("\n"));
    }
    let (mut store, created) = Store::open(p, db).map_err(|e| e.msg)?;
    store.seed(p, &created).map_err(|(line, e)| format!("seed on line {line}: {}", e.msg))?;
    store.log.clear();
    Ok(store)
}

fn error(msg: &str) -> String {
    format!("{{\"error\":{}}}", json_str(msg))
}

fn answer(p: &Program, store: &mut Store, job: &Job) -> (u16, String) {
    let r = &p.routes[job.route];
    let subj = match request(r, job) {
        Ok(s) => s,
        Err(msg) => return (400, error(&msg)),
    };
    let mut log = vec![];
    let mut s = initial(p, &r.word, Rc::new(subj));
    let mut rt = Runtime { log: &mut log, store: Some(store), store_error: None };
    for _ in 0..BUDGET {
        if s.status != Status::Ready {
            break;
        }
        s = step(p, &s, &mut rt);
    }
    for l in &log {
        println!("{} {} > {l}", r.verb, r.path);
    }
    store.log.clear();
    let out = match s.status {
        Status::Done => reply(&s.subj),
        Status::Error => Err(s.error.map_or_else(String::new, |e| format!("{} (line {})", e.msg, e.line))),
        Status::Hole => Err(format!("reached a hole: {}", s.hole.unwrap_or_default())),
        Status::Ready => Err(format!("still running after {BUDGET} steps")),
    };
    out.unwrap_or_else(|msg| (500, error(&msg)))
}

/// Path params and query fields at the top level, the body in `body`, the time in `time`.
fn request(r: &Route, job: &Job) -> Result<Rec, String> {
    let mut rec = Rec::new();
    let raw = |pairs: &Pairs, c: &Col| pairs.iter().find(|(k, _)| **k == *c.name).map(|(_, v)| text(c, v));
    for (c, pairs) in r.path_params().map(|c| (c, &job.path)).chain(r.query.iter().map(|c| (c, &job.query))) {
        field(&mut rec, c, raw(pairs, c))?;
    }
    if let Some(cols) = &r.body {
        let Some(Value::Rec(given)) = from_json(&job.body) else { return Err("the body must be a JSON object".into()) };
        let mut body = Rec::new();
        for c in cols {
            field(&mut body, c, given.get(&c.name).filter(|v| !matches!(v, Value::Nil)).cloned())?;
        }
        rec.insert("body".into(), Value::Rec(Rc::new(body)));
    }
    rec.insert("time".into(), Value::Text(now_utc().into()));
    Ok(rec)
}

fn field(rec: &mut Rec, c: &Col, v: Option<Value>) -> Result<(), String> {
    match v.map(|v| decode(c.ty, v)) {
        None if c.opt => Ok(()),
        None => Err(format!("{} is required", c.name)),
        Some(None) => Err(format!("{} must be {}", c.name, c.ty.name())),
        Some(Some(v)) => {
            rec.insert(c.name.clone(), v);
            Ok(())
        }
    }
}

/// Path and query values arrive as text; unless the column wants text, they're read as JSON.
fn text(c: &Col, s: &str) -> Value {
    match c.ty {
        ColType::Text | ColType::Const => Value::Text(s.into()),
        _ => from_json(s).unwrap_or_else(|| Value::Text(s.into())),
    }
}

fn decode(ty: ColType, v: Value) -> Option<Value> {
    match (ty, v) {
        (ColType::Int, Value::Num(n)) if n.fract() == 0.0 => Some(Value::Num(n)),
        (ColType::Num, v @ Value::Num(_)) | (ColType::Text, v @ Value::Text(_)) | (ColType::Bool, v @ Value::Bool(_)) | (ColType::Json, v) => Some(v),
        (ColType::Const, Value::Text(s)) => Some(Value::Const(s)),
        _ => None,
    }
}

fn reply(subj: &Rec) -> Result<(u16, String), String> {
    let status = match subj.get("status") {
        Some(Value::Num(n)) if n.fract() == 0.0 && (100.0..600.0).contains(n) => *n as u16,
        Some(v) => return Err(format!("status must be a number from 100 to 599, got {}", v.show())),
        None => return Err("the route left no status".into()),
    };
    let json = subj.get("json").ok_or("the route left no json")?;
    Ok((status, if status == 204 { String::new() } else { to_plain_json(json) }))
}

/// `2026-10-06T21:03:00Z`, from days since the epoch to a civil date (Hinnant's algorithm).
fn now_utc() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, secs) = (s.div_euclid(86400), s.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

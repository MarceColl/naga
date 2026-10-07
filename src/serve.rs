//! The server: axum in front, the program behind. Requests run in parallel on tokio's
//! blocking threads, each on the program as it is when it arrives and with a connection of its
//! own to the database; `Live` holds both.
use crate::ast::*;
use crate::machine::{Runtime, Status, initial, step};
use crate::num::json_str;
use crate::store::{Migration, Store};
use crate::syntax::parse;
use crate::value::*;
use axum::Router;
use axum::http::{StatusCode, header};
use std::sync::{Arc, Mutex, RwLock};

const BUDGET: usize = 200_000;

/// One request, as strings: the index of its route, its path params, its query and its body.
pub struct Req {
    pub route: usize,
    pub path: Pairs,
    pub query: Pairs,
    pub body: String,
}

/// What a request did: the answer, how the run stopped, what it said and the SQL it ran.
pub struct Handled {
    pub status: u16,
    pub json: String,
    pub stop: Status,
    pub said: Vec<String>,
    pub sql: Vec<String>,
}

/// An HTTP request as text, before any route is chosen.
pub struct Raw {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: String,
}

/// A running program: the version requests run on (replaced as it changes) and the database
/// they share, a connection per request at a time.
pub struct Live {
    program: RwLock<Arc<Program>>,
    /// The live pages open now.
    pub pages: crate::live::Sessions,
    db: std::path::PathBuf,
    idle: Mutex<Vec<Store>>,
}

impl Live {
    /// The program at `db`, its tables made to fit it.
    pub fn open(p: Arc<Program>, db: &std::path::Path) -> Result<(Arc<Live>, Migration), String> {
        let (store, m) = open(&p, db)?;
        Ok((Arc::new(Live { program: RwLock::new(p), pages: Default::default(), db: db.into(), idle: Mutex::new(vec![store]) }), m))
    }

    pub fn program(&self) -> Arc<Program> {
        self.program.read().map_or_else(|e| e.into_inner().clone(), |p| p.clone())
    }

    /// From the next request on, requests run on `p`, and open pages show what it renders.
    pub fn publish(self: &Arc<Self>, p: Arc<Program>) {
        if let Ok(mut cur) = self.program.write() {
            *cur = p;
        }
        if !self.pages.is_empty() {
            let me = self.clone();
            std::thread::Builder::new().stack_size(64 << 20).spawn(move || crate::live::refresh(&me, None)).ok();
        }
    }

    /// Runs `f` with a connection to the database, from the idle ones or a new one.
    pub fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> Result<R, String> {
        let idle = self.idle.lock().ok().and_then(|mut v| v.pop());
        let mut store = match idle {
            Some(s) => s,
            None => Store::connect(&self.db).map_err(|e| e.msg)?,
        };
        let r = f(&mut store);
        store.log.clear();
        if let Ok(mut v) = self.idle.lock() {
            v.push(store);
        }
        Ok(r)
    }

    /// Answers a request on the program as it is now; if it wrote to a table, open pages
    /// refresh.
    pub fn answer(&self, raw: &Raw) -> Handled {
        let p = self.program();
        let h = self.with_store(|s| respond(&p, s, raw)).unwrap_or_else(|e| Handled { status: 500, json: error(&e), stop: Status::Error, said: vec![], sql: vec![] });
        if h.sql.iter().any(|q| ["INSERT", "UPDATE", "DELETE"].iter().any(|w| q.starts_with(w))) {
            crate::live::refresh(self, None);
        }
        h
    }
}

/// The server: every request is answered by `live` on a blocking thread; `after` hears what
/// each one got (the editor shows it, `naga serve` prints what words said).
pub fn router(live: Arc<Live>, after: impl Fn(&Raw, &Handled) + Clone + Send + Sync + 'static) -> Router {
    let sockets = live.clone();
    Router::new()
        .route("/naga/live.js", axum::routing::get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], crate::live::SCRIPT) }))
        .route(
            "/naga/live",
            axum::routing::get(move |ws: axum::extract::ws::WebSocketUpgrade, q: axum::extract::Query<std::collections::HashMap<String, String>>| {
                let live = sockets.clone();
                let id = q.get("s").and_then(|s| s.parse().ok()).unwrap_or(0);
                async move { ws.on_upgrade(move |socket| crate::live::connect(live, id, socket)) }
            }),
        )
        .fallback(move |method: axum::http::Method, uri: axum::http::Uri, body: String| {
            let (live, after) = (live.clone(), after.clone());
            async move {
                let raw = Raw { method: method.to_string(), path: uri.path().into(), query: uri.query().unwrap_or("").into(), body };
                // a live page answers a GET with a page; everything else is JSON
                let (status, kind, text) = tokio::task::spawn_blocking(move || {
                    if raw.method == "GET" && let Some((status, page)) = crate::live::mount(&live, &raw) {
                        return (status, "text/html; charset=utf-8", page);
                    }
                    let h = live.answer(&raw);
                    after(&raw, &h);
                    (h.status, "application/json", h.json)
                })
                .await
                .unwrap_or_else(|_| (500, "application/json", error("the request stopped")));
                (StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, kind)], text)
            }
        })
}

/// A runtime whose blocking threads have room for deep programs (the interpreter recurses).
pub fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(64 << 20).build()
}

/// Answers a request with the program: its route's word, or the list of routes at `GET /`
/// (unless the program answers it), or a 404 or 405 naming the routes there are.
pub fn respond(p: &Program, store: &mut Store, raw: &Raw) -> Handled {
    let full = if raw.query.is_empty() { raw.path.clone() } else { format!("{}?{}", raw.path, raw.query) };
    if let Some((route, path, query)) = p.route_for(&raw.method, &full) {
        return handle(p, store, &Req { route, path, query, body: raw.body.clone() });
    }
    let pages = p.lives.iter().map(|l| json_str(&format!("LIVE {}", l.path)));
    let listing = format!("[{}]", p.routes.iter().map(|r| json_str(&r.decl())).chain(pages).collect::<Vec<_>>().join(","));
    let (status, json) = match (raw.method.as_str(), raw.path.as_str()) {
        ("GET", "/") => (200, format!("{{\"routes\":{listing}}}")),
        _ if p.routes.iter().any(|r| r.bind(&raw.path).is_some()) => (405, error(&format!("{} {} isn't a route; the path takes other verbs", raw.method, raw.path))),
        _ => (404, format!("{{\"error\":{},\"routes\":{listing}}}", json_str(&format!("no route {} {}", raw.method, raw.path)))),
    };
    Handled { status, json, stop: Status::Done, said: vec![], sql: vec![] }
}

/// `naga serve`: parses `src` and opens its tables at `db`; words' `say` lines are printed.
pub fn start(src: String, db: std::path::PathBuf) -> Result<Router, String> {
    let (live, m) = Live::open(Arc::new(parse(&src)), &db)?;
    m.applied.iter().for_each(|a| println!("naga: {a}"));
    m.pending.iter().for_each(|a| println!("naga: pending: {a} — the table keeps its old shape"));
    Ok(router(live, |raw, h| h.said.iter().for_each(|l| println!("{} {} > {l}", raw.method, raw.path))))
}

pub fn open(p: &Program, db: &std::path::Path) -> Result<(Store, Migration), String> {
    if !p.errors.is_empty() {
        return Err(p.errors.iter().map(|e| format!("parse {}:{} {}", e.line, e.col, e.msg)).collect::<Vec<_>>().join("\n"));
    }
    Store::open(p, db).map_err(|e| e.msg)
}

pub fn error(msg: &str) -> String {
    format!("{{\"error\":{}}}", json_str(msg))
}

/// Decodes the request, runs its route's word on `store` and encodes the reply.
pub fn handle(p: &Program, store: &mut Store, req: &Req) -> Handled {
    let r = &p.routes[req.route];
    let subj = match request(r, req) {
        Ok(s) => s,
        Err(msg) => return Handled { status: 400, json: error(&msg), stop: Status::Done, said: vec![], sql: vec![] },
    };
    let ran = run_word(p, store, &r.word, subj);
    let (status, json) = ran.out.and_then(|subj| reply(&subj)).unwrap_or_else(|msg| (500, error(&msg)));
    Handled { status, json, stop: ran.stop, said: ran.said, sql: ran.sql }
}

/// What running a word did: the record it left (or why it stopped), how it stopped, what
/// it said and the SQL it ran.
pub struct Ran {
    pub out: Result<Arc<Rec>, String>,
    pub stop: Status,
    pub said: Vec<String>,
    pub sql: Vec<String>,
}

/// Runs `word` on `subj` against `store`, within the step budget.
pub fn run_word(p: &Program, store: &mut Store, word: &Name, subj: Rec) -> Ran {
    let mut log = vec![];
    let mut s = initial(p, word, Arc::new(subj));
    let mut rt = Runtime { log: &mut log, store: Some(store), store_error: None };
    for _ in 0..BUDGET {
        if s.status != Status::Ready {
            break;
        }
        s = step(p, &s, &mut rt);
    }
    let sql = store.log.drain(..).map(|(_, l)| l).collect();
    let out = match s.status {
        Status::Done => Ok(s.subj.clone()),
        Status::Error => Err(s.error.clone().map_or_else(String::new, |e| format!("{} (line {})", e.msg, e.line))),
        Status::Hole => Err(format!("reached a hole: {}", s.hole.clone().unwrap_or_default())),
        Status::Ready => Err(format!("still running after {BUDGET} steps")),
    };
    Ran { out, stop: s.status, said: log, sql }
}

/// Path params and query fields at the top level, the body in `body`, the time in `time`.
pub fn request(r: &Route, job: &Req) -> Result<Rec, String> {
    let raw = |pairs: &Pairs, d: &FieldDecl| pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(&d.name)).map(|(_, v)| (d.name.clone(), text(d, v)));
    let given: Rec = r.path_params().filter_map(|d| raw(&job.path, d)).chain(r.query.iter().filter_map(|d| raw(&job.query, d))).collect();
    let mut rec = record(r.params(), &given, "")?;
    if let Some(decls) = &r.body {
        let Some(Value::Rec(given)) = from_json(&job.body) else { return Err("the body must be a JSON object".into()) };
        rec.insert("body".into(), Value::Rec(Arc::new(record(decls, &given, "")?)));
    }
    rec.insert("time".into(), Value::Text(now_utc().into()));
    Ok(rec)
}

/// The declared fields of `given`, decoded; `at` prefixes the names in errors.
fn record<'a>(decls: impl IntoIterator<Item = &'a FieldDecl>, given: &Rec, at: &str) -> Result<Rec, String> {
    let mut out = Rec::new();
    for d in decls {
        let at = format!("{at}{}", d.name);
        // names ignore case, JSON keys included
        let v = given.iter().find(|(k, _)| k.eq_ignore_ascii_case(&d.name)).map(|(_, v)| v);
        match v.filter(|v| !matches!(v, Value::Nil)) {
            None if d.opt => {}
            None => return Err(format!("{at} is required")),
            Some(v) => drop(out.insert(d.name.clone(), decode(&d.ty, v, &at)?)),
        }
    }
    Ok(out)
}

fn decode(ty: &FieldTy, v: &Value, at: &str) -> Result<Value, String> {
    use {ColType::*, FieldTy::Col};
    match (ty, v) {
        (Col(Int), Value::Num(n)) if n.fract() != 0.0 => Err(format!("{at} must be {ty}")),
        (Col(Const), Value::Text(s)) => Ok(Value::Const(s.to_lowercase().into())),
        (Col(Int | Num), Value::Num(_)) | (Col(Text), Value::Text(_)) | (Col(Bool), Value::Bool(_)) | (Col(Json), _) => Ok(v.clone()),
        (FieldTy::List(t), Value::List(xs)) => {
            let xs = xs.iter().enumerate().map(|(i, x)| decode(t, x, &format!("{at}[{i}]")));
            Ok(Value::List(Arc::new(xs.collect::<Result<_, _>>()?)))
        }
        (FieldTy::Rec(ds), Value::Rec(r)) => Ok(Value::Rec(Arc::new(record(ds, r, &format!("{at}."))?))),
        _ => Err(format!("{at} must be {ty}")),
    }
}

/// Path and query values arrive as text; unless the field wants text, they're read as JSON.
fn text(d: &FieldDecl, s: &str) -> Value {
    match d.ty {
        FieldTy::Col(ColType::Text | ColType::Const) => Value::Text(s.into()),
        _ => from_json(s).unwrap_or_else(|| Value::Text(s.into())),
    }
}

pub fn reply(subj: &Rec) -> Result<(u16, String), String> {
    let status = match subj.get("status") {
        Some(Value::Num(n)) if n.fract() == 0.0 && (100.0..600.0).contains(n) => *n as u16,
        Some(v) => return Err(format!("status must be a number from 100 to 599, got {}", v.show())),
        None => return Err("the route left no status".into()),
    };
    let json = subj.get("json").ok_or("the route left no json")?;
    Ok((status, if status == 204 { String::new() } else { to_plain_json(json) }))
}

/// `2026-10-06T21:03:00Z`, from days since the epoch to a civil date (Hinnant's algorithm).
pub fn now_utc() -> String {
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

// The program, its values and a run's state cross threads: requests run in parallel on them.
const _: () = {
    const fn shared<T: Send + Sync>() {}
    shared::<Program>();
    shared::<Value>();
    shared::<crate::machine::State>();
};

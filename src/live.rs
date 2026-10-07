//! Live pages: a word run on a session's record, the markup it leaves sent to the browser,
//! events coming back over a websocket. Sessions are shared; each has its own lock, so one
//! session's events stay in order while different sessions run in parallel.
use crate::ast::*;
use crate::serve::{Live, Raw, Req, request, run_word};
use crate::value::*;
use axum::extract::ws::{Message, WebSocket};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// The script every live page loads: morphdom, then the bit that talks to the server.
pub const SCRIPT: &str = concat!(include_str!("../assets/morphdom.min.js"), "\n", include_str!("../assets/live.js"));

/// One open page: the word it runs, its state, what it shows, and where its updates go.
pub struct Session {
    word: Name,
    state: Rec,
    html: String,
    out: Option<mpsc::UnboundedSender<String>>,
}

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<u64, Arc<Mutex<Session>>>>,
    next: AtomicU64,
}

impl Sessions {
    fn get(&self, id: u64) -> Option<Arc<Mutex<Session>>> {
        self.map.lock().ok()?.get(&id).cloned()
    }
    fn drop_session(&self, id: u64) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(&id);
        }
    }
    pub fn len(&self) -> usize {
        self.map.lock().map_or(0, |m| m.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------- markup ----------
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const VOID: [&str; 6] = ["br", "hr", "img", "input", "meta", "link"];

/// Markup as naga values, as HTML: `[:tag, {attrs}?, child…]`; strings are escaped, lists of
/// elements flatten, `on-click: :inc` becomes `data-naga-click="inc"`.
pub fn render(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v);
    out
}

fn write(out: &mut String, v: &Value) {
    match v {
        Value::Nil => {}
        Value::Text(s) => out.push_str(&escape(s)),
        Value::List(xs) => match xs.first() {
            Some(Value::Const(tag)) => element(out, tag, &xs[1..]),
            _ => xs.iter().for_each(|x| write(out, x)),
        },
        v => out.push_str(&escape(&v.text())),
    }
}

fn element(out: &mut String, tag: &str, rest: &[Value]) {
    out.push('<');
    out.push_str(tag);
    let (attrs, children) = match rest.first() {
        Some(Value::Rec(a)) => (Some(a), &rest[1..]),
        _ => (None, rest),
    };
    for (k, v) in attrs.iter().flat_map(|a| a.iter()) {
        match (k.strip_prefix("on-"), v) {
            (Some(ev), v) => out.push_str(&format!(" data-naga-{ev}=\"{}\"", escape(&v.text()))),
            (None, Value::Nil | Value::Bool(false)) => {}
            (None, Value::Bool(true)) => out.push_str(&format!(" {k}")),
            (None, v) => out.push_str(&format!(" {k}=\"{}\"", escape(&v.text()))),
        }
    }
    out.push('>');
    if VOID.contains(&tag) {
        return;
    }
    children.iter().for_each(|c| write(out, c));
    out.push_str(&format!("</{tag}>"));
}

// ---------- sessions ----------
/// The word left `html`, which is shown; the rest, but `event`, is the next state.
fn split(subj: &Rec) -> Result<(Rec, String), String> {
    let html = subj.get("html").ok_or("the page left no html")?;
    let state = subj.iter().filter(|(k, _)| !matches!(&***k, "html" | "event")).map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok((state, render(html)))
}

fn trouble(msg: &str) -> String {
    format!("<pre style=\"color:#ff3b30;background:#1a0000;padding:1em;white-space:pre-wrap\">naga: {}</pre>", escape(msg))
}

/// A GET for a live page: the word runs on the params, and a new session holds the state.
/// None when no live page has this path.
pub fn mount(live: &Live, raw: &Raw) -> Option<(u16, String)> {
    let p = live.program();
    let full = if raw.query.is_empty() { raw.path.clone() } else { format!("{}?{}", raw.path, raw.query) };
    let (path_part, query) = full.split_once('?').unwrap_or((&full, ""));
    let page = p.lives.iter().find_map(|l| Some((l, l.bind(path_part)?)))?;
    let query: Pairs = query.split('&').filter(|q| !q.is_empty()).map(|q| q.split_once('=').unwrap_or((q, ""))).map(|(k, v)| (k.into(), v.into())).collect();
    let req = Req { route: 0, path: page.1, query, body: String::new() };
    let subj = match request(page.0, &req) {
        Ok(s) => s,
        Err(msg) => return Some((400, trouble(&msg))),
    };
    let ran = live.with_store(|s| run_word(&p, s, &page.0.word, subj));
    let (state, html) = match ran.and_then(|r| r.out).and_then(|subj| split(&subj)) {
        Ok(x) => x,
        Err(msg) => return Some((500, trouble(&msg))),
    };
    let id = live.pages.next.fetch_add(1, Ordering::Relaxed) + 1;
    let session = Session { word: page.0.word.clone(), state, html: html.clone(), out: None };
    if let Ok(mut m) = live.pages.map.lock() {
        m.insert(id, Arc::new(Mutex::new(session)));
    }
    let doc = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{}</title></head><body><div id=\"naga\" data-s=\"{id}\">{html}</div><script src=\"/naga/live.js\"></script></body></html>",
        escape(&raw.path)
    );
    Some((200, doc))
}

/// An event from the page: the word runs on the state and the event; a changed page is sent.
pub fn event(live: &Live, id: u64, msg: &str) {
    let Some(session) = live.pages.get(id) else { return };
    let Ok(mut s) = session.lock() else { return };
    let event = match from_json(msg) {
        Some(Value::Rec(r)) => {
            let mut r = (*r).clone();
            if let Some(Value::Text(n)) = r.get("name").cloned() {
                r.insert("name".into(), Value::Const(n.to_lowercase().into()));
            }
            Value::Rec(Arc::new(r))
        }
        _ => return,
    };
    let mut subj = s.state.clone();
    subj.insert("event".into(), event);
    let p = live.program();
    let ran = live.with_store(|st| run_word(&p, st, &s.word, subj));
    let html = match ran.and_then(|r| r.out).and_then(|subj| split(&subj)) {
        Ok((state, html)) => {
            s.state = state;
            html
        }
        Err(msg) => format!("{}{}", trouble(&msg), s.html),
    };
    if html != s.html {
        s.html = html.clone();
        if let Some(out) = &s.out {
            let _ = out.send(html);
        }
    }
}

/// A page's websocket: updates go out as they're made, events come in and run in turn.
pub async fn connect(live: Arc<Live>, id: u64, mut socket: WebSocket) {
    let Some(session) = live.pages.get(id) else {
        let _ = socket.send(Message::Text("<p>this page's session has ended — reloading</p>".into())).await;
        return;
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    if let Ok(mut s) = session.lock() {
        s.out = Some(tx);
    }
    loop {
        tokio::select! {
            html = rx.recv() => match html {
                Some(h) => if socket.send(Message::Text(h.into())).await.is_err() { break },
                None => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let live = live.clone();
                    let t = t.to_string();
                    let _ = tokio::task::spawn_blocking(move || event(&live, id, &t)).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    live.pages.drop_session(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn eval(src: &str) -> Value {
        let p = parse(&format!("main = +{{v: {src}}}\n"));
        let crate::ast::StmtKind::Add(fs) = &p.words["main"].clauses[0].body[0].kind else { panic!() };
        crate::eval::eval(fs[0].expr.as_ref().unwrap(), &crate::eval::Env { subj: &Rec::new(), now: 0.0, outer: &[] }).unwrap()
    }

    #[test]
    fn markup_renders_as_html() {
        assert_eq!(render(&eval(r#"[:div, {class: "c"}, [:h1, "Count: 3"], [:button, {on-click: :inc, disabled: false}, "+"]]"#)),
            r#"<div class="c"><h1>Count: 3</h1><button data-naga-click="inc">+</button></div>"#);
        assert_eq!(render(&eval(r#"[:ul, [[:li, "a"], [:li, "<b>"]]]"#)), "<ul><li>a</li><li>&lt;b&gt;</li></ul>");
        assert_eq!(render(&eval(r#"[:input, {value: 1, checked: true}]"#)), r#"<input value="1" checked>"#);
    }
}

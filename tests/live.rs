//! The live database follows the program: what's safe at once, the rest when reset.
use naga::store::{Migration, Store};
use naga::syntax::parse;

fn db(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("naga-live-{}-{name}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn program(tables: &str) -> naga::ast::Program {
    let p = parse(&format!("{tables}\nseed notes = [{{title: \"seeded\"}}]\n"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    p
}

fn titles(s: &mut Store, p: &naga::ast::Program) -> Vec<String> {
    s.snapshot(p).unwrap()["notes"].iter().map(|r| r.as_rec().unwrap()["title"].text()).collect()
}

#[test]
fn safe_changes_apply_and_keep_the_data() {
    let path = db("safe");
    let v1 = program("table notes :: {title: Text}");
    let (mut s, m) = Store::open(&v1, &path).unwrap();
    assert_eq!(m.applied, ["created notes"]);
    s.insert(&v1.tables["notes"], &naga::value::Value::Rec(std::sync::Arc::new([("title".into(), naga::value::Value::Text("mine".into()))].into_iter().collect()))).unwrap();

    let v2 = program("table notes :: {title: Text, note: Text?}\ntable tags :: {name: Text}");
    assert_eq!(s.migrate(&v2).unwrap(), Migration { applied: vec!["added notes.note".into(), "created tags".into()], pending: vec![] });
    assert_eq!(titles(&mut s, &v2), ["seeded", "mine"]);

    let v3 = program("table notes :: {title: Text, note: Num?, rank: Int}");
    let m = s.migrate(&v3).unwrap();
    assert!(m.applied.is_empty());
    assert_eq!(m.pending, ["notes.note was TEXT, is now REAL", "notes.rank is new and required"]);
    let v4 = program("table notes :: {note: Text?}");
    assert_eq!(s.migrate(&v4).unwrap().pending, ["notes.title is gone from the program"]);

    let v5 = program("table notes :: {title: Text, note: Num?}");
    assert_eq!(s.migrate(&v5).unwrap().pending, ["notes.note was TEXT, is now REAL"]);
    s.reset(&v5, "notes").unwrap();
    assert_eq!(s.migrate(&v5).unwrap(), Migration::default());
    assert_eq!(titles(&mut s, &v5), ["seeded"]);
}

#[test]
fn reopening_migrates_too() {
    let path = db("reopen");
    let v1 = program("table notes :: {title: Text}");
    drop(Store::open(&v1, &path).unwrap());
    let v2 = program("table notes :: {title: Text, done: Bool?}");
    let (_, m) = Store::open(&v2, &path).unwrap();
    assert_eq!(m.applied, ["added notes.done"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn requests_run_in_parallel_and_every_write_lands() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let src = "table notes :: {title: Text}\nroute POST /notes {title: Text} = add\nroute GET /notes = all\nadd = +{note: {title: body.title}} notes.insert +{status: 201, json: note}\nall = notes.all +{status: 200, json: len(notes)}\n";
    let app = naga::serve::start(src.into(), db("parallel")).unwrap();
    let posts = (0..64).map(|i| {
        let app = app.clone();
        tokio::spawn(async move {
            let req = Request::post("/notes").body(Body::from(format!("{{\"title\":\"n{i}\"}}"))).unwrap();
            app.oneshot(req).await.unwrap().status().as_u16()
        })
    });
    let statuses: Vec<u16> = futures_join(posts).await;
    assert!(statuses.iter().all(|s| *s == 201), "{statuses:?}");
    let res = app.oneshot(Request::get("/notes").body(Body::empty()).unwrap()).await.unwrap();
    let body = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    assert_eq!(&body[..], b"64");
}

async fn futures_join(hs: impl Iterator<Item = tokio::task::JoinHandle<u16>>) -> Vec<u16> {
    let mut out = vec![];
    for h in hs.collect::<Vec<_>>() {
        out.push(h.await.unwrap());
    }
    out
}

#[test]
fn a_published_program_answers_the_next_request() {
    use naga::serve::{Live, Raw};
    let v1 = std::sync::Arc::new(parse("route GET /x = w\nw = +{status: 200, json: 1}\n"));
    let (live, _) = Live::open(v1, &db("publish")).unwrap();
    let get = || live.answer(&Raw { method: "GET".into(), path: "/x".into(), query: String::new(), body: String::new() }).json;
    assert_eq!(get(), "1");
    live.publish(std::sync::Arc::new(parse("route GET /x = w\nw = +{status: 200, json: 2}\n")));
    assert_eq!(get(), "2");
}

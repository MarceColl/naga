//! Live pages end to end: a GET mounts a session, a websocket carries clicks in and pages out.
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn db(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("naga-livepage-{}-{name}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_click_runs_the_word_and_the_page_comes_back() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/counter.nag")).unwrap();
    let app = naga::serve::start(src, db("counter")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // mount: the page, rendered, with its session
    let page = get(addr, "/counter").await;
    assert!(page.contains("<h1>0</h1>") && page.contains(r#"data-naga-click="inc""#), "{page}");
    let id: String = page.split("data-s=\"").nth(1).unwrap().chars().take_while(|c| c.is_ascii_digit()).collect();
    assert!(get(addr, "/naga/live.js").await.contains("morphdom"));

    // two clicks, two pages
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/naga/live?s={id}")).await.unwrap();
    for n in ["1", "2"] {
        ws.send(Message::Text(r#"{"name":"inc"}"#.into())).await.unwrap();
        let Some(Ok(Message::Text(html))) = ws.next().await else { panic!("no page") };
        assert!(html.contains(&format!("<h1>{n}</h1>")), "{html}");
    }
    ws.send(Message::Text(r#"{"name":"dec"}"#.into())).await.unwrap();
    let Some(Ok(Message::Text(html))) = ws.next().await else { panic!("no page") };
    assert!(html.contains("<h1>1</h1>"), "{html}");

    // a second page is a second session, starting over
    assert!(get(addr, "/counter").await.contains("<h1>0</h1>"));
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_from_one_page_shows_on_every_page_and_so_does_a_new_program() {
    let src = "table notes :: {text: Text}\nlive /board = board\n\
board {event: {name: :add}} = +{note: {text: event.form.text}} notes.insert -{note} render\nboard = render\n\
render = notes.all @notes( +{li: [:li, text]} ) +{html: [:main, [:ul, notes.li]]} -{notes}\n";
    let (live, _) = naga::serve::Live::open(std::sync::Arc::new(naga::syntax::parse(src)), &db("board")).unwrap();
    let app = naga::serve::router(live.clone(), |_, _| {});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut a = open(addr, "/board").await;
    let mut b = open(addr, "/board").await;
    a.send(Message::Text(r#"{"name":"add","form":{"text":"hello <you>"}}"#.into())).await.unwrap();
    for ws in [&mut a, &mut b] {
        let html = next(ws).await;
        assert!(html.contains("<li>hello &lt;you&gt;</li>"), "{html}");
    }

    // a new image: every page shows what the new word renders, on the state it had
    live.publish(std::sync::Arc::new(naga::syntax::parse(&src.replace("[:li, text]", "[:li, \"• \", text]"))));
    for ws in [&mut a, &mut b] {
        let html = next(ws).await;
        assert!(html.contains("<li>• hello &lt;you&gt;</li>"), "{html}");
    }
}

async fn open(addr: std::net::SocketAddr, path: &str) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let page = get(addr, path).await;
    let id: String = page.split("data-s=\"").nth(1).unwrap().chars().take_while(|c| c.is_ascii_digit()).collect();
    tokio_tungstenite::connect_async(format!("ws://{addr}/naga/live?s={id}")).await.unwrap().0
}

async fn next(ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) -> String {
    match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(Message::Text(html)))) => html.to_string(),
        other => panic!("no page: {other:?}"),
    }
}

async fn get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

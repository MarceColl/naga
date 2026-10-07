//! `route` declarations and `naga serve`. naga only: the JS reference has no routes.
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use naga::syntax::parse;
use std::path::PathBuf;
use tower::ServiceExt;

fn ledger() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/ledger-server.nag")).unwrap()
}

fn fresh_db(name: &str) -> PathBuf {
    let db = std::env::temp_dir().join(format!("naga-{}-{name}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    db
}

async fn send(app: &axum::Router, method: &str, uri: &str, body: &str) -> (u16, String) {
    let req = Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    (status, String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap())
}

#[tokio::test]
async fn the_ledger_answers_over_http() {
    let app = naga::serve::start(ledger(), fresh_db("ledger")).unwrap();
    let cases = [
        ("GET", "/accounts/1", "", 200, r#"{"id":1,"name":"Checking","kind":"asset","archived":false,"balance":440000,"#),
        ("GET", "/accounts/abc", "", 400, r#"{"error":"id must be Int"}"#),
        ("GET", "/accounts/99", "", 404, r#""no such account""#),
        ("POST", "/accounts", r#"{"name":"Pets","kind":"Animal"}"#, 422, r#""kind must be"#),
        ("POST", "/accounts", r#"{"name":"Pets"}"#, 400, r#"{"error":"kind is required"}"#),
        ("POST", "/accounts", r#"{"name":7,"kind":"asset"}"#, 400, r#"{"error":"name must be Text"}"#),
        ("POST", "/accounts", "[1]", 400, r#"{"error":"the body must be a JSON object"}"#),
        ("POST", "/transactions", r#"{"date":"d","payee":"p","postings":{}}"#, 400, r#"{"error":"postings must be [{account-id: Int, amount: Num}]"}"#),
        ("POST", "/transactions", r#"{"date":"d","payee":"p","postings":[{"account-id":1,"amount":5},{"account-id":2.5,"amount":-5}]}"#, 400, r#"{"error":"postings[1].account-id must be Int"}"#),
        ("POST", "/transactions", r#"{"date":"d","payee":"p","postings":[{"amount":5}]}"#, 400, r#"{"error":"postings[0].account-id is required"}"#),
        ("POST", "/transactions", r#"{"date":"2026-10-02","payee":"Bakery","postings":[{"account-id":5,"amount":300,"note":"x"},{"account-id":1,"amount":-300}]}"#, 201, r#"{"id":4,"date":"2026-10-02","payee":"Bakery"}"#),
        ("POST", "/accounts", r#"{"name":"Cafés","kind":"expense","extra":1}"#, 201, r#"{"id":8,"name":"Cafés","kind":"expense","archived":false}"#),
        ("PATCH", "/accounts/2", r#"{"archived":true}"#, 200, r#"{"id":2,"name":"Savings","kind":"asset","archived":true}"#),
        ("DELETE", "/accounts/8", "", 204, ""),
        ("DELETE", "/accounts/1", "", 409, r#""account has postings"#),
        ("GET", "/transactions?account=6", "", 200, r#"[{"tx-id":3,"account-id":6,"amount":120000,"date":"2026-09-27","payee":"Landlord"}]"#),
        ("GET", "/reports/income?from=2026-09-01", "", 400, r#"{"error":"to is required"}"#),
        ("GET", "/reports/income?from=2026-09-01&to=2026-11-01", "", 200, r#"{"income":310000,"expenses":120300,"net":189700,"#),
        ("GET", "/", "", 200, r#"{"routes":["GET /accounts","GET /accounts/:id:Int","POST /accounts {name: Text, kind: Const}","#),
        ("GET", "/nope", "", 404, r#"{"error":"no route GET /nope","routes":["GET /accounts","#),
        ("PUT", "/accounts/1", "", 405, ""),
    ];
    for (method, uri, body, status, prefix) in cases {
        let (s, b) = send(&app, method, uri, body).await;
        assert_eq!((s, b.starts_with(prefix)), (status, true), "{method} {uri} → {s} {b}");
    }
}

#[tokio::test]
async fn tables_outlive_the_server_and_seed_once() {
    let db = fresh_db("again");
    let app = naga::serve::start(ledger(), db.clone()).unwrap();
    assert_eq!(send(&app, "POST", "/accounts", r#"{"name":"Pets","kind":"expense"}"#).await.0, 201);
    let app = naga::serve::start(ledger(), db).unwrap();
    let (_, accounts) = send(&app, "GET", "/accounts", "").await;
    assert_eq!(accounts.matches("\"name\"").count(), 8, "{accounts}");
}

#[tokio::test]
async fn a_route_that_leaves_no_status_is_a_500() {
    let src = "route GET /x = nop-route\nnop-route = +{json: 1}\n";
    let app = naga::serve::start(src.into(), fresh_db("nostatus")).unwrap();
    assert_eq!(send(&app, "GET", "/x", "").await, (500, r#"{"error":"the route left no status"}"#.into()));
}

#[test]
fn routes_parse() {
    let p = parse("route GET /a/:id:Int/b?q:Text&n:Num? {x: Const, y: Json?} = w\nw = nop\n");
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let r = &p.routes[0];
    assert_eq!((&*r.verb, r.path.as_str(), r.pattern(), &*r.word), ("GET", "/a/:id:Int/b", "/a/{id}/b".to_string(), "w"));
    let names = |ds: Vec<&naga::ast::FieldDecl>| ds.iter().map(|d| d.show(":")).collect::<Vec<_>>().join(" ");
    assert_eq!(names(r.params().collect()), "id:Int q:Text n:Num?");
    assert_eq!(names(r.body.iter().flatten().collect()), "x:Const y:Json?");
    assert_eq!(r.decl(), "GET /a/:id:Int/b?q:Text&n:Num? {x: Const, y: Json?}");
    assert!(parse("route = nop\n").routes.is_empty(), "a word may still be called route");
    let p = parse("route POST /t {lines: [{sku: Const, qty: Int}], tags: [Text]?, meta: {by: Text}?}\n  = w\nw = nop\n");
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    assert_eq!(p.routes[0].decl(), "POST /t {lines: [{sku: Const, qty: Int}], tags: [Text]?, meta: {by: Text}?}");
}

#[test]
fn route_errors() {
    let first = |src: &str| parse(src).errors.first().map(|e| format!("{}:{} {}", e.line, e.col, e.msg)).unwrap_or_default();
    assert_eq!(first("route FETCH /a = w\nw = nop"), "1:6 a route starts with a verb: GET, POST, PUT, PATCH, DELETE");
    assert_eq!(first("route GET a = w\nw = nop"), "1:10 a route path starts with /, got “a”");
    assert_eq!(first("route GET /a/:id = w\nw = nop"), "1:10 “id”: write name:Type, with Type one of Int, Num, Text, Bool, Const, Json");
    assert_eq!(first("route GET /a?time:Text = w\nw = nop"), "1:10 route GET /a: time can't be a param, the router sets it");
    assert_eq!(first("route GET /a/:x:Int?x:Int = w\nw = nop"), "1:10 route GET /a/:x:Int: x can't be a param, it's already a param");
    assert_eq!(first("route POST /a {x: [Int} = w\nw = nop"), "1:22 expected “]” to close the list type, found “}”");
    assert_eq!(first("route POST /a {x: Date} = w\nw = nop"), "1:18 types are Int, Num, Text, Bool, Const, Json, [Type] and {field: Type} (add ? when it may be missing, like Text?)");
    assert_eq!(first("table ts :: {x: [Int]}"), "1:16 column types are Int, Num, Text, Bool, Const, Json (add ? for an optional column, like Text?)");
    assert_eq!(first("route GET /a = missing"), "1:0 route GET /a: missing isn't defined");
    assert_eq!(first("route GET /a/:x:Int = w\nroute POST /a/:x:Int = w\nroute POST /a/:x:Text = w\nw = nop"), "3:0 route POST /a/:x:Text is already declared on line 2");
    assert_eq!(first("route GET /a/:x:Int = w\nroute POST /a/:y:Int = w\nw = nop"), "2:0 route POST /a/:y:Int: name the params as on line 1 (/a/:x:Int)");
}

#[test]
fn routes_are_checked_as_call_sites() {
    let errors = |src: &str| {
        let p = parse(src);
        let r = naga::check::Checker::default().check(&p, None);
        r.errors.iter().map(|e| format!("{}:{} {}", e.line, e.col, e.msg)).collect::<Vec<_>>()
    };
    assert_eq!(errors("route GET /x = w\nw = +{json: 1}\n"), ["1:0 route GET /x: w can finish without status"]);
    assert_eq!(
        errors("route GET /x/:id:Int = w\nw {id: 1} = +{status: 200, json: 1}\nw = +{json: 2}\n"),
        ["1:0 route GET /x/:id:Int: w can finish without status"]
    );
    assert_eq!(
        errors("route GET /x?q:Text? = w\nw = +{status: 200, json: q}\n"),
        ["2:4 w needs q, but nothing on this path provides it (w) (when route GET /x runs)"]
    );
    assert!(errors("route POST /x {n: Num} = w\nw = +{status: 201, json: body.n + 1}\n").is_empty());
    let nested = |read: &str| errors(&format!("route POST /x {{items: [{{n: Num}}]}} = w\nw = @body( @items( +{{m: {read} * 2}} ) ) +{{status: 200, json: body}}\n"));
    assert!(nested("n").is_empty());
    assert_eq!(nested("k"), ["2:19 w needs body.items.k, but nothing on this path provides it (w) (when route POST /x runs)"]);
    assert!(errors(&ledger()).is_empty(), "{:?}", errors(&ledger()));
}

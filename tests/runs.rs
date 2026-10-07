//! `run` declarations, and what the editor reads from the engine. naga only.
use naga::runs::{Verdict, run};
use naga::syntax::parse;

const NOTES: &str = r#"table notes :: {text: Text}
route GET /notes/:id:Int = get-note
route POST /notes {text: Text} = post-note
route GET /open = open
route GET /boom = boom
get-note = notes.get found
found {-missing} = +{status: 404, json: "no such note"}
found {note} = +{status: 200, json: note}
post-note = +{note: {text: body.text}} notes.insert +{status: 201, json: note}
open = ? answer something
boom = +{status: 200, json: 1/0}
"#;

fn outcomes(runs: &str) -> Vec<(Verdict, Vec<String>, Option<String>)> {
    let p = parse(&format!("{NOTES}{runs}"));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    p.runs.iter().map(|r| run(&p, r)).map(|o| (o.verdict, o.said, o.detail)).collect()
}

#[test]
fn verdicts() {
    let o = outcomes(
        r#"
run "passes, key order aside"
  POST /notes {text: "a"} → 201 {text: "a", id: 1}
  GET /notes/1 → 200
run "wrong status"
  POST /notes {text: "a"} → 200
run "wrong json"
  POST /notes {text: "a"} → 201 {id: 2, text: "a"}
run "a hole"
  GET /open → 200
run "an error"
  GET /boom → 200
run "a 400 is an answer"
  POST /notes {txt: "a"} → 400 {error: "text is required"}
"#,
    );
    let verdicts: Vec<Verdict> = o.iter().map(|x| x.0).collect();
    assert_eq!(verdicts, [Verdict::Pass, Verdict::Fail, Verdict::Fail, Verdict::Waits, Verdict::Error, Verdict::Pass]);
    assert_eq!(o[0].1, [r#"201 {"id":1,"text":"a"}"#, r#"200 {"id":1,"text":"a"}"#]);
    assert_eq!(o[3].2.as_deref(), Some("reached a hole: answer something"));
    assert_eq!(o[4].2.as_deref(), Some("division by zero (line 11)"));
}

#[test]
fn each_run_starts_from_the_seeds() {
    let o = outcomes("\nrun \"one\"\n  POST /notes {text: \"a\"} → 201 {id: 1, text: \"a\"}\nrun \"two\"\n  POST /notes {text: \"b\"} → 201 {id: 1, text: \"b\"}\n");
    assert_eq!((o[0].0, o[1].0), (Verdict::Pass, Verdict::Pass));
}

#[test]
fn run_errors() {
    let first = |src: &str| parse(&format!("{NOTES}{src}")).errors.first().map(|e| format!("{}:{} {}", e.line, e.col, e.msg)).unwrap_or_default();
    assert_eq!(first("run \"x\"\n  GET /nope → 200\n"), "13:0 GET /nope matches no route");
    assert_eq!(first("run \"x\"\n  GET /open {a: 1} → 200\n"), "13:0 route GET /open takes no body");
    assert_eq!(first("run \"x\"\n  GET /open 200\n"), "13:12 expected “→” before the status the request should get, like  GET /accounts/1 → 200");
    assert_eq!(first("run \"x\"\n  GET /open → 99\n"), "13:14 expected a status from 100 to 599, like 200");
    assert_eq!(first("run \"x\"\n  open → 200\n"), "13:2 a step starts with GET, POST, PUT, PATCH, DELETE, LIVE or EVENT");
    assert_eq!(first("run \"x\"\n"), "12:0 run \"x\" has no requests: add lines like  GET /accounts/1 → 200");
}

#[test]
fn the_ledger_runs_pass_and_show_their_tables() {
    let p = parse(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/ledger-server.nag")).unwrap());
    assert_eq!(p.runs.len(), 7);
    for r in &p.runs {
        let o = run(&p, r);
        assert_eq!(o.verdict, Verdict::Pass, "{}: {:?} {:?}", r.name, o.said, o.detail);
    }
    let o = run(&p, &p.runs[0]);
    assert_eq!((o.seed["transactions"].len(), o.end["transactions"].len(), o.end["postings"].len()), (3, 4, 8));
    assert!(o.sql.iter().any(|l| l.starts_with("INSERT INTO \"transactions\"")), "{:?}", o.sql);
}

#[test]
fn statements_know_where_they_end_and_the_checker_shapes_around_them() {
    let p = parse("main = +{n: 3} countdown\n\ncountdown {n: 0} = say \"liftoff\"\ncountdown        = say \"{n}…\" n-1 countdown\n");
    let ends: Vec<String> = p.words["countdown"].clauses[1].body.iter().map(|s| format!("{}:{}-{}:{}", s.pos.line, s.pos.col, s.end.line, s.end.col)).collect();
    assert_eq!(ends, ["4:19-4:29", "4:30-4:33", "4:34-4:43"]);
    let r = naga::check::Checker::default().check(&p, Some(&"main".into()));
    let text = |m: &std::collections::HashMap<(usize, usize), Vec<naga::check::Point>>, at| m[&at].iter().map(|x| x.text.clone()).collect::<Vec<_>>();
    assert_eq!(text(&r.before, (1, 7)), ["{..r, ¬n}"]);
    assert_eq!(text(&r.after, (1, 7)), ["{..r, n: Num}"]);
    assert_eq!(text(&r.before, (4, 30)), ["{..r, n}"]);
}

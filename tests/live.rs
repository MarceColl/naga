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
    s.insert(&v1.tables["notes"], &naga::value::Value::Rec(std::rc::Rc::new([("title".into(), naga::value::Value::Text("mine".into()))].into_iter().collect()))).unwrap();

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

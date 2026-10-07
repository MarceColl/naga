//! Names ignore case; constants are `:name`. naga only: the JS reference stopped here.
use naga::report::report;

fn run(src: &str) -> String {
    report(src).lines().filter(|l| l.starts_with("> ") || l.starts_with("parse") || l.starts_with("run \"") || l.starts_with("  ✗")).collect::<Vec<_>>().join("\n")
}

#[test]
fn names_ignore_case() {
    let out = run("MAIN = +{N: 2} Twice SHOW\ntwice = n=N * 2\n");
    assert_eq!(out, "> {n: 4}");
}

#[test]
fn constants_are_colon_names_in_any_case() {
    let out = run("main = +{kind: :Asset, all: [:a, :B]} check show\ncheck {kind: :asset} when kind == :ASSET = +{ok: true, label: \"Kind {kind}\"}\ncheck = +{ok: false}\n");
    assert_eq!(out, "> {kind: :asset, all: [:a, :b], ok: true, label: \"Kind asset\"}");
    assert_eq!(run("main = +{a: 1} +{b:a} show\n"), "> {a: 1, b: 1}", "a colon after a name is still a field's");
    assert_eq!(run("main = +{s: \"Asset\"} show\n"), "> {s: \"Asset\"}", "strings keep their case");
    assert!(run("main = :asset\n").contains(":asset is a constant, not a step"));
}

#[test]
fn routes_runs_and_tables_ignore_case_too() {
    let src = "TABLE Notes :: {Title: TEXT, Kind: const}\nROUTE post /NOTES {title: text, kind: CONST} = Add\nroute GET /notes/:id:INT = get-one\nadd = +{note: {title: body.title, kind: body.kind}} notes.insert +{status: 201, json: note}\nget-one = notes.get found\nfound {-missing} = +{status: 404, json: \"no\"}\nfound {note} = +{status: 200, json: note}\nrun \"x\"\n  Post /notes {Title: \"Hi\", KIND: \"Idea\"} → 201 {id: 1, title: \"Hi\", kind: :idea}\n  get /notes/1 → 200\n";
    assert_eq!(run(src), "run \"x\" pass");
}

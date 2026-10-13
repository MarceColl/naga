//! Every case survives `Program` → records → `Program`: the decoded program prints its golden.
use naga::check::Checker;
use naga::machine::{Session, Step};
use naga::records::{clause, program, un_clause, un_program};
use naga::report::report_program;
use naga::syntax::parse;
use naga::value::{Value, eqv};
use std::fs;
use std::path::Path;

#[test]
fn every_case_round_trips_to_its_golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let (mut n, mut failed) = (0, vec![]);
    for e in fs::read_dir(root.join("cases")).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let want = fs::read_to_string(root.join("golden").join(format!("{name}.out"))).unwrap();
        let recs = program(&parse(&fs::read_to_string(&path).unwrap()));
        n += 1;
        match un_program(&recs) {
            Err(e) => failed.push(format!("{name}: does not decode: {e}")),
            Ok(p) if !eqv(&program(&p), &recs) || program(&p).show() != recs.show() => failed.push(format!("{name}: encoding the decoded program gives different records")),
            Ok(p) if report_program(&p) != want => failed.push(format!("{name}: report differs from the golden")),
            Ok(_) => {}
        }
    }
    failed.sort();
    println!("{}/{n} cases round-trip to their golden", n - failed.len());
    assert!(failed.is_empty(), "{}/{n} differ:\n{}", failed.len(), failed.join("\n"));
}

/// The records carry the clause's source text and positions too: the checker caches by `chunk_text`
/// and the diagnostics quote `head_src` and `guard_src`, and no golden would notice if they went missing.
#[test]
fn chunk_text_and_positions_survive() {
    let mut seen = 0;
    for (name, src) in cases() {
        let (before, after) = (parse(&src), un_program(&program(&parse(&src))).unwrap());
        assert_eq!(before.words.len(), after.words.len(), "{name}: words");
        for (w, bw) in &before.words {
            let aw = &after.words[w];
            assert_eq!(bw.clauses.len(), aw.clauses.len(), "{name}: {w} clauses");
            for (i, (b, a)) in bw.clauses.iter().zip(&aw.clauses).enumerate() {
                let at = format!("{name}: {w} clause {i}");
                assert_eq!(b.chunk_text, a.chunk_text, "{at}: chunk_text");
                assert_eq!(b.chunk_line, a.chunk_line, "{at}: chunk_line");
                assert_eq!(b.head_src, a.head_src, "{at}: head_src");
                assert_eq!(b.guard_src, a.guard_src, "{at}: guard_src");
                assert_eq!(b.line, a.line, "{at}: line");
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "no clauses compared");
}

/// The checker's memo keys on the chunk text: a decoded program is the same clauses to it, so a
/// checker that has seen the parsed program recomputes nothing for the decoded one.
#[test]
fn the_checker_memo_sees_the_same_clauses() {
    let mut warmed = 0;
    for (name, src) in cases() {
        let p = parse(&src);
        let mut checker = Checker::default();
        let first = checker.check(&p, None);
        warmed += first.stats.recomputed.len();
        let second = checker.check(&un_program(&program(&p)).unwrap(), None);
        assert!(second.stats.recomputed.is_empty(), "{name}: recomputed {:?}", second.stats.recomputed);
    }
    assert!(warmed > 0, "the first check never filled the memo");
}

/// Every case's name and source.
fn cases() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("cases");
    let mut all: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (p.file_stem().unwrap().to_string_lossy().to_string(), fs::read_to_string(&p).unwrap()))
        .collect();
    all.sort();
    all
}

/// What the cases never write: `not`, `or`, `⫽{…}` and `raise` steps, a `{}` head, a pattern that matches nil.
#[test]
fn what_the_cases_miss_round_trips_too() {
    let src = "main = +{a: 1, x: nil} ⫽{a: 2} empty check say not (a == 1) or a > 1 raise \"stop\"\nempty {} = nop\ncheck {x: nil} = say \"nil x\"\ncheck = nop\n";
    let recs = program(&parse(src));
    let p = un_program(&recs).unwrap();
    assert_eq!(program(&p).show(), recs.show());
    assert_eq!(report_program(&p), naga::report::report(src));
    assert!(p.words["empty"].clauses[0].pattern.as_ref().is_some_and(|x| x.is_empty()), "a {{}} head stays closed");
    assert!(p.words["main"].clauses[0].pattern.is_none(), "a head without a pattern stays open");
    assert!(matches!(p.words["check"].clauses[0].pattern.as_ref().unwrap()[0].lit, Some(Value::Nil)), "x: nil stays a literal");
}

/// The decoded program is what runs: an edit to the records shows in the report.
#[test]
fn an_edit_to_the_records_changes_the_run() {
    let recs = program(&parse("main = say \"before\"\n"));
    let after = naga::value::rec([("k".into(), Value::Const("lit".into())), ("v".into(), Value::Text("after".into()))].into_iter().collect());
    let recs = set(&recs, &["words", "main", "clauses", "0", "body", "0", "expr"], after);
    let out = report_program(&un_program(&recs).unwrap());
    assert!(out.contains("> after") && !out.contains("> before"), "{out}");
}

/// `v` with the value at `path` (field names, list indexes) replaced.
fn set(v: &Value, path: &[&str], new: Value) -> Value {
    let Some((k, rest)) = path.split_first() else { return new };
    match v {
        Value::Rec(r) => {
            let mut r = r.as_ref().clone();
            r.insert((*k).into(), set(&r[*k], rest, new));
            naga::value::rec(r)
        }
        Value::List(xs) => {
            let (i, mut xs) = (k.parse::<usize>().unwrap(), xs.as_ref().clone());
            xs[i] = set(&xs[i], rest, new);
            naga::value::list(xs)
        }
        v => panic!("can't go into {}", v.show()),
    }
}

/// A clause record as naga source, so a naga program can hold one.
fn lit(v: &Value) -> String {
    match v {
        Value::Text(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('{', "\\{").replace('\n', "\\n").replace('\t', "\\t")),
        Value::List(xs) => format!("[{}]", xs.iter().map(lit).collect::<Vec<_>>().join(", ")),
        Value::Rec(r) => format!("{{{}}}", r.iter().map(|(k, x)| format!("{k}: {}", lit(x))).collect::<Vec<_>>().join(", ")),
        v => v.show(),
    }
}

/// A naga word walks a clause record with today's `keys`, `head`, `tail` and `len`, drops its first
/// statement with `⫽`, and the record it leaves decodes back into a clause.
#[test]
fn a_naga_word_walks_and_edits_a_clause_record() {
    let p = parse("greet {name} when name ≠ \"\" = say \"hi {name}\" show\n");
    let it = lit(&clause(&p.words["greet"].clauses[0]));
    let src = format!(
        "main = +{{it: {it}}} say keys(it) say len(it.body) say [head(it.body).k, head(tail(it.body)).k] it=(it ⫽ {{body: tail(it.body)}})\n"
    );
    let probe = naga::syntax::parse(&src);
    assert!(probe.errors.is_empty(), "{:?}", probe.errors);
    let mut s = Session::new(&probe, &"main".into());
    s.run(Step::Continue, &[], 200000);
    println!("{src}{}", s.log.join("\n"));
    assert_eq!(s.log, ["[:line, :head, :pattern, :guard, :guard_src, :chunk, :body]", "2", "[:say, :show]"]);
    let edited = un_clause(&s.state().subj["it"]).unwrap();
    assert_eq!(edited.body.iter().map(|s| s.kind_name()).collect::<Vec<_>>(), ["show"]);
    assert_eq!((edited.head_src.as_str(), edited.guard_src.as_deref()), ("greet {name} when name ≠ \"\"", Some("name ≠ \"\"")));
}

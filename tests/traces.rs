//! Stepping, time travel and incremental rechecks, against the reference traces.
use naga::check::Checker;
use naga::machine::{Session, State, Step};
use naga::syntax::parse;
use std::fs;
use std::path::Path;

fn golden(name: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name)).unwrap()
}

fn invoice() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/invoice.nag")).unwrap()
}

fn desc(s: &State, at: usize) -> String {
    let next = s.next_stmt().map_or("-".into(), |n| format!("{}{}@{}", n.kind_name(), n.name().map(|x| format!(":{x}")).unwrap_or_default(), n.pos.line));
    format!("{} lvl{} next={next} ctx={} at={at}", s.status.name(), s.top_lvl(), s.context().join(">"))
}

#[test]
fn stepping_matches_the_reference() {
    let p = parse(&invoice());
    let mut s = Session::new(&p, &"main".into());
    let mut out = vec![];
    let plan: [(&str, &[usize]); 19] = [
        ("into", &[]), ("into", &[]), ("into", &[]), ("into", &[]), ("over", &[]), ("into", &[]), ("into", &[]), ("into", &[]),
        ("out", &[]), ("over", &[]), ("out", &[]), ("restart", &[]), ("continue", &[21]), ("continue", &[21]), ("into", &[]),
        ("out", &[]), ("continue", &[12, 25]), ("continue", &[12, 25]), ("continue", &[]),
    ];
    for (mode, bps) in plan {
        let step = match mode {
            "restart" => {
                s.restart();
                out.push(format!("restart {}", desc(s.state(), s.index())));
                continue;
            }
            "into" => Step::Into,
            "over" => Step::Over,
            "out" => Step::Out,
            _ => Step::Continue,
        };
        s.run(step, bps, 200000);
        let bp = if bps.is_empty() { String::new() } else { format!(" bp{}", bps.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")) };
        out.push(format!("{mode}{bp} {} pause={}", desc(s.state(), s.index()), s.pause));
    }
    s.seek(10);
    out.push(format!("seek {}", desc(s.state(), s.index())));
    s.back();
    out.push(format!("back {}", desc(s.state(), s.index())));
    s.restart();
    out.push(format!("restart {}", desc(s.state(), s.index())));
    s.run(Step::Continue, &[], 200000);
    out.push(format!("again {}", desc(s.state(), s.index())));
    assert_eq!(out.join("\n") + "\n", golden("stepping.out"));
}

#[test]
fn rechecks_match_the_reference() {
    let inv = invoice();
    let mut c = Checker::default();
    let mut out = vec![];
    let mut go = |label: &str, src: String| {
        let r = c.check(&parse(&src), Some(&"main".into()));
        let names: Vec<&str> = r.stats.recomputed.iter().map(|n| &**n).collect();
        out.push(format!("{label}: recomputed [{}] reused {} · errors {}", names.join(", "), r.stats.reused.len(), r.errors.len()));
    };
    go("first run", inv.clone());
    go("same text", inv.clone());
    go("edit report message", inv.replacen("voided\"", "was voided\"", 1));
    go("edit fail body, same signature", inv.replacen("fail = tries+1", "fail = tries+2", 1));
    go("add a blank line at the top", format!("\n{inv}"));
    go("change finalize signature", inv.replacen("finalize = +{due: now + 30, tries: 0}", "finalize = +{due: now + 30, tries: 0, note: \"x\"}", 1));
    go("back to the start", inv.clone());
    assert_eq!(out.join("\n") + "\n", golden("incremental.out"));
}

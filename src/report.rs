//! The text `naga run` prints; `spec/report.js` prints the same from the reference engine.
use crate::check::Checker;
use crate::machine::{Session, Step};
use crate::syntax::parse;

pub fn report(src: &str) -> String {
    report_program(&parse(src))
}

/// The same report for a program that was already parsed, or built some other way.
pub fn report_program(p: &crate::ast::Program) -> String {
    let mut out = vec![];
    out.extend(p.errors.iter().map(|e| format!("parse {}:{} {}", e.line, e.col, e.msg)));
    out.extend(p.warnings.iter().map(|w| format!("warn {} {}", w.line, w.msg)));
    out.extend(p.holes.iter().map(|h| format!("hole {} {}: {}", h.line, h.word, h.note)));
    // without main, the first word runs, unless routes are the way in
    let first = p.words.keys().next().filter(|_| p.routes.is_empty());
    let entry = if p.words.contains_key("main") { Some("main".into()) } else { first.cloned() };
    if let Some(entry) = &entry {
        let mut s = Session::new(p, entry);
        s.run(Step::Continue, &[], 200000);
        let st = s.state();
        let err = st.error.as_ref().map(|e| format!(": {} @{}", e.msg, e.line)).unwrap_or_default();
        out.push(format!("run {} after {} steps{err}", st.status.name(), s.index()));
        if let Some(e) = &st.error {
            out.extend(e.details.iter().map(|d| format!("  why {d}")));
        }
        if let Some(h) = &st.hole {
            out.push(format!("  hole {h}"));
        }
        out.extend(s.log[..st.out_len].iter().map(|l| format!("> {l}")));
        out.push(format!("subj {}", crate::value::show_rec(&st.subj)));
        out.push(format!("pause {}", s.pause));
        if let Some(store) = &s.store {
            out.extend(store.log.iter().map(|(t, l)| format!("sql [{t}] {l}")));
        }
    }
    let r = Checker::default().check(p, entry.as_ref());
    out.push(format!("check {} errors", r.errors.len()));
    out.extend(r.errors.iter().map(|e| format!("  ✗ {}:{} {}", e.line, e.col, e.msg)));
    for n in p.words.keys() {
        let cases = r.signatures.get(n).map(Vec::as_slice).unwrap_or_default();
        if cases.is_empty() {
            out.push(format!("sig {n} : (none)"));
        }
        for c in cases {
            let res = match &c.fail {
                Some(f) => format!("✗ {f}"),
                None if c.out.is_empty() => "never returns".into(),
                None => c.out.join(" | "),
            };
            out.push(format!("sig {n} : {} → {res}{}", c.input, if c.guard { "   (guarded)" } else { "" }));
        }
    }
    for r in &p.runs {
        let o = crate::runs::run(p, r);
        out.push(format!("run \"{}\" {}", r.name, o.verdict.name()));
        out.extend(o.said.iter().map(|l| format!("  > {l}")));
        if o.verdict == crate::runs::Verdict::Fail {
            out.extend(o.want.iter().map(|l| format!("  want {l}")));
        }
        out.extend(o.detail.iter().map(|d| format!("  why {d}")));
    }
    out.push(String::new());
    out.join("\n")
}

/// The checker's errors that can say more, written out in full: each with the code it names
/// (marked underneath), the paths that lead to it, and a fix when there is one.
pub fn explain(src: &str) -> String {
    use crate::check::Part;
    let p = parse(src);
    if !p.errors.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = src.split('\n').collect();
    let entry = p.words.contains_key("main").then(|| crate::value::Name::from("main"));
    let r = Checker::default().check(&p, entry.as_ref());
    let mut out = String::new();
    for e in &r.errors {
        let Some(d) = &e.detail else { continue };
        out += &format!("\n── {} {}\n\n", e.msg.to_uppercase(), "─".repeat(60usize.saturating_sub(e.msg.chars().count())));
        // each line the refs are on, with ^ under them
        let mut shown: Vec<usize> = d.refs.iter().map(|s| s.line).collect();
        shown.sort();
        shown.dedup();
        for l in shown {
            out += &format!("{l:>5} │ {}\n", lines.get(l - 1).unwrap_or(&""));
            let mut marks = vec![' '; lines.get(l - 1).map_or(0, |x| x.chars().count()) + 1];
            for s in d.refs.iter().filter(|s| s.line == l) {
                let end = if s.end_line == l { s.end_col } else { marks.len() - 1 };
                let n = marks.len();
                marks[s.col.min(n)..end.min(n)].iter_mut().for_each(|m| *m = '^');
            }
            out += &format!("      │ {}\n", marks.iter().collect::<String>().trim_end());
        }
        out += "\n";
        for line in &d.lines {
            let text: String = line
                .iter()
                .map(|part| match part {
                    Part::Text(t) | Part::Shape(t) => t.clone(),
                    Part::Ref(i) => d.refs.get(*i).map_or(String::new(), |s| s.text.clone()),
                    Part::Ok => "✓".into(),
                    Part::No => "✗".into(),
                })
                .collect();
            out += &format!("  {text}\n");
        }
        if let Some(f) = &d.fix {
            out += &format!("\n      {f}\n");
        }
    }
    out
}

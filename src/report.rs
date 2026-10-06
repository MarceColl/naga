//! The text `naga run` prints; `spec/report.js` prints the same from the reference engine.
use crate::check::Checker;
use crate::machine::{Session, Step};
use crate::syntax::parse;

pub fn report(src: &str) -> String {
    let mut out = vec![];
    let p = parse(src);
    out.extend(p.errors.iter().map(|e| format!("parse {}:{} {}", e.line, e.col, e.msg)));
    out.extend(p.warnings.iter().map(|w| format!("warn {} {}", w.line, w.msg)));
    out.extend(p.holes.iter().map(|h| format!("hole {} {}: {}", h.line, h.word, h.note)));
    let entry = if p.words.contains_key("main") { Some("main".into()) } else { p.words.keys().next().cloned() };
    if let Some(entry) = &entry {
        let mut s = Session::new(&p, entry);
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
    let r = Checker::default().check(&p, entry.as_ref());
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
    out.push(String::new());
    out.join("\n")
}

//! Handing a hole to Claude: what it's told, and how its fill is judged. A fill is kept when
//! it parses, adds no checker error and breaks no run that worked before.
use crate::editor::analysis::{Analysis, Step};
use crate::editor::claude::Fill;
use crate::runs::Verdict;
use std::rc::Rc;

pub const TRIES: usize = 3;

#[derive(Clone)]
pub struct Agent {
    /// Tells this agent's replies from a stopped one's.
    pub id: u64,
    pub hole: usize,
    /// `usable`, or `rework`.
    pub label: String,
    pub tries: usize,
    /// The fill being typed into the hole, and how much of it shows.
    pub fill: Option<Fill>,
    pub shown: usize,
    /// What fills are judged against: the program before the agent started.
    pub base: Rc<Analysis>,
    /// For a rework: the code the hole replaced, put back if nothing better is found.
    pub original: Option<String>,
    /// The last fill and why it was rejected, for the next try.
    pub rejected: Option<(Fill, String)>,
}

impl Fill {
    /// The text typed into the hole: the code, then any helpers below it.
    pub fn text(&self) -> String {
        match &self.helpers {
            Some(h) => format!("{}\n\n{h}", self.code),
            None => self.code.clone(),
        }
    }
}

/// `checker clean · runs 7/7`, or what a candidate broke.
pub fn judge(base: &Analysis, cand: &Analysis) -> Result<(), String> {
    if let Some(e) = cand.program.errors.first() {
        return Err(format!("doesn’t parse: {}", e.msg));
    }
    if let Some(e) = cand.errors.iter().find(|e| !base.errors.iter().any(|b| b.msg == e.msg)) {
        return Err(format!("checker: {}", e.msg));
    }
    let broken = |v: Verdict| matches!(v, Verdict::Fail | Verdict::Error);
    for (i, (r, b)) in cand.runs.iter().zip(&base.runs).enumerate() {
        if broken(r.verdict) && r.verdict != b.verdict {
            let name = cand.program.runs.get(i).map_or("", |r| r.name.as_str());
            return Err(match r.verdict {
                Verdict::Error => format!("run “{name}” stops: {}", r.detail.clone().unwrap_or_default()),
                _ => format!("run “{name}” answers {} — expected {}", r.said.join(" / "), r.want.join(" / ")),
            });
        }
    }
    Ok(())
}

/// What Claude is told about the hole.
pub fn prompt(an: &Analysis, hole: &Step, a: &Agent) -> String {
    let numbered: Vec<String> = an.lines.iter().enumerate().map(|(i, l)| format!("{:>4}  {}", i + 1, l.iter().collect::<String>())).collect();
    let note = hole.hole().unwrap_or("").trim();
    let mut p = format!("The program, with line numbers:\n\n{}\n\n", numbered.join("\n"));
    p += &format!("Fill the hole in `{}` on line {}: `? {note}`.\n", hole.word, hole.range.pos.line);
    let arrives = an.arrives(hole);
    if !arrives.is_empty() {
        p += &format!("The record that arrives there: {}\n", arrives.join("  |  "));
    }
    if let Some(o) = &a.original {
        p += &format!("\nThis is a rework: the hole replaced `{o}`. Write the same behaviour more simply, in the program's style; if nothing is simpler, return it unchanged.\n");
    }
    let runs: Vec<String> = an
        .program
        .runs
        .iter()
        .zip(&an.runs)
        .map(|(r, o)| format!("- “{}”: {}{}", r.name, o.verdict.name(), if o.verdict == Verdict::Pass { String::new() } else { format!(" (answered {}; expected {})", o.said.join(" / "), o.want.join(" / ")) }))
        .collect();
    if !runs.is_empty() {
        p += &format!("\nThe runs (requests the program must answer), as things stand:\n{}\nYour fill must not break a run that passes; runs waiting on this hole should pass after it.\n", runs.join("\n"));
    }
    if let Some((f, why)) = &a.rejected {
        p += &format!("\nYour previous fill was rejected.\nIt was:\n{}\nWhy: {why}\nTry again.\n", f.text());
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fill_is_judged_by_the_checker_and_the_runs() {
        let src = "route GET /x = w\nw = ? answer\nrun \"x\"\n  GET /x → 200 1\n";
        let base = Analysis::new(src);
        assert_eq!(judge(&base, &Analysis::new(&src.replace("? answer", "+{status: 200, json: 1}"))), Ok(()));
        assert!(judge(&base, &Analysis::new(&src.replace("? answer", "+{status: 200, json: 2}"))).unwrap_err().starts_with("run “x” answers 200 2"));
        assert!(judge(&base, &Analysis::new(&src.replace("? answer", "+{json: 1}"))).unwrap_err().starts_with("checker: "));
        assert!(judge(&base, &Analysis::new(&src.replace("? answer", "+{"))).unwrap_err().starts_with("doesn’t parse"));
    }
}

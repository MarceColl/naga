//! Sections: `*` and `**` headings at column 0, folded like org mode. A heading is open,
//! folded (nothing under it shows), or showing only its children's headings.
use crate::editor::analysis::Analysis;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fold {
    #[default]
    Open,
    Folded,
    /// Its subheadings show, its own lines don't.
    Children,
}

/// Fold states, by the heading's text.
pub type Folds = HashMap<String, Fold>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// 1-based.
    pub line: usize,
    pub level: usize,
    pub text: String,
}

pub fn headings(an: &Analysis) -> Vec<Heading> {
    an.lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.first() == Some(&'*'))
        .map(|(i, l)| Heading { line: i + 1, level: l.iter().take_while(|c| **c == '*').count(), text: l.iter().collect() })
        .collect()
}

/// The last line of the section a heading starts: before the next heading at its level or above.
pub fn end(an: &Analysis, hs: &[Heading], i: usize) -> usize {
    hs[i + 1..].iter().find(|h| h.level <= hs[i].level).map_or(an.lines.len(), |h| h.line - 1)
}

/// Which lines the folds hide, by line index (0-based).
pub fn hidden(an: &Analysis, folds: &Folds) -> Vec<bool> {
    let hs = headings(an);
    let mut out = vec![false; an.lines.len()];
    let mut stack: Vec<(usize, Fold)> = vec![];
    let mut next = hs.iter().peekable();
    for (i, slot) in out.iter_mut().enumerate() {
        let heading = next.next_if(|h| h.line == i + 1);
        // a heading shows unless an ancestor is folded, or one above its parent shows only
        // children; a line shows only if every heading above it is open
        if let Some(h) = heading {
            while stack.last().is_some_and(|(l, _)| *l >= h.level) {
                stack.pop();
            }
            let parent = stack.len().checked_sub(1);
            *slot = stack.iter().enumerate().any(|(j, (_, f))| *f == Fold::Folded || *f == Fold::Children && Some(j) != parent);
            stack.push((h.level, folds.get(&h.text).copied().unwrap_or_default()));
        } else {
            *slot = stack.iter().any(|(_, f)| *f != Fold::Open);
        }
    }
    out
}

/// What a closed section holds: `… 9 words, 4 runs`.
pub fn summary(an: &Analysis, hs: &[Heading], i: usize) -> String {
    let (a, b) = (hs[i].line, end(an, hs, i));
    let inside = |l: usize| l > a && l <= b;
    let mut words: Vec<&str> = an.defs.iter().filter(|d| inside(d.line)).map(|d| &*d.name).collect();
    words.dedup();
    let tables = an.program.tables.values().filter(|t| inside(t.line)).count();
    let runs = an.program.runs.iter().filter(|r| inside(r.line)).count();
    let routes = an.program.routes.iter().filter(|r| inside(r.line)).count();
    let lines = b - a;
    let parts: Vec<String> = [(routes, "route"), (words.len(), "word"), (tables, "table"), (runs, "run")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}{}", if n == 1 { "" } else { "s" }))
        .collect();
    match parts.is_empty() {
        true => format!("… {lines} lines"),
        false => format!("… {}", parts.join(", ")),
    }
}

/// Tab on a heading: open → folded → children (when it has any) → open.
pub fn cycle(an: &Analysis, folds: &Folds, line: usize) -> Option<Folds> {
    let hs = headings(an);
    let i = hs.iter().position(|h| h.line == line)?;
    let has_children = hs[i + 1..].iter().take_while(|h| h.level > hs[i].level).next().is_some();
    let next = match folds.get(&hs[i].text).copied().unwrap_or_default() {
        Fold::Open => Fold::Folded,
        Fold::Folded if has_children => Fold::Children,
        _ => Fold::Open,
    };
    let mut f = folds.clone();
    f.insert(hs[i].text.clone(), next);
    Some(f)
}

/// Shift-Tab: everything shown → top headings only → every heading → everything shown.
pub fn cycle_all(an: &Analysis, folds: &Folds) -> Folds {
    let hs = headings(an);
    let state = |h: &Heading| folds.get(&h.text).copied().unwrap_or_default();
    let overview = hs.iter().all(|h| state(h) == if h.level == 1 { Fold::Folded } else { Fold::Open });
    let all_open = hs.iter().all(|h| state(h) == Fold::Open);
    hs.iter()
        .map(|h| {
            let f = match (all_open, overview) {
                (true, _) if h.level == 1 => Fold::Folded,
                (true, _) => Fold::Open,
                (_, true) => Fold::Children,
                _ => Fold::Open,
            };
            (h.text.clone(), f)
        })
        .collect()
}

pub fn all(an: &Analysis, fold: Fold) -> Folds {
    headings(an).into_iter().map(|h| (h.text, fold)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "top = nop\n* A\na = nop\n** A1\nb = nop\n* B\nc = nop\nrun \"x\"\n  GET /x → 200\n";

    fn shown(an: &Analysis, f: &Folds) -> String {
        hidden(an, f).iter().map(|h| if *h { '.' } else { '#' }).collect()
    }

    #[test]
    fn folds_hide_sections_and_cycle_like_org() {
        // lines: top, * A, a, ** A1, b, * B, c, run, GET, (empty)
        let an = Analysis::new(SRC);
        let open = Folds::new();
        assert_eq!(shown(&an, &open), "##########");
        let f = cycle(&an, &open, 2).unwrap();
        assert_eq!(shown(&an, &f), "##...#####", "A folded: everything under it goes");
        let f = cycle(&an, &f, 2).unwrap();
        assert_eq!(shown(&an, &f), "##.#.#####", "A children: A1's heading shows, nothing else");
        let f = cycle(&an, &f, 2).unwrap();
        assert_eq!(shown(&an, &f), "##########");
        let f = cycle_all(&an, &open);
        assert_eq!(shown(&an, &f), "##...#....", "overview: the top headings");
        let f = cycle_all(&an, &f);
        assert_eq!(shown(&an, &f), "##.#.#....", "contents: every heading");
        assert_eq!(shown(&an, &cycle_all(&an, &f)), "##########");
        assert_eq!(summary(&an, &headings(&an), 2), "… 1 word, 1 run");
    }
}

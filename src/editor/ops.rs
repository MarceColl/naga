//! Keyword operations over selected steps. Each takes the analysis and returns the program's
//! new text, or a message saying why it can't.
use crate::editor::analysis::{Analysis, Range, Step};
use crate::ast::Pos;
use crate::syntax::strip_comment;

pub type R<T = String> = Result<T, String>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(msg.into())
}

/// lowercase parts joined by single dashes: `count-problems`.
pub fn is_name(s: &str) -> bool {
    !s.is_empty() && s.starts_with(|c: char| c.is_ascii_lowercase()) && s.split('-').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

/// A line that starts a definition: `name =`, `name {…} =`, `name when …`.
fn starts_def(line: &str) -> bool {
    let head: String = line.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    let rest = line[head.len()..].trim_start();
    head.starts_with(|c: char| c.is_ascii_lowercase()) && (rest.starts_with('{') || rest.starts_with('=') || rest.starts_with("when ") || rest == "when")
}

/// The program's lines, each as characters, so columns index them directly.
fn lines(an: &Analysis) -> Vec<Vec<char>> {
    an.lines.clone()
}

fn join(lines: &[Vec<char>]) -> String {
    lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn offset(lines: &[Vec<char>], p: Pos) -> usize {
    lines[..p.line - 1].iter().map(|l| l.len() + 1).sum::<usize>() + p.col
}

pub fn slice(an: &Analysis, r: Range) -> String {
    let text: Vec<char> = join(&an.lines).chars().collect();
    text[offset(&an.lines, r.pos)..offset(&an.lines, r.end)].iter().collect()
}

fn replace_in(lines: &[Vec<char>], r: Range, repl: &str) -> String {
    let text: Vec<char> = join(lines).chars().collect();
    let (a, b) = (offset(lines, r.pos), offset(lines, r.end));
    text[..a].iter().collect::<String>() + repl + &text[b..].iter().collect::<String>()
}

fn split(text: &str) -> Vec<Vec<char>> {
    text.split('\n').map(|l| l.chars().collect()).collect()
}

/// Several lines of code as one: comments and blank lines dropped.
pub fn one_line(s: &str) -> String {
    s.split('\n').map(|x| strip_comment(x).trim()).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Continuation lines of a multi-line fill: new definitions stay at column 0, the rest is indented.
fn shape_lines<'a>(more: impl Iterator<Item = &'a str>) -> Vec<String> {
    more.map(|x| if starts_def(x.trim()) { x.trim().to_string() } else { format!("  {}", x.trim()) }).collect()
}

/// The selected sibling steps, first to last.
pub fn sel_range(an: &Analysis, ids: &[usize]) -> R<Range> {
    let mut ss: Vec<&Step> = ids.iter().filter_map(|&i| an.step(i)).collect();
    ss.sort_by_key(|s| s.idx);
    match (ss.first(), ss.last()) {
        (Some(a), Some(b)) => Ok(Range { pos: a.range.pos, end: b.range.end }),
        _ => fail("select steps first"),
    }
}

/// The last line of the word's last clause.
fn word_end_line(an: &Analysis, word: &str) -> usize {
    an.program.words.get(word).map_or(0, |w| w.clauses.iter().map(|c| c.chunk_line + c.chunk_text.split('\n').count() - 1).max().unwrap_or(0))
}

pub fn extract(an: &Analysis, ids: &[usize], name: &str) -> R {
    if name.is_empty() {
        return fail("extract needs a name: EXTRACT name");
    }
    if !is_name(name) {
        return fail(format!("{name} isn’t a word name (lowercase, dashes between parts)"));
    }
    if an.program.words.contains_key(name) {
        return fail(format!("{name} already exists"));
    }
    let r = sel_range(an, ids)?;
    let body = one_line(&slice(an, r));
    let first = an.step(ids[0]).ok_or("select steps first")?;
    let at = word_end_line(an, &first.word) - (r.end.line - r.pos.line);
    let mut out: Vec<String> = replace_in(&lines(an), r, name).split('\n').map(String::from).collect();
    out.splice(at..at, [String::new(), format!("{name} = {body}")]);
    Ok(out.join("\n"))
}

pub fn inline(an: &Analysis, ids: &[usize]) -> R {
    let [id] = ids else { return fail("inline works on one call") };
    let s = an.step(*id).ok_or("inline works on one call")?;
    let Some(name) = s.call() else { return fail("inline works on a call to a word") };
    let Some(w) = an.program.words.get(name) else { return fail(format!("{name} isn’t defined")) };
    let [c] = w.clauses.as_slice() else { return fail(format!("{name} has {} clauses — only a single-clause word can be inlined", w.clauses.len())) };
    if c.pattern.as_ref().is_some_and(|p| !p.is_empty()) || c.guard.is_some() {
        return fail(format!("{name} matches on its input — inlining it would drop that check"));
    }
    let code: String = c.chunk_text.split('\n').map(strip_comment).collect::<Vec<_>>().join("\n");
    let after_head = code.find(&c.head_src).map_or(0, |i| i + c.head_src.len());
    let eq = code[after_head..].find('=').map_or(code.len(), |i| after_head + i);
    let body = one_line(&code[(eq + 1).min(code.len())..]);
    if body.contains('?') {
        return fail(format!("{name} still has a hole"));
    }
    Ok(replace_in(&lines(an), s.range, if body.is_empty() { "nop" } else { &body }))
}

/// Replaces the steps with a hole whose note is `note`, or the code it replaces.
pub fn punch(an: &Analysis, ids: &[usize], note: &str) -> R<(String, String, String)> {
    let r = sel_range(an, ids)?;
    let original = one_line(&slice(an, r));
    let mut ls: Vec<String> = an.lines.iter().map(|l| l.iter().collect()).collect();
    let before: String = an.lines[r.pos.line - 1][..r.pos.col].iter().collect();
    let tail: String = an.lines[r.end.line - 1][r.end.col..].iter().collect();
    let after = strip_comment(&tail).trim().to_string();
    let n = if note.trim().is_empty() { original.clone() } else { note.to_string() }.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut repl = vec![format!("{before}? {n}")];
    if !after.is_empty() {
        repl.push(format!("  {after}"));
    }
    ls.splice(r.pos.line - 1..r.end.line, repl);
    Ok((ls.join("\n"), original, n))
}

pub fn drop(an: &Analysis, ids: &[usize]) -> R {
    let r = sel_range(an, ids)?;
    let mut ls: Vec<String> = an.lines.iter().map(|l| l.iter().collect()).collect();
    let start: String = an.lines[r.pos.line - 1][..r.pos.col].iter().collect();
    let end: String = an.lines[r.end.line - 1][r.end.col..].iter().collect();
    let mut joined = format!("{} {}", start.trim_end(), end.trim_start()).trim_end().to_string();
    if joined.ends_with('=') {
        joined += " nop";
    }
    let repl = if joined.trim().is_empty() { vec![] } else { vec![joined] };
    ls.splice(r.pos.line - 1..r.end.line, repl);
    Ok(ls.join("\n"))
}

pub fn swap(an: &Analysis, ids: &[usize]) -> R {
    let msg = "swap needs exactly two steps next to each other";
    let [a, b] = ids else { return fail(msg) };
    let (mut a, mut b) = (an.step(*a).ok_or(msg)?, an.step(*b).ok_or(msg)?);
    if a.idx > b.idx {
        std::mem::swap(&mut a, &mut b);
    }
    if a.parent != b.parent || b.idx != a.idx + 1 {
        return fail(msg);
    }
    let (ta, tb) = (slice(an, a.range), slice(an, b.range));
    // b comes after a, so replacing b leaves a's range where it was
    let t1 = replace_in(&lines(an), b.range, &ta);
    Ok(replace_in(&split(&t1), a.range, &tb))
}

pub fn rename(an: &Analysis, word: &str, to: &str) -> R {
    if to.is_empty() {
        return fail("rename needs the new name: RENAME name");
    }
    if !is_name(to) {
        return fail(format!("{to} isn’t a word name (lowercase, dashes between parts)"));
    }
    if an.program.words.contains_key(to) {
        return fail(format!("{to} already exists"));
    }
    let n = word.chars().count();
    let mut edits: Vec<Range> = an.defs.iter().filter(|d| &*d.name == word).map(|d| Range { pos: Pos { line: d.line, col: 0 }, end: Pos { line: d.line, col: n } }).collect();
    edits.extend(an.steps.iter().filter(|s| s.call().is_some_and(|c| &**c == word)).map(|s| s.range));
    edits.sort_by_key(|e| std::cmp::Reverse((e.pos.line, e.pos.col)));
    Ok(edits.iter().fold(join(&an.lines), |text, e| replace_in(&split(&text), *e, to)))
}


/// Fills a hole. A fill that restates the whole word (`usable {…} = …`) replaces the line;
/// otherwise it goes where the hole was. `helpers` are new definitions put after it.
pub fn fill(an: &Analysis, hole: &Step, code: &str, helpers: Option<&str>) -> String {
    let mut ls: Vec<String> = an.lines.iter().map(|l| l.iter().collect()).collect();
    let i = hole.range.pos.line - 1;
    let prefix: String = an.lines[i][..hole.range.pos.col].iter().collect();
    let code_lines: Vec<&str> = code.split('\n').collect();
    let whole_word = prefix.trim().strip_prefix(&*hole.word).is_some_and(|r| r.trim() == "=");
    let mut repl: Vec<String> = if whole_word && code_lines[0].trim().starts_with(&*hole.word) {
        code_lines.iter().map(|x| x.trim_end().to_string()).collect()
    } else {
        std::iter::once(format!("{prefix}{}", code_lines[0].trim())).chain(shape_lines(code_lines[1..].iter().copied())).collect()
    };
    if let Some(h) = helpers {
        repl.push(String::new());
        repl.extend(h.split('\n').map(String::from));
    }
    ls.splice(i..=i, repl);
    ls.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::analysis::Analysis;

    const SRC: &str = "go = a b c\n\na = +{x: 1}\nb = nop\nc = nop\nhole = ? fill me\n";

    fn ids(an: &Analysis, texts: &[&str]) -> Vec<usize> {
        texts.iter().map(|t| an.steps.iter().find(|s| &slice(an, s.range) == t).unwrap().id).collect()
    }

    #[test]
    fn extract_inline_and_back() {
        let an = Analysis::new(SRC);
        let t = extract(&an, &ids(&an, &["a", "b"]), "ab").unwrap();
        assert_eq!(t, "go = ab c\n\nab = a b\n\na = +{x: 1}\nb = nop\nc = nop\nhole = ? fill me\n");
        let an = Analysis::new(&t);
        let t = inline(&an, &ids(&an, &["ab"])).unwrap();
        assert!(t.starts_with("go = a b c\n"), "{t}");
        assert_eq!(extract(&Analysis::new(SRC), &ids(&Analysis::new(SRC), &["a"]), "b").unwrap_err(), "b already exists");
    }

    #[test]
    fn punch_drop_swap_rename() {
        let an = Analysis::new(SRC);
        assert_eq!(punch(&an, &ids(&an, &["b"]), "").unwrap().0.lines().next(), Some("go = a ? b"));
        assert_eq!(punch(&an, &ids(&an, &["b"]), "").unwrap().0.lines().nth(1), Some("  c"));
        assert_eq!(drop(&an, &ids(&an, &["a", "b", "c"])).unwrap().lines().next(), Some("go = nop"));
        assert_eq!(swap(&an, &ids(&an, &["a", "b"])).unwrap().lines().next(), Some("go = b a c"));
        let t = rename(&an, "a", "first").unwrap();
        assert!(t.starts_with("go = first b c\n\nfirst = +{x: 1}"), "{t}");
    }

    #[test]
    fn filling_a_hole() {
        let an = Analysis::new(SRC);
        let h = an.holes().next().unwrap().clone();
        assert!(fill(&an, &h, "+{y: 2}\nsay \"ok\"", None).ends_with("hole = +{y: 2}\n  say \"ok\"\n"));
        let t = fill(&an, &h, "hole {x} = nop\nhole = nop", Some("more = nop"));
        assert!(t.ends_with("hole {x} = nop\nhole = nop\n\nmore = nop\n"), "{t}");
    }
}

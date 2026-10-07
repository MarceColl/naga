//! The program as lines of characters, and what vim's motions and edits do to it. Positions
//! are `Pos`: lines from 1, columns from 0, as everywhere else in naga.
use crate::ast::Pos;

pub type Lines = Vec<Vec<char>>;

#[cfg(test)]
pub fn lines(text: &str) -> Lines {
    text.split('\n').map(|l| l.chars().collect()).collect()
}

pub fn text(ls: &Lines) -> String {
    ls.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordNext,
    WordBack,
    WordEnd,
    Start,
    FirstNonBlank,
    End,
    Top,
    Bottom,
}

impl Motion {
    /// Whole lines when an operator takes it: `dj` deletes two lines.
    pub fn linewise(self) -> bool {
        matches!(self, Motion::Up | Motion::Down | Motion::Top | Motion::Bottom)
    }
    /// Includes the character it lands on: `de` deletes through the word's last letter.
    pub fn inclusive(self) -> bool {
        matches!(self, Motion::WordEnd | Motion::End)
    }
}

/// Words are naga names (letters, digits, `_`, `-`); other punctuation runs are words too.
fn class(c: char) -> u8 {
    match c {
        c if c.is_whitespace() => 0,
        c if c.is_alphanumeric() || c == '_' || c == '-' => 1,
        _ => 2,
    }
}

fn len(ls: &Lines, line: usize) -> usize {
    ls.get(line - 1).map_or(0, Vec::len)
}

/// The character at `p`, with the end of a line read as a newline.
fn at(ls: &Lines, p: Pos) -> char {
    ls[p.line - 1].get(p.col).copied().unwrap_or('\n')
}

fn next(ls: &Lines, p: Pos) -> Option<Pos> {
    match p.col < len(ls, p.line) {
        true => Some(Pos { col: p.col + 1, ..p }),
        false => (p.line < ls.len()).then_some(Pos { line: p.line + 1, col: 0 }),
    }
}

fn prev(ls: &Lines, p: Pos) -> Option<Pos> {
    match p.col {
        0 => (p.line > 1).then(|| Pos { line: p.line - 1, col: len(ls, p.line - 1) }),
        c => Some(Pos { col: c - 1, ..p }),
    }
}

/// A position the cursor can rest on in normal mode: not past the last character.
pub fn clamp(ls: &Lines, p: Pos, insert: bool) -> Pos {
    let line = p.line.clamp(1, ls.len().max(1));
    let max = len(ls, line).saturating_sub(usize::from(!insert));
    Pos { line, col: p.col.min(max) }
}

/// Where `m` takes the cursor from `p`; `goal` is the column up and down aim for.
pub fn go(ls: &Lines, p: Pos, m: Motion, goal: usize) -> Pos {
    let cls = |q: Pos| class(at(ls, q));
    match m {
        Motion::Left => Pos { col: p.col.saturating_sub(1), ..p },
        Motion::Right => Pos { col: (p.col + 1).min(len(ls, p.line)), ..p },
        Motion::Up => Pos { line: p.line.saturating_sub(1).max(1), col: goal },
        Motion::Down => Pos { line: (p.line + 1).min(ls.len()), col: goal },
        Motion::Start => Pos { col: 0, ..p },
        Motion::FirstNonBlank => Pos { col: ls[p.line - 1].iter().position(|c| !c.is_whitespace()).unwrap_or(0), ..p },
        Motion::End => Pos { col: len(ls, p.line).saturating_sub(1), ..p },
        Motion::Top => Pos { line: 1, col: 0 },
        Motion::Bottom => Pos { line: ls.len(), col: 0 },
        Motion::WordNext => {
            // past this word, then past the blanks after it
            let mut q = p;
            let c0 = cls(q);
            while c0 != 0 && cls(q) == c0 {
                let Some(n) = next(ls, q) else { return q };
                q = n;
            }
            while cls(q) == 0 {
                let Some(n) = next(ls, q) else { return q };
                q = n;
            }
            q
        }
        Motion::WordBack => {
            let mut q = prev(ls, p).unwrap_or(p);
            while cls(q) == 0 {
                let Some(n) = prev(ls, q) else { return q };
                q = n;
            }
            let c0 = cls(q);
            while let Some(n) = prev(ls, q).filter(|&n| cls(n) == c0) {
                q = n;
            }
            q
        }
        Motion::WordEnd => {
            let mut q = next(ls, p).unwrap_or(p);
            while cls(q) == 0 {
                let Some(n) = next(ls, q) else { return q };
                q = n;
            }
            let c0 = cls(q);
            while let Some(n) = next(ls, q).filter(|&n| cls(n) == c0) {
                q = n;
            }
            q
        }
    }
}

pub fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if (a.line, a.col) <= (b.line, b.col) { (a, b) } else { (b, a) }
}

/// The text from `a` up to `b`.
pub fn slice(ls: &Lines, a: Pos, b: Pos) -> String {
    let mut out = String::new();
    let mut q = a;
    while (q.line, q.col) < (b.line, b.col) {
        out.push(at(ls, q));
        let Some(n) = next(ls, q) else { break };
        q = n;
    }
    out
}

/// Removes the text from `a` up to `b`.
pub fn delete(ls: &Lines, a: Pos, b: Pos) -> Lines {
    let mut out = ls.clone();
    let tail: Vec<char> = ls[b.line - 1][b.col.min(len(ls, b.line))..].to_vec();
    out[a.line - 1].truncate(a.col);
    out[a.line - 1].extend(tail);
    out.drain(a.line..b.line);
    out
}

/// Puts `s` at `p`; returns the lines and where the inserted text ends.
pub fn insert(ls: &Lines, p: Pos, s: &str) -> (Lines, Pos) {
    let mut out = ls.clone();
    let tail = out[p.line - 1].split_off(p.col);
    let mut parts = s.split('\n');
    out[p.line - 1].extend(parts.next().unwrap_or("").chars());
    let mut at = p.line;
    for part in parts {
        out.insert(at, part.chars().collect());
        at += 1;
    }
    let end = Pos { line: at, col: out[at - 1].len() };
    out[at - 1].extend(tail);
    (out, end)
}

/// Whole lines `a..=b` as text, each ending in a newline.
pub fn line_text(ls: &Lines, a: usize, b: usize) -> String {
    ls[a - 1..b].iter().map(|l| l.iter().collect::<String>() + "\n").collect()
}

pub fn delete_lines(ls: &Lines, a: usize, b: usize) -> Lines {
    let mut out = ls.clone();
    out.drain(a - 1..b);
    if out.is_empty() {
        out.push(vec![]);
    }
    out
}

/// Puts whole lines (each ending in a newline) before line `at`.
pub fn insert_lines(ls: &Lines, at: usize, s: &str) -> Lines {
    let mut out = ls.clone();
    let new: Vec<Vec<char>> = s.strip_suffix('\n').unwrap_or(s).split('\n').map(|l| l.chars().collect()).collect();
    out.splice(at - 1..at - 1, new);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn word_motions_treat_naga_names_as_words() {
        let ls = lines("get-account = accounts.get found\n  +{status: 404}");
        assert_eq!(go(&ls, p(1, 0), Motion::WordNext, 0), p(1, 12));
        assert_eq!(go(&ls, p(1, 12), Motion::WordNext, 0), p(1, 14));
        assert_eq!(go(&ls, p(1, 14), Motion::WordNext, 0), p(1, 22));
        assert_eq!(go(&ls, p(1, 27), Motion::WordNext, 0), p(2, 2));
        assert_eq!(go(&ls, p(2, 2), Motion::WordBack, 0), p(1, 27));
        assert_eq!(go(&ls, p(1, 14), Motion::WordEnd, 0), p(1, 21));
        assert_eq!(go(&ls, p(1, 5), Motion::FirstNonBlank, 0), p(1, 0));
        assert_eq!(go(&ls, p(2, 9), Motion::FirstNonBlank, 0), p(2, 2));
    }

    #[test]
    fn edits_across_lines() {
        let ls = lines("ab\ncd\nef");
        assert_eq!(text(&delete(&ls, p(1, 1), p(3, 1))), "af");
        assert_eq!(slice(&ls, p(1, 1), p(3, 1)), "b\ncd\ne");
        let (ins, end) = insert(&ls, p(2, 1), "X\nYY");
        assert_eq!((text(&ins), end), ("ab\ncX\nYYd\nef".to_string(), p(3, 2)));
        assert_eq!(text(&delete_lines(&ls, 2, 3)), "ab");
        assert_eq!(text(&insert_lines(&ls, 2, "zz\n")), "ab\nzz\ncd\nef");
        assert_eq!(line_text(&ls, 1, 2), "ab\ncd\n");
    }
}

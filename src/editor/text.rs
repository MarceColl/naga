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

/// The `=` that starts a definition's body (or a route's word): the first one outside
/// brackets and strings that isn't part of `==`, `<=`, `>=`, `!=` or `=>`.
fn head_eq(l: &[char]) -> Option<usize> {
    let (mut depth, mut in_str) = (0i32, false);
    for (i, &c) in l.iter().enumerate() {
        match c {
            '"' => in_str = !in_str,
            _ if in_str => {}
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            '-' if l.get(i + 1) == Some(&'-') => return None,
            '=' if depth == 0 => {
                let before = i.checked_sub(1).map(|j| l[j]);
                let after = l.get(i + 1).copied();
                if !matches!(before, Some('=' | '<' | '>' | '!')) && !matches!(after, Some('=' | '>')) {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn head_eq_col(l: &[char]) -> Option<usize> {
    head_eq(l)
}

/// What a line starts, for alignment: a definition (0) or a route (1).
fn kind(l: &[char]) -> Option<u8> {
    let first: String = l.iter().take_while(|c| !c.is_whitespace()).collect();
    match first.to_lowercase().as_str() {
        "route" => head_eq(l).map(|_| 1),
        "table" | "seed" | "run" | "" => None,
        w if w.starts_with(|c: char| c.is_alphabetic()) => head_eq(l).map(|_| 0),
        _ => None,
    }
}

/// Lines continuing the definition above them.
fn continues(l: &[char]) -> bool {
    l.first().is_some_and(|c| c.is_whitespace()) && l.iter().any(|c| !c.is_whitespace())
}

/// Lines up the `=` of the definitions (or routes) around `line` that have no blank line
/// between them; lines continuing one move with it. Returns the lines and how far `line`'s
/// text after its `=` moved.
pub fn align(ls: &Lines, line: usize) -> (Lines, isize) {
    let mut at = line;
    while at > 1 && continues(&ls[at - 1]) {
        at -= 1;
    }
    let Some(k) = ls.get(at - 1).and_then(|l| kind(l)) else { return (ls.clone(), 0) };
    let part = |i: usize| kind(&ls[i - 1]) == Some(k) || continues(&ls[i - 1]);
    let (mut a, mut b) = (at, at);
    while a > 1 && part(a - 1) {
        a -= 1;
    }
    while b < ls.len() && part(b + 1) {
        b += 1;
    }
    let heads: Vec<usize> = (a..=b).filter(|&i| kind(&ls[i - 1]) == Some(k)).collect();
    if heads.len() < 2 {
        return (ls.clone(), 0);
    }
    let end_of_head = |i: usize| {
        let l = &ls[i - 1];
        let eq = head_eq(l).unwrap();
        (eq, l[..eq].iter().rposition(|c| !c.is_whitespace()).map_or(0, |p| p + 1))
    };
    let target = heads.iter().map(|&i| end_of_head(i).1).max().unwrap_or(0) + 1;
    let mut out = ls.clone();
    let (mut shift, mut moved) = (0isize, 0isize);
    for i in a..=b {
        let l = &ls[i - 1];
        if kind(l) == Some(k) {
            let (eq, head) = end_of_head(i);
            let mut new: Vec<char> = l[..head].to_vec();
            new.resize(target, ' ');
            new.extend(&l[eq..]);
            shift = target as isize - eq as isize;
            out[i - 1] = new;
            if i == line {
                moved = shift;
            }
        } else if shift != 0 {
            // a continuation moves with its definition, keeping at least two spaces of indent
            let indent = l.iter().take_while(|c| **c == ' ').count() as isize;
            let to = (indent + shift).max(2) as usize;
            out[i - 1] = std::iter::repeat_n(' ', to).chain(l[indent as usize..].iter().copied()).collect();
            if i == line {
                moved = to as isize - indent;
            }
        }
    }
    (out, moved)
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
    fn definitions_together_line_up_their_equals() {
        let ls = lines("found {-missing} = +{status: 404}\nfound {note} = +{status: 200}\n  reply\n\nother = nop");
        let (out, moved) = align(&ls, 2);
        assert_eq!(text(&out), "found {-missing} = +{status: 404}\nfound {note}     = +{status: 200}\n      reply\n\nother = nop");
        assert_eq!(moved, 4);
        let ls = lines("go when n == 0 = say \"x\"\ngo = nop");
        assert_eq!(text(&align(&ls, 1).0), "go when n == 0 = say \"x\"\ngo             = nop");
        let ls = lines("route GET /a = w\nroute POST /accounts = v");
        assert_eq!(text(&align(&ls, 1).0), "route GET /a         = w\nroute POST /accounts = v");
        let ls = lines("a = nop\n\nbb = nop");
        assert_eq!(text(&align(&ls, 1).0), "a = nop\n\nbb = nop", "a blank line keeps them apart");
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

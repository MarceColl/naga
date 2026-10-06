//! The screen as data: what to draw, in which colours, and what each piece of text points at.
//! colorForth-style: one screen of code on black, colour as meaning, one status line at the bottom.
use crate::analysis::{Analysis, Range, Step};
use crate::state::{Mode, Sel, State};
use naga::check::Point;
use naga::value::Name;

pub type Color = [u8; 3];

pub mod palette {
    use super::Color;
    pub const BG: Color = [0x00, 0x00, 0x00];
    pub const FG: Color = [0xEC, 0xEC, 0xEC];
    pub const DIM: Color = [0x8C, 0x8C, 0x8C];
    pub const FAINT: Color = [0x50, 0x50, 0x50];
    pub const DEF: Color = [0xFF, 0xD6, 0x0A];
    pub const CALL: Color = [0x3B, 0xFF, 0x8D];
    pub const TABLE: Color = [0x4F, 0xA8, 0xFF];
    pub const OP: Color = [0x00, 0xE1, 0xFF];
    pub const NUM: Color = [0xFF, 0x9F, 0x1C];
    pub const STR: Color = [0xD5, 0x9C, 0xFF];
    pub const KW: Color = [0xFF, 0x4F, 0xD8];
    pub const HOLE: Color = [0xE1, 0x06, 0x00];
    pub const HOLE_FG: Color = [0xFF, 0xFF, 0xFF];
    pub const ERR: Color = [0xFF, 0x3B, 0x30];
    pub const HOVER: Color = [0x16, 0x16, 0x16];
    pub const RULE: Color = [0x1C, 0x1C, 0x1C];
}
use palette::*;

/// What a piece of text points at, for the mouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Step(usize),
    Def(Name, usize),
    Table(Name),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub fg: Color,
    pub bg: Option<Color>,
    /// 500, 700 or 800.
    pub weight: u16,
    /// Underlined in red: a checker or parse error.
    pub err: bool,
}

impl Style {
    pub const fn plain(fg: Color) -> Style {
        Style { fg, bg: None, weight: 500, err: false }
    }
    const fn bold(fg: Color, weight: u16) -> Style {
        Style { fg, bg: None, weight, err: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// The program line it shows (a folded seed shows its first).
    pub src: usize,
}

impl Line {
    fn push(&mut self, text: &str, style: Style, target: Option<Target>) {
        match self.spans.last_mut() {
            Some(s) if s.style == style && s.target == target => s.text.push_str(text),
            _ => self.spans.push(Span { text: text.into(), style, target }),
        }
    }
    /// What the character at `col` points at.
    pub fn target_at(&self, col: usize) -> Option<&Target> {
        let mut at = 0;
        for s in &self.spans {
            let n = s.text.chars().count();
            if col < at + n {
                return s.target.as_ref();
            }
            at += n;
        }
        None
    }
}

/// The code, then the status bar: shapes above, the message and the counts below.
#[derive(Debug, Default)]
pub struct Frame {
    pub code: Vec<Line>,
    pub shape: Vec<Line>,
    pub msg: Line,
    pub stat: Line,
}

// ---------- tokens ----------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Com,
    Str,
    Op,
    Kw,
    Num,
    Id,
    Pun,
}

const KEYWORDS: [&str; 18] =
    ["when", "say", "show", "raise", "break", "nop", "now", "and", "or", "not", "true", "false", "nil", "wrap", "unwrap", "table", "seed", "route"];
const GLYPHS: [char; 10] = ['‖', '→', '⊂', '⊃', '⫽', '⊔', '⊓', '⊑', '∆', '⍎'];

fn id_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn id_cont(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A class for every character of a line.
fn classes(line: &[char]) -> Vec<Class> {
    let mut out = vec![Class::Pun; line.len()];
    let mut i = 0;
    let mark = |out: &mut Vec<Class>, a: usize, b: usize, c: Class| out[a..b].iter_mut().for_each(|x| *x = c);
    while i < line.len() {
        let c = line[i];
        let next = line.get(i + 1).copied();
        let j = match c {
            '-' if next == Some('-') => {
                mark(&mut out, i, line.len(), Class::Com);
                break;
            }
            '"' => {
                let mut j = i + 1;
                while j < line.len() && line[j] != '"' {
                    j += if line[j] == '\\' { 2 } else { 1 };
                }
                let j = (j + 1).min(line.len());
                mark(&mut out, i, j, Class::Str);
                j
            }
            '+' | '-' | '.' if next == Some('{') => {
                mark(&mut out, i, i + 2, Class::Op);
                i + 2
            }
            '-' | '|' if next == Some(if c == '-' { '>' } else { '|' }) => {
                mark(&mut out, i, i + 2, Class::Op);
                i + 2
            }
            '@' if next.is_some_and(id_start) => {
                let mut j = i + 1;
                while j < line.len() && (id_cont(line[j]) || line[j] == '-') {
                    j += 1;
                }
                mark(&mut out, i, j, Class::Op);
                j
            }
            '^' => {
                let mut j = i;
                while j < line.len() && line[j] == '^' {
                    j += 1;
                }
                mark(&mut out, i, j, Class::Op);
                j
            }
            c if GLYPHS.contains(&c) => {
                mark(&mut out, i, i + 1, Class::Op);
                i + 1
            }
            c if c.is_ascii_digit() => {
                let mut j = i;
                while j < line.len() && (line[j].is_ascii_digit() || (line[j] == '.' && line.get(j + 1).is_some_and(char::is_ascii_digit))) {
                    j += 1;
                }
                mark(&mut out, i, j, Class::Num);
                j
            }
            c if id_start(c) => {
                let mut j = i + 1;
                while j < line.len() && (id_cont(line[j]) || (line[j] == '-' && line.get(j + 1).is_some_and(|&n| id_start(n)))) {
                    j += 1;
                }
                let word: String = line[i..j].iter().collect();
                let class = if KEYWORDS.contains(&word.as_str()) {
                    Class::Kw
                } else if c.is_uppercase() {
                    Class::Num
                } else {
                    Class::Id
                };
                mark(&mut out, i, j, class);
                j
            }
            _ => i + 1,
        };
        i = j;
    }
    out
}

fn class_style(c: Class) -> Style {
    match c {
        Class::Com => Style::plain(FAINT),
        Class::Str => Style::plain(STR),
        Class::Op => Style::bold(OP, 800),
        Class::Kw => Style::bold(KW, 700),
        Class::Num => Style::plain(NUM),
        Class::Id => Style::plain(FG),
        Class::Pun => Style::plain(DIM),
    }
}

/// Uppercase, the way the reference shows code; strings keep their case.
fn shown(c: char, class: Class) -> char {
    if class == Class::Str { c } else { c.to_uppercase().next().unwrap_or(c) }
}

// ---------- the code ----------
struct Marks {
    sel: Option<Range>,
    hover: Option<Range>,
    err_steps: Vec<usize>,
    err_lines: Vec<usize>,
}

fn sel_range(an: &Analysis, ids: &[usize]) -> Option<Range> {
    let mut ss: Vec<&Step> = ids.iter().filter_map(|&i| an.step(i)).collect();
    ss.sort_by_key(|s| s.idx);
    Some(Range { pos: ss.first()?.range.pos, end: ss.last()?.range.end })
}

pub fn view(st: &State) -> Frame {
    let an = &st.an;
    let mut err_steps = vec![];
    let mut err_lines = vec![];
    for e in &an.errors {
        let row = an.cells.get(e.line.wrapping_sub(1));
        match row.and_then(|r| r.get(e.col).copied().filter(|&x| x != 0).or_else(|| r.iter().copied().find(|&x| x != 0))) {
            Some(id) => err_steps.push(id),
            None => err_lines.push(e.line),
        }
    }
    let sel = match &st.sel {
        Some(Sel::Steps(ids)) => sel_range(an, ids),
        _ => None,
    };
    let hover = match &st.hover {
        Some(Target::Step(id)) => an.step(*id).map(|s| s.range),
        _ => None,
    };
    let marks = Marks { sel, hover, err_steps, err_lines };
    let mut code = vec![];
    let mut folding = false;
    for (li, chars) in an.lines.iter().enumerate() {
        let ln = li + 1;
        let text: String = chars.iter().collect();
        if text.starts_with("seed") && text[4..].starts_with(char::is_whitespace) {
            code.push(seed_line(an, li));
            folding = true;
            continue;
        }
        if folding && text.starts_with(char::is_whitespace) && !text.trim().is_empty() {
            continue;
        }
        folding = false;
        code.push(code_line(st, &marks, ln));
    }
    let mut f = Frame { code, ..Frame::default() };
    bar(st, &mut f);
    f
}

/// A seed is data: one folded line, a click away from the table's rows.
fn seed_line(an: &Analysis, li: usize) -> Line {
    let mut j = li + 1;
    while an.lines.get(j).is_some_and(|l| l.first().is_some_and(|c| c.is_whitespace()) && l.iter().any(|c| !c.is_whitespace())) {
        j += 1;
    }
    let chunk: String = an.lines[li..j].iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join(" ");
    let rows = chunk.matches('{').count();
    let mut line = Line { src: li + 1, ..Line::default() };
    let name = chunk.split_whitespace().nth(1).unwrap_or("").to_string();
    line.push("SEED ", Style::bold(KW, 700), None);
    line.push(&name.to_uppercase(), Style::bold(TABLE, 700), Some(Target::Table(name.as_str().into())));
    line.push(" = [", Style::plain(DIM), None);
    line.push(&format!("…{rows} ROWS]"), Style::plain(FAINT), Some(Target::Table(name.as_str().into())));
    line
}

fn code_line(st: &State, m: &Marks, ln: usize) -> Line {
    let an = &st.an;
    let chars = &an.lines[ln - 1];
    let cls = classes(chars);
    let row = &an.cells[ln - 1];
    let def = an.def_at(ln);
    let hole = an.holes().find(|h| h.range.pos.line == ln);
    let table_decl = chars.iter().collect::<String>().strip_prefix("table").filter(|r| r.starts_with(' ')).map(|r| r.split_whitespace().next().unwrap_or("").to_string());
    let mut line = Line { src: ln, ..Line::default() };
    for (i, &ch) in chars.iter().enumerate() {
        let sid = row[i];
        let step = (sid != 0).then(|| an.step(sid)).flatten();
        let mut style = class_style(cls[i]);
        let mut target = step.map(|s| Target::Step(s.id));
        let in_def = def.is_some_and(|d| i < d.name.chars().count());
        if let Some(t) = table_decl.as_ref().filter(|t| (6..6 + t.chars().count()).contains(&i)) {
            style = Style::bold(TABLE, 700);
            target = Some(Target::Table(t.as_str().into()));
        } else if in_def {
            style = Style::bold(DEF, 800);
            target = def.map(|d| Target::Def(d.name.clone(), ln));
        } else if step.is_some_and(|s| s.call().is_some()) && cls[i] == Class::Id {
            style = Style::bold(CALL, 700);
        } else if step.is_some_and(|s| s.table().is_some()) && (cls[i] == Class::Id || ch == '.') {
            style = Style::bold(TABLE, 700);
        }
        let mut c = shown(ch, cls[i]);
        let is_hole = hole.is_some_and(|h| i >= h.range.pos.col);
        if is_hole {
            style = Style { fg: HOLE_FG, bg: Some(HOLE), weight: 700, err: false };
            target = hole.map(|h| Target::Step(h.id));
            c = ch.to_uppercase().next().unwrap_or(ch);
        }
        let selected = m.sel.is_some_and(|r| r.contains(ln, i) && ch != ' ')
            || (in_def && matches!(&st.sel, Some(Sel::Def(n, l)) if Some(n) == def.map(|d| &d.name) && *l == ln));
        if selected {
            style = Style { fg: if is_hole { HOLE } else { BG }, bg: Some(FG), ..style };
        } else if m.hover.is_some_and(|r| r.contains(ln, i)) && !is_hole && st.mode == Mode::Idle {
            style.bg = Some(HOVER);
        }
        style.err = (sid != 0 && m.err_steps.contains(&sid)) || (in_def && m.err_lines.contains(&ln));
        line.push(&c.to_string(), style, target);
    }
    line
}

// ---------- the status bar ----------
const HINT: &str = "click a red hole to write it · shift-click it to hand it to an agent · click steps to select them, then type extract · inline · hole · rework · edit · drop · swap · rename · click a table to see its rows · type debug 1–7 to step through a run · ctrl-z undoes";

fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// Non-lifted shapes, or the most detailed lifted one when that's all there is.
fn best(pts: Option<&Vec<Point>>) -> Vec<String> {
    let Some(pts) = pts.filter(|p| !p.is_empty()) else { return vec![] };
    let real: Vec<&Point> = pts.iter().filter(|p| !p.lifted).collect();
    let mut out: Vec<String> = vec![];
    let use_ = if real.is_empty() { vec![pts.iter().max_by_key(|p| p.text.len()).unwrap()] } else { real };
    for p in use_ {
        if !out.contains(&p.text) {
            out.push(p.text.clone());
        }
    }
    out
}

fn sig_text(an: &Analysis, name: &str) -> Option<Vec<String>> {
    let cases = an.checked.as_ref()?.signatures.get(name).filter(|c| !c.is_empty())?;
    Some(
        cases
            .iter()
            .map(|c| {
                let res = match &c.fail {
                    Some(f) => format!("✗ {f}"),
                    None if c.out.is_empty() => "never returns".into(),
                    None => c.out.join(" | "),
                };
                format!("{} → {res}", c.input)
            })
            .collect(),
    )
}

fn shapes_at(an: &Analysis, s: &Step) -> (Vec<String>, Vec<String>) {
    let Some(c) = &an.checked else { return (vec![], vec![]) };
    let k = (s.range.pos.line, s.range.pos.col);
    (best(c.before.get(&k)), best(c.after.get(&k)))
}

fn hole_arrives(an: &Analysis, hole: &Step) -> Vec<String> {
    let Some(c) = &an.checked else { return vec![] };
    let mut out: Vec<String> = vec![];
    for s in an.steps.iter().filter(|s| s.call() == Some(&hole.word)) {
        for t in best(c.before.get(&(s.range.pos.line, s.range.pos.col))) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    if out.is_empty() {
        out = best(c.before.get(&(hole.range.pos.line, hole.range.pos.col)));
    }
    out
}

fn bar(st: &State, f: &mut Frame) {
    let an = &st.an;
    let (holes, errs) = (an.holes().count(), an.errors.len());
    let dim = Style::plain(DIM);
    f.stat.push(&format!("HOLES {holes}"), if holes > 0 { Style::plain(HOLE) } else { dim }, None);
    f.stat.push(" · ", dim, None);
    f.stat.push(&format!("ERRORS {errs}"), if errs > 0 { Style::plain(ERR) } else { dim }, None);
    f.stat.push(&format!(" · RUNS {}/{}", an.passing(), an.runs.len()), dim, None);
    f.msg.push(&upper(st.msg.as_deref().unwrap_or(HINT)), dim, None);
    let target = st.hover.clone().or_else(|| match &st.sel {
        Some(Sel::Def(n, l)) => Some(Target::Def(n.clone(), *l)),
        Some(Sel::Steps(ids)) => ids.last().map(|&i| Target::Step(i)),
        None => None,
    });
    f.shape = target.map(|t| shape(an, &t)).unwrap_or_default();
}

fn shape(an: &Analysis, t: &Target) -> Vec<Line> {
    let k = Style::plain(DIM);
    let w = Style::bold(CALL, 800);
    let fg = Style::plain(FG);
    let e = Style::plain(ERR);
    let mut first = Line::default();
    let mut lines = vec![];
    match t {
        Target::Table(name) => {
            first.push(&upper(name), Style::bold(TABLE, 800), None);
            first.push(" · CLICK TO SEE ITS ROWS", k, None);
        }
        Target::Def(name, _) => {
            first.push(&upper(name), w, None);
            first.push(" : ", k, None);
            first.push(&upper(&sig_text(an, name).map_or_else(|| "not checked".into(), |s| s.join("  ;  "))), fg, None);
            let errs = an.errors.iter().filter(|x| an.defs.iter().any(|d| &d.name == name && d.line == x.line));
            lines.extend(errs.map(|x| Line { spans: vec![Span { text: upper(&x.msg), style: e, target: None }], src: 0 }));
        }
        Target::Step(id) => {
            let Some(s) = an.step(*id) else { return vec![] };
            if s.hole().is_some() {
                let arr = hole_arrives(an, s);
                first.push(" HOLE ", Style { fg: HOLE_FG, bg: Some(HOLE), weight: 500, err: false }, None);
                first.push(" IN ", fg, None);
                first.push(&upper(&s.word), w, None);
                first.push(" ARRIVES AS ", k, None);
                match arr.is_empty() {
                    true => first.push("UNKNOWN — WHAT RUNS BEFORE IT IS STILL OPEN", k, None),
                    false => first.push(&upper(&arr.join("  |  ")), fg, None),
                }
                return vec![first];
            }
            let (before, after) = shapes_at(an, s);
            let or_dash = |v: Vec<String>| if v.is_empty() { "—".to_string() } else { upper(&v.join("  |  ")) };
            let mut second = Line::default();
            if let Some(name) = s.call() {
                let open = an.holes().any(|h| &h.word == name);
                let sig = sig_text(an, name);
                let txt = match sig {
                    None => "not defined".into(),
                    Some(s) if open && s.iter().all(|x| x.ends_with("never returns")) => "still a hole".into(),
                    Some(s) => s.join("  ;  ") + if open { "  ;  still has a hole" } else { "" },
                };
                first.push(&upper(name), w, None);
                first.push(" : ", k, None);
                first.push(&upper(&txt), fg, None);
                second.push("HERE ", k, None);
                second.push(&or_dash(before), fg, None);
                second.push(" ▸ ", k, None);
                second.push(&or_dash(after), fg, None);
            } else {
                first.push("BEFORE ", k, None);
                first.push(&or_dash(before), fg, None);
                second.push("AFTER ", k, None);
                second.push(&or_dash(after), fg, None);
            }
            lines.push(second);
            let r = s.range;
            let errs = an.errors.iter().filter(|x| x.line >= r.pos.line && x.line <= r.end.line && (x.col >= r.pos.col || x.line > r.pos.line));
            lines.extend(errs.take(2).map(|x| Line { spans: vec![Span { text: upper(&x.msg), style: e, target: None }], src: 0 }));
        }
    }
    lines.insert(0, first);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        classes(&chars)
            .iter()
            .map(|c| match c {
                Class::Com => 'c',
                Class::Str => 's',
                Class::Op => 'o',
                Class::Kw => 'k',
                Class::Num => 'n',
                Class::Id => 'i',
                Class::Pun => '.',
            })
            .collect()
    }

    #[test]
    fn tokens_get_their_classes() {
        assert_eq!(kinds(r#"main = +{n: 3} say "hi" n-1 -- done"#), r#"iiii...ooi..n..kkk.ssss.i.n.ccccccc"#);
        assert_eq!(kinds("go when kind == Asset ⊃body ^^id"), "ii.kkkk.iiii....nnnnn.oiiii.ooii");
        assert_eq!(kinds("@lines( x ) post-account"), "oooooo..i...iiiiiiiiiiii");
    }
}

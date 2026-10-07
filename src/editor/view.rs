//! The screen as data: what to draw, in which colours, and what each piece of text points at.
//! colorForth-style: one screen of code on black, colour as meaning, one status line at the bottom.
use crate::editor::analysis::{Analysis, Range, Step};
use crate::editor::state::{Data, Edit, Mode, OPS, State, Tone};
use crate::ast::ColType;
use crate::runs::Verdict;
use crate::editor::debug::Debug;
use crate::machine::{self, Frame as MFrame, Status};
use crate::value::{Value, eqv, show_rec};
use crate::value::Name;

pub type Color = [u8; 3];

pub mod palette {
    pub const EDIT: Color = [0x1E, 0x1E, 0x1E];
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
    /// The data view's seed (`None`) or one of its runs.
    Run(Option<usize>),
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
    /// Characters before the program's column 0: the debugger's gutter.
    pub off: usize,
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
    /// The insert-mode caret: a code row and a column, drawn as a thin bar.
    pub caret: Option<(usize, usize)>,
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
    /// The debugger's step.
    cur: Option<Range>,
    hover: Option<Range>,
    err_steps: Vec<usize>,
    err_lines: Vec<usize>,
}


pub fn view(st: &State) -> Frame {
    if let Some(d) = &st.data {
        return data_view(st, d);
    }
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
    let sel = st.visual().map(|(pos, end)| Range { pos, end });
    let hover = match &st.hover {
        Some(Target::Step(id)) => an.step(*id).map(|s| s.range),
        _ => None,
    };
    let cur = st.debug.as_ref().and_then(Debug::current);
    let marks = Marks { sel, cur, hover, err_steps, err_lines };
    let mut code = vec![];
    let mut caret = None;
    let mut folding = false;
    for (li, chars) in an.lines.iter().enumerate() {
        let ln = li + 1;
        let text: String = chars.iter().collect();
        // a seed folds to one line, unless the cursor is in it
        let seed_end = (li + 1..an.lines.len()).find(|&j| !(an.lines[j].first().is_some_and(|c| c.is_whitespace()) && an.lines[j].iter().any(|c| !c.is_whitespace()))).unwrap_or(an.lines.len());
        let cursor_in = (ln..=seed_end).contains(&st.cursor.line);
        if text.starts_with("seed") && text[4..].starts_with(char::is_whitespace) && !cursor_in {
            code.push(gutter(st, ln, seed_line(an, li)));
            folding = true;
            continue;
        }
        if folding && text.starts_with(char::is_whitespace) && !text.trim().is_empty() {
            continue;
        }
        folding = false;
        // the agent's fill, typed into its hole as it arrives
        let typing = st.agent.as_ref().and_then(|a| {
            let h = an.step(a.hole).filter(|h| h.range.pos.line == ln)?;
            let text: Vec<char> = a.fill.as_ref().map_or(vec![], |f| f.text().chars().take(a.shown).collect());
            let range = Range { pos: h.range.pos, end: crate::ast::Pos { line: ln, col: chars.len() } };
            Some(Edit { range, cursor: text.len(), text, rework: false, hole: Some(h.id) })
        });
        if let Some(e) = typing {
            code.extend(edit_rows(st, &marks, &e));
            continue;
        }
        let mut line = Line { src: ln, ..Line::default() };
        code_spans(st, &marks, ln, 0, chars.len(), &mut line);
        if let Some(d) = st.debug.as_ref().filter(|_| cur.is_some_and(|c| c.pos.line == ln)) {
            let (ghost, bad) = ghost(&d.now().st);
            line.push(&format!("   ‹ {ghost}"), Style::plain(if bad { ERR } else { FAINT }), None);
        }
        // the cursor: a block on its character, or past the end of the line
        let here = st.cursor.line == ln && st.debug.is_none() && st.data.is_none();
        if here && st.mode == Mode::Insert {
            caret = Some((code.len(), st.cursor.col));
        } else if here && st.cursor.col >= chars.len() {
            line.push(" ", Style { fg: BG, bg: Some(FG), ..Style::plain(FG) }, None);
        }
        code.push(gutter(st, ln, line));
    }
    let mut f = Frame { code, caret, ..Frame::default() };
    match &st.debug {
        Some(d) => debug_bar(st, d, &mut f),
        None => bar(st, &mut f),
    }
    f
}

/// While debugging, every line starts with a breakpoint ● or the current step's ▸.
fn gutter(st: &State, ln: usize, line: Line) -> Line {
    let Some(d) = &st.debug else { return line };
    let (mark, fg) = match (d.bps.contains(&ln), d.current().is_some_and(|c| c.pos.line == ln)) {
        (true, _) => ("• ", ERR),
        (false, true) => ("› ", DEF),
        _ => ("  ", FAINT),
    };
    let mut out = Line { src: line.src, off: 2, ..Line::default() };
    out.push(mark, Style::plain(fg), None);
    out.spans.extend(line.spans);
    out
}

/// The record beside the current step, or why the run stopped.
fn ghost(s: &machine::State) -> (String, bool) {
    let (text, bad) = match s.status {
        Status::Error => (format!("stops: {}", s.error.as_ref().map_or("", |e| &e.msg)), true),
        Status::Hole => ("stopped at a hole".to_string(), true),
        _ => (show_rec(&s.subj), false),
    };
    let short: String = text.chars().take(108).collect();
    (if short.len() < text.len() { short + "…" } else { text }, bad)
}

/// The words and focuses the run is inside, outermost first.
fn stack(an: &Analysis, s: &machine::State) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for f in &s.k {
        let name = match f {
            MFrame::Seq { body, i, .. } => {
                let stmt = body.get(*i).or_else(|| i.checked_sub(1).and_then(|j| body.get(j)));
                let Some(w) = stmt.and_then(|x| an.steps.iter().find(|s| s.range.pos == x.pos)).map(|s| s.word.to_string()) else { continue };
                w
            }
            MFrame::Focus { field, .. } => format!("@{field}"),
            MFrame::Map { field, results, .. } => format!("@{field}[{}]", results.len()),
            MFrame::Par { results, branches, .. } => format!("‖ {}/{}", results.len() + 1, branches.len()),
        };
        if out.last() != Some(&name) {
            out.push(name);
        }
    }
    out
}

fn debug_bar(st: &State, d: &Debug, f: &mut Frame) {
    let an = &st.an;
    let now = d.now();
    let s = &now.st;
    let (k, fg) = (Style::plain(DIM), Style::plain(FG));
    let mut first = Line::default();
    first.push(&format!("RUN {} ", d.run + 1), Style::bold(DEF, 800), None);
    first.push(&upper(an.program.runs.get(d.run).map_or("", |r| r.name.as_str())), fg, None);
    first.push(&format!(" · STEP {}/{} · ", d.at, d.hist.len() - 1), k, None);
    first.push(&upper(d.reqs.get(now.req).map_or("", String::as_str)), fg, None);
    first.push(" · ", k, None);
    match s.status {
        Status::Done => first.push("FINISHED", Style::bold(CALL, 800), None),
        Status::Error => first.push(&upper(&format!("stops: {}", s.error.as_ref().map_or("", |e| &e.msg))), Style::plain(ERR), None),
        Status::Hole => first.push(" STOPPED AT A HOLE ", Style { fg: HOLE_FG, bg: Some(HOLE), weight: 500, err: false }, None),
        Status::Ready => {
            let words = stack(an, s);
            for (i, w) in words.iter().enumerate() {
                if i > 0 {
                    first.push(" › ", k, None);
                }
                first.push(&upper(w), if i + 1 == words.len() { Style::bold(CALL, 800) } else { fg }, None);
            }
        }
    }
    // the record, its changes marked when the step stayed in the same place
    let prev = d.at.checked_sub(1).map(|i| &d.hist[i]).filter(|p| p.req == now.req && stack(an, &p.st) == stack(an, s)).map(|p| p.st.subj.clone());
    let mut rec = Line::default();
    rec.push("{", fg, None);
    for (i, (key, v)) in s.subj.iter().enumerate() {
        if i > 0 {
            rec.push(", ", k, None);
        }
        let was = prev.as_ref().map(|p| p.get(key));
        let (mark, c) = match was {
            Some(None) => ("+", CALL),
            Some(Some(w)) if !eqv(w, v) => ("~", OP),
            _ => ("", FG),
        };
        rec.push(&format!("{mark}{}", upper(key)), Style::plain(c), None);
        rec.push(": ", k, None);
        rec.push(&v.show(), fg, None);
    }
    for key in prev.iter().flat_map(|p| p.keys()).filter(|key| !s.subj.contains_key(*key)) {
        rec.push(&format!(", −{}", upper(key)), Style::plain(ERR), None);
    }
    rec.push("}", fg, None);
    f.shape = vec![first, rec];
    let said = &d.log[..s.out_len.min(d.log.len())];
    let last = said.last().map_or("nothing printed yet".to_string(), |l| format!("printed {l}"));
    f.msg.push(&last, fg, None);
    f.msg.push("  → INTO · ↓ OVER · ↑ OUT · ← BACK · ENTER CONTINUE · HOME RESTART · CLICK A STEP FOR A BREAKPOINT · 1–9 RUN · ESC CODE", k, None);
    stat(st, f);
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

/// The rows of a line being edited: what's before the edit, the typed text with its caret
/// (one row per typed line), and what's after it.
fn edit_rows(st: &State, m: &Marks, e: &Edit) -> Vec<Line> {
    let (ln, end) = (e.range.pos.line, e.range.end);
    let style = match e.rework {
        true => Style { fg: FG, bg: Some(EDIT), weight: 700, err: false },
        false => Style { fg: HOLE_FG, bg: Some(HOLE), weight: 700, err: false },
    };
    let caret = Style { fg: style.bg.unwrap_or(BG), bg: Some(style.fg), ..style };
    let mut rows = vec![Line { src: ln, ..Line::default() }];
    code_spans(st, m, ln, 0, e.range.pos.col, &mut rows[0]);
    for (i, &c) in e.text.iter().enumerate() {
        let row = rows.last_mut().unwrap();
        match c {
            '\n' => {
                if i == e.cursor {
                    row.push(" ", caret, None);
                }
                rows.push(Line { src: ln, ..Line::default() });
            }
            c => row.push(&c.to_string(), if i == e.cursor { caret } else { style }, None),
        }
    }
    let last = rows.last_mut().unwrap();
    if e.cursor == e.text.len() {
        last.push(" ", caret, None);
    }
    code_spans(st, m, end.line, end.col, st.an.lines[end.line - 1].len(), last);
    rows
}

/// The characters `a..b` of program line `ln`, styled, onto `line`.
fn code_spans(st: &State, m: &Marks, ln: usize, a: usize, b: usize, line: &mut Line) {
    let an = &st.an;
    let chars = &an.lines[ln - 1];
    let cls = classes(chars);
    let row = &an.cells[ln - 1];
    let def = an.def_at(ln);
    let hole = an.holes().find(|h| h.range.pos.line == ln);
    let table_decl = chars.iter().collect::<String>().strip_prefix("table").filter(|r| r.starts_with(' ')).map(|r| r.split_whitespace().next().unwrap_or("").to_string());
    for (i, &ch) in chars.iter().enumerate().take(b).skip(a) {
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
        if m.cur.is_some_and(|r| r.contains(ln, i)) && !is_hole {
            style = Style { fg: BG, bg: Some(DEF), ..style };
        }
        let block = st.cursor == (crate::ast::Pos { line: ln, col: i }) && st.mode != Mode::Insert && st.debug.is_none() && st.data.is_none();
        let selected = m.sel.is_some_and(|r| r.contains(ln, i)) || block;
        if selected {
            style = Style { fg: if is_hole { HOLE } else { BG }, bg: Some(FG), ..style };
        } else if m.hover.is_some_and(|r| r.contains(ln, i)) && !is_hole && st.mode != Mode::Insert {
            style.bg = Some(HOVER);
        }
        style.err = (sid != 0 && m.err_steps.contains(&sid)) || (in_def && m.err_lines.contains(&ln));
        line.push(&c.to_string(), style, target);
    }
}

// ---------- the status bar ----------
const HINT: &str = "i a o to write · esc · v V to select · :extract name :inline :hole :rework :drop :swap :rename name · :data · :debug n · :reset table · enter on a hole writes it, ga hands it to claude · u ⌃r · it’s live as soon as it parses";

fn upper(s: &str) -> String {
    s.to_uppercase()
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
    (crate::editor::analysis::best(c.before.get(&k)), crate::editor::analysis::best(c.after.get(&k)))
}

/// `HOLES n · ERRORS n · RUNS p/t`, in red where something needs doing.
fn stat(st: &State, f: &mut Frame) {
    let an = &st.an;
    let mode = match &st.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Visual(_, false) => "VISUAL",
        Mode::Visual(_, true) => "V-LINE",
        Mode::Cmd(..) => "COMMAND",
    };
    f.stat.push(&format!("{mode} {}:{} · ", st.cursor.line, st.cursor.col + 1), Style::plain(if st.mode == Mode::Insert { CALL } else { FG }), None);
    if st.image.text != st.an.text {
        f.stat.push("DOESN’T PARSE · ", Style::plain(ERR), None);
    }
    let (holes, errs) = (an.holes().count(), an.errors.len());
    let dim = Style::plain(DIM);
    f.stat.push(&format!("HOLES {holes}"), if holes > 0 { Style::plain(HOLE) } else { dim }, None);
    f.stat.push(" · ", dim, None);
    f.stat.push(&format!("ERRORS {errs}"), if errs > 0 { Style::plain(ERR) } else { dim }, None);
    f.stat.push(&format!(" · RUNS {}/{}", an.passing(), an.runs.len()), dim, None);
    if let Some((port, last)) = &st.live {
        f.stat.push(" · ", dim, None);
        f.stat.push(&format!("LIVE :{port}"), Style::plain(CALL), None);
        if let Some(l) = last {
            f.stat.push(&format!(" {}", upper(l)), dim, None);
        }
    }
    if !st.pending.is_empty() {
        f.stat.push(&format!(" · PENDING {}", st.pending.len()), Style::plain(ERR), None);
    }
}

fn bar(st: &State, f: &mut Frame) {
    let an = &st.an;
    let dim = Style::plain(DIM);
    stat(st, f);
    match (&st.mode, &st.msg) {
        (Mode::Cmd(c, _), _) => {
            let first = c.split_whitespace().next().unwrap_or("").to_lowercase();
            let sug: Vec<&str> = OPS.iter().copied().filter(|o| o.starts_with(&first)).collect();
            f.msg.push(&format!(":{}▌", upper(c)), Style::plain(FG), None);
            f.msg.push(&format!("  {}", upper(&sug.join(" · "))), dim, None);
        }
        (_, Some((text, tone))) => {
            let fg = match tone {
                Tone::Ok => CALL,
                Tone::No => ERR,
                Tone::Cmd => FG,
                Tone::Agent => DEF,
                Tone::Plain => DIM,
            };
            f.msg.push(&upper(text), Style::plain(fg), None);
        }
        (_, None) => f.msg.push(&upper(HINT), dim, None),
    }
    // the bar describes what the mouse is over, else what the cursor is on
    let c = st.cursor;
    let def = an.def_at(c.line).filter(|d| c.col < d.name.chars().count()).map(|d| Target::Def(d.name.clone(), c.line));
    let target = st.hover.clone().or(def).or_else(|| st.step_at_cursor().map(|s| Target::Step(s.id)));
    f.shape = target.map(|t| shape(an, &t)).unwrap_or_default();
    if f.shape.is_empty() && !st.pending.is_empty() {
        let mut l = Line::default();
        l.push(&upper(&format!("pending: {} — :reset the table to apply", st.pending.join(" · "))), Style::plain(ERR), None);
        f.shape.push(l);
    }
}

fn shape(an: &Analysis, t: &Target) -> Vec<Line> {
    let k = Style::plain(DIM);
    let w = Style::bold(CALL, 800);
    let fg = Style::plain(FG);
    let e = Style::plain(ERR);
    let mut first = Line::default();
    let mut lines = vec![];
    match t {
        Target::Run(_) => {}
        Target::Table(name) => {
            first.push(&upper(name), Style::bold(TABLE, 800), None);
            first.push(" · CLICK TO SEE ITS ROWS", k, None);
        }
        Target::Def(name, _) => {
            first.push(&upper(name), w, None);
            first.push(" : ", k, None);
            first.push(&upper(&sig_text(an, name).map_or_else(|| "not checked".into(), |s| s.join("  ;  "))), fg, None);
            let errs = an.errors.iter().filter(|x| an.defs.iter().any(|d| &d.name == name && d.line == x.line));
            lines.extend(errs.map(|x| Line { spans: vec![Span { text: upper(&x.msg), style: e, target: None }], src: 0, off: 0 }));
        }
        Target::Step(id) => {
            let Some(s) = an.step(*id) else { return vec![] };
            if s.hole().is_some() {
                let arr = an.arrives(s);
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
                second.push(" › ", k, None);
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
            lines.extend(errs.take(2).map(|x| Line { spans: vec![Span { text: upper(&x.msg), style: e, target: None }], src: 0, off: 0 }));
        }
    }
    lines.insert(0, first);
    lines
}

// ---------- the data view ----------
fn verdict_color(v: Verdict) -> Color {
    match v {
        Verdict::Pass => CALL,
        Verdict::Fail | Verdict::Error => ERR,
        Verdict::Waits => HOLE,
    }
}

/// A cell: its text and colour, by the column's type.
fn cell(ty: ColType, v: Option<&Value>) -> (String, Color) {
    match (ty, v) {
        (_, None | Some(Value::Nil)) => ("—".into(), FAINT),
        (ColType::Int | ColType::Num | ColType::Const, Some(v)) => (upper(&v.show()), NUM),
        (ColType::Text, Some(v)) => (v.text(), STR),
        (ColType::Bool, Some(v)) => (upper(&v.show()), KW),
        (ColType::Json, Some(v)) => (v.show(), FG),
    }
}

fn data_view(st: &State, d: &Data) -> Frame {
    let an = &st.an;
    let mut f = Frame::default();
    let Some(t) = an.program.tables.get(&d.table) else { return f };
    let run = d.run.and_then(|i| Some((i, an.runs.get(i)?, an.program.runs.get(i)?)));
    let seeded = an.seeded();
    let before = seeded.get(&t.name).cloned().unwrap_or_default();
    let after = run.map_or_else(|| before.clone(), |(_, o, _)| o.end.get(&t.name).cloned().unwrap_or_default());
    let id = |r: &Value| r.as_rec().and_then(|r| r.get("id")).map(Value::show).unwrap_or_default();
    let get = |r: &Value, k: &str| r.as_rec().and_then(|r| r.get(k)).cloned();
    let cols: Vec<(Name, ColType, bool)> = std::iter::once(("id".into(), ColType::Int, false)).chain(t.cols.iter().map(|c| (c.name.clone(), c.ty, c.opt))).collect();
    // every row after, then the ones the run deleted; marked + − ~
    let mut rows: Vec<(Value, char, Vec<Name>)> = vec![];
    for r in &after {
        let old = before.iter().find(|b| id(b) == id(r));
        let changed: Vec<Name> = old.map_or(vec![], |o| cols.iter().filter(|c| !opt_eq(get(o, &c.0), get(r, &c.0))).map(|c| c.0.clone()).collect());
        let mark = if old.is_none() { '+' } else if changed.is_empty() { ' ' } else { '~' };
        rows.push((r.clone(), mark, changed));
    }
    rows.extend(before.iter().filter(|b| !after.iter().any(|a| id(a) == id(b))).map(|b| (b.clone(), '−', vec![])));
    rows.sort_by(|a, b| id(&a.0).parse::<f64>().unwrap_or(0.0).total_cmp(&id(&b.0).parse::<f64>().unwrap_or(0.0)));

    // tabs and runs
    let mut head = Line::default();
    for name in an.program.tables.keys() {
        let style = if *name == t.name { Style::bold(TABLE, 800) } else { Style::plain(FAINT) };
        head.push(&upper(name), style, Some(Target::Table(name.clone())));
        head.push("  ", Style::plain(FAINT), None);
    }
    head.push("  ", Style::plain(FAINT), None);
    let on = |yes: bool, fg: Color| Style { fg: if yes { FG } else { fg }, bg: yes.then_some(RULE), weight: 500, err: false };
    head.push(" SEED ", on(d.run.is_none(), FAINT), Some(Target::Run(None)));
    for (i, o) in an.runs.iter().enumerate() {
        head.push(&format!(" {} ", i + 1), on(d.run == Some(i), verdict_color(o.verdict)), Some(Target::Run(Some(i))));
    }
    f.code.push(head);
    f.code.push(Line::default());

    // the table, in columns three cells apart
    let text = |r: &Value, c: &(Name, ColType, bool)| cell(c.1, get(r, &c.0).as_ref()).0;
    let widths: Vec<usize> = cols.iter().map(|c| rows.iter().map(|r| text(&r.0, c).chars().count()).chain([c.0.chars().count(), c.1.name().len() + 1]).max().unwrap_or(0) + 3).collect();
    let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count())));
    let mut names = Line::default();
    let mut types = Line::default();
    names.push("  ", Style::plain(FG), None);
    types.push("  ", Style::plain(FAINT), None);
    for (c, w) in cols.iter().zip(&widths) {
        names.push(&pad(&upper(&c.0), *w), Style::bold(FG, 800), None);
        types.push(&pad(&upper(&format!("{}{}", c.1.name(), if c.2 { "?" } else { "" })), *w), Style::plain(FAINT), None);
    }
    f.code.extend([names, types]);
    if rows.is_empty() {
        f.code.push(Line { spans: vec![Span { text: "  NO ROWS".into(), style: Style::plain(FAINT), target: None }], src: 0, off: 0 });
    }
    for (r, mark, changed) in &rows {
        let mut line = Line::default();
        let mc = match mark {
            '+' => CALL,
            '−' => ERR,
            '~' => OP,
            _ => FG,
        };
        let bg = (*mark == '+').then_some([0x07, 0x17, 0x0D]);
        line.push(&format!("{mark} "), Style::bold(mc, 800), None);
        for (c, w) in cols.iter().zip(&widths) {
            let (s, fg) = cell(c.1, get(r, &c.0).as_ref());
            let fg = if *mark == '−' { FAINT } else if changed.contains(&c.0) { OP } else { fg };
            line.push(&pad(&s, *w), Style { fg, bg, weight: 500, err: false }, None);
        }
        f.code.push(line);
    }

    // what the run asked of this table
    if let Some((i, o, _)) = run {
        f.code.push(Line::default());
        let q = format!("\"{}\"", t.name);
        let mut h = Line::default();
        h.push(&upper(&format!("what run {} asked of {}", i + 1, t.name)), Style::plain(DIM), None);
        f.code.push(h);
        let ops: Vec<&String> = o.sql.iter().filter(|l| l.contains(&q)).collect();
        if ops.is_empty() {
            f.code.push(Line { spans: vec![Span { text: "NOTHING".into(), style: Style::plain(FAINT), target: None }], src: 0, off: 0 });
        }
        for l in ops {
            f.code.push(Line { spans: vec![Span { text: l.clone(), style: Style::plain(FAINT), target: None }], src: 0, off: 0 });
        }
    }

    // the bar
    let mut first = Line::default();
    first.push(&upper(&t.name), Style::bold(TABLE, 800), None);
    let decl: Vec<String> = t.cols.iter().map(|c| format!("{}: {}{}", c.name, c.ty.name(), if c.opt { "?" } else { "" })).collect();
    first.push(&upper(&format!(" :: {{id: Int, {}}}", decl.join(", "))), Style::plain(DIM), None);
    let mut second = Line::default();
    second.push(&format!("{} ROWS ", after.len()), Style::plain(FG), None);
    match run {
        None => second.push("AS SEEDED", Style::plain(FG), None),
        Some((i, o, r)) => {
            second.push(&upper(&format!("after run {} · {} · ", i + 1, r.name)), Style::plain(FG), None);
            let (what, c) = match o.verdict {
                Verdict::Pass => ("passes".to_string(), CALL),
                Verdict::Waits => ("stopped at a hole".to_string(), HOLE),
                Verdict::Error => (format!("stops: {}", o.detail.clone().unwrap_or_default()), ERR),
                Verdict::Fail => ("answers something else".to_string(), ERR),
            };
            second.push(&upper(&what), Style::plain(c), None);
            let n = |m: char| rows.iter().filter(|r| r.1 == m).count();
            second.push(&format!(" · +{} −{} ~{}", n('+'), n('−'), n('~')), Style::plain(DIM), None);
        }
    }
    f.shape = vec![first, second];
    f.msg.push("← → SEED AND RUNS · ↑ ↓ TABLES · ESC BACK TO THE CODE", Style::plain(DIM), None);
    stat(st, &mut f);
    f
}

fn opt_eq(a: Option<Value>, b: Option<Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => eqv(&a, &b),
        (a, b) => a.is_none() && b.is_none(),
    }
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

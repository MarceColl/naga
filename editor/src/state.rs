//! The editor's state and the one function that changes it. `update` takes a state and an
//! event and returns the next state; the program text inside the analysis is the only data,
//! and undo is a stack of earlier texts.
use crate::analysis::{Analysis, Parent, Range, Step};
use crate::ops;
use crate::view::Target;
use naga::value::Name;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    /// Sibling steps, in order.
    Steps(Vec<usize>),
    /// A word's name at the head of the clause on this line.
    Def(Name, usize),
}

/// Text typed in place of `range`: a hole's fill, or a rewrite of selected steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range,
    pub text: Vec<char>,
    pub cursor: usize,
    /// Rewriting existing steps rather than filling a hole.
    pub rework: bool,
    pub hole: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    /// A keyword operation being typed.
    Cmd(String),
    Edit(Edit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Escape,
    Enter,
    Backspace,
    Char(char),
    Undo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Ok,
    No,
    Cmd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The mouse went down on this target, or on nothing.
    Press(Option<Target>, bool),
    /// The mouse moved over this target (`drag`: with the button down).
    Hover(Option<Target>, bool),
    Release,
    Key(Key, bool),
    /// Lines to scroll by.
    Scroll(f32),
    /// How many code rows fit, and which program line each displayed row shows.
    Viewport(usize, Vec<usize>),
}

/// A table's rows, as seeded (`run: None`) or after a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data {
    pub table: Name,
    pub run: Option<usize>,
}

#[derive(Clone)]
pub struct State {
    pub an: Rc<Analysis>,
    pub data: Option<Data>,
    pub undo: Vec<String>,
    pub sel: Option<Sel>,
    /// Where a shift-extended selection started.
    pub anchor: Option<usize>,
    pub hover: Option<Target>,
    pub dragging: bool,
    pub mode: Mode,
    pub msg: Option<(String, Tone)>,
    /// First row shown, in rows (fractional while scrolling).
    pub scroll: f32,
    /// Rows that fit on screen, and the program line of each row.
    pub rows: usize,
    pub row_lines: Rc<Vec<usize>>,
}

pub const OPS: [&str; 10] = ["extract name", "inline", "hole note", "rework", "edit", "drop", "swap", "rename name", "data", "debug 1–7"];

impl State {
    pub fn new(text: &str) -> State {
        State {
            an: Rc::new(Analysis::new(text)),
            data: None,
            undo: vec![],
            sel: None,
            anchor: None,
            hover: None,
            dragging: false,
            mode: Mode::Idle,
            msg: None,
            scroll: 0.0,
            rows: 0,
            row_lines: Rc::default(),
        }
    }
    fn say(self, text: impl Into<String>, tone: Tone) -> State {
        State { msg: Some((text.into(), tone)), ..self }
    }
}

/// `checker clean · runs 7/7`, or how many errors there are.
pub fn status(an: &Analysis) -> String {
    let checker = match an.errors.len() {
        0 => "checker clean".to_string(),
        1 => "1 checker error".to_string(),
        n => format!("{n} checker errors"),
    };
    format!("{checker} · runs {}/{}", an.passing(), an.runs.len())
}

/// The analysis of `text`, when it parses.
fn parse_ok(text: &str) -> Result<Analysis, String> {
    let an = Analysis::new(text);
    match an.program.errors.first() {
        Some(e) => Err(e.msg.clone()),
        None => Ok(an),
    }
}

/// Makes `an` the program, keeping the old text for undo.
fn take(st: State, an: Analysis, ok: &str) -> State {
    let first = an.errors.first().map(|e| format!(" — {}", e.msg)).unwrap_or_default();
    let msg = format!("{}{}{first}", if ok.is_empty() { String::new() } else { format!("{ok} · ") }, status(&an));
    let tone = if an.errors.is_empty() { Tone::Ok } else { Tone::No };
    let mut undo = st.undo.clone();
    undo.push(st.an.text.clone());
    State { an: Rc::new(an), undo, sel: None, anchor: None, hover: None, mode: Mode::Idle, ..st }.say(msg, tone)
}

fn commit_or_say(st: State, text: R, ok: &str) -> State {
    match text {
        Err(why) => st.say(why, Tone::No),
        Ok(text) => match parse_ok(&text) {
            Ok(an) => take(st, an, ok),
            Err(why) => st.say(format!("that doesn’t parse: {why}"), Tone::No),
        },
    }
}

type R = ops::R<String>;

/// `step`, or the ancestor of it that sits beside steps whose parent is `parent`.
fn align<'a>(an: &'a Analysis, step: &'a Step, parent: &Parent) -> Option<&'a Step> {
    let mut s = step;
    while &s.parent != parent {
        let Parent::Step(id, _) = s.parent else { return None };
        s = an.step(id)?;
    }
    Some(s)
}

/// Every sibling between `a` and `b`, both included.
fn range(an: &Analysis, a: &Step, b: &Step) -> Sel {
    let (lo, hi) = (a.idx.min(b.idx), a.idx.max(b.idx));
    Sel::Steps(an.siblings(a).iter().filter(|x| (lo..=hi).contains(&x.idx)).map(|x| x.id).collect())
}

fn one(s: &Step) -> (Option<Sel>, Option<usize>) {
    (Some(Sel::Steps(vec![s.id])), Some(s.id))
}

/// Writing a hole's fill, from the hole to the end of its line.
fn edit_hole(st: State, h: &Step) -> State {
    let line = h.range.pos.line;
    let range = Range { pos: h.range.pos, end: naga::ast::Pos { line, col: st.an.lines[line - 1].len() } };
    let e = Edit { range, text: vec![], cursor: 0, rework: false, hole: Some(h.id) };
    State { mode: Mode::Edit(e), sel: None, ..st }.say("write the fill · enter keeps it · shift-enter for a new line · esc leaves the hole", Tone::Cmd)
}

/// Opens the data view on `table`, keeping the run it showed last.
fn open_data(st: State, table: &str) -> State {
    if !st.an.program.tables.contains_key(table) {
        return st.say(format!("there's no table called {table}"), Tone::No);
    }
    let run = st.data.as_ref().and_then(|d| d.run);
    State { data: Some(Data { table: table.into(), run }), sel: None, hover: None, ..st }
}

fn data(st: State, d: Data, ev: Event) -> State {
    let tables: Vec<Name> = st.an.program.tables.keys().cloned().collect();
    let runs = st.an.runs.len();
    let at = tables.iter().position(|t| *t == d.table).unwrap_or(0);
    let d = match ev {
        Event::Key(Key::Escape, _) => return State { data: None, ..st },
        Event::Key(Key::Right, _) => Data { run: Some(d.run.map_or(0, |r| r + 1).min(runs.saturating_sub(1))).filter(|_| runs > 0), ..d },
        Event::Key(Key::Left, _) => Data { run: d.run.and_then(|r| r.checked_sub(1)), ..d },
        Event::Key(Key::Down, _) => Data { table: tables[(at + 1) % tables.len()].clone(), ..d },
        Event::Key(Key::Up, _) => Data { table: tables[(at + tables.len() - 1) % tables.len()].clone(), ..d },
        Event::Press(Some(Target::Table(t)), _) => Data { table: t, ..d },
        Event::Press(Some(Target::Run(r)), _) => Data { run: r, ..d },
        Event::Hover(t, _) => return State { hover: t, ..st },
        _ => d,
    };
    State { data: Some(d), ..st }
}

pub fn update(st: State, ev: Event) -> State {
    match (&st.mode, ev) {
        (_, Event::Viewport(rows, lines)) => State { rows, row_lines: Rc::new(lines), ..st },
        (Mode::Idle, ev) if st.data.is_some() && !matches!(ev, Event::Scroll(_)) => {
            let d = st.data.clone().unwrap();
            data(st, d, ev)
        }
        (_, Event::Scroll(dy)) => State { scroll: (st.scroll + dy).clamp(0.0, st.row_lines.len() as f32), ..st },
        (Mode::Edit(e), Event::Key(k, shift)) => {
            let e = e.clone();
            editing(st, e, k, shift)
        }
        (Mode::Edit(_), _) => st,
        (Mode::Cmd(c), Event::Key(k, _)) => {
            let c = c.clone();
            command(st, c, k)
        }
        (_, ev @ Event::Press(..)) => idle(State { mode: Mode::Idle, ..st }, ev),
        (_, ev) => idle(st, ev),
    }
}

fn editing(st: State, mut e: Edit, k: Key, shift: bool) -> State {
    match k {
        Key::Escape => State { mode: Mode::Idle, ..st }.say("left as it was", Tone::Plain),
        Key::Enter if !shift => finish(st, e),
        Key::Enter | Key::Char(_) => {
            e.text.insert(e.cursor, if let Key::Char(c) = k { c } else { '\n' });
            e.cursor += 1;
            State { mode: Mode::Edit(e), ..st }
        }
        Key::Backspace if e.cursor > 0 => {
            e.cursor -= 1;
            e.text.remove(e.cursor);
            State { mode: Mode::Edit(e), ..st }
        }
        Key::Left => State { mode: Mode::Edit(Edit { cursor: e.cursor.saturating_sub(1), ..e }), ..st },
        Key::Right => State { mode: Mode::Edit(Edit { cursor: (e.cursor + 1).min(e.text.len()), ..e }), ..st },
        _ => st,
    }
}

fn finish(st: State, e: Edit) -> State {
    let typed: String = e.text.iter().collect();
    if typed.trim().is_empty() {
        return State { mode: Mode::Idle, ..st }.say("empty — nothing changed", Tone::Plain);
    }
    let an = st.an.clone();
    let (text, ok) = match e.hole.and_then(|h| an.step(h)) {
        Some(h) => (ops::fill(&an, h, &typed, None), format!("filled {}", h.word)),
        None => (ops::edit_range(&an, e.range, &typed), "rewritten".into()),
    };
    match parse_ok(&text) {
        Ok(an) => take(st, an, &ok),
        Err(why) => State { mode: Mode::Edit(e), ..st }.say(format!("that doesn’t parse: {why} — fix it or esc"), Tone::No),
    }
}

fn command(st: State, mut c: String, k: Key) -> State {
    match k {
        Key::Escape => State { mode: Mode::Idle, ..st },
        Key::Enter => run(State { mode: Mode::Idle, ..st }, &c),
        Key::Backspace => {
            c.pop();
            State { mode: if c.is_empty() { Mode::Idle } else { Mode::Cmd(c) }, ..st }
        }
        Key::Char(ch) => {
            c.push(ch);
            State { mode: Mode::Cmd(c), ..st }
        }
        _ => st,
    }
}

/// A keyword operation on the selection: `extract name`, `inline`, `hole note`, …
fn run(st: State, raw: &str) -> State {
    let mut words = raw.split_whitespace();
    let op = words.next().unwrap_or("").to_lowercase();
    let arg = words.collect::<Vec<_>>().join(" ");
    let an = st.an.clone();
    let ids = match &st.sel {
        Some(Sel::Steps(ids)) => Some(ids.clone()),
        _ => None,
    };
    let call = ids.as_ref().filter(|i| i.len() == 1).and_then(|i| an.step(i[0])).and_then(|s| s.call().cloned());
    let need = |what: &str| format!("select {what} first");
    match op.as_str() {
        "extract" | "factor" => match &ids {
            Some(ids) => commit_or_say(st, ops::extract(&an, ids, &arg.to_lowercase()), &format!("extracted {}", arg.to_lowercase())),
            None => st.say(need("steps"), Tone::No),
        },
        "inline" => match &ids {
            Some(ids) => commit_or_say(st, ops::inline(&an, ids), &format!("inlined {}", call.as_deref().unwrap_or(""))),
            None => st.say(need("a call"), Tone::No),
        },
        "drop" => match &ids {
            Some(ids) => commit_or_say(st, ops::drop(&an, ids), "dropped"),
            None => st.say(need("steps"), Tone::No),
        },
        "swap" => match &ids {
            Some(ids) => commit_or_say(st, ops::swap(&an, ids), "swapped"),
            None => st.say(need("two steps"), Tone::No),
        },
        "rename" => {
            let word = match &st.sel {
                Some(Sel::Def(n, _)) => Some(n.clone()),
                _ => call,
            };
            match word {
                Some(w) => commit_or_say(st, ops::rename(&an, &w, &arg.to_lowercase()), &format!("renamed {w} to {}", arg.to_lowercase())),
                None => st.say("select a word name or a call to rename", Tone::No),
            }
        }
        "hole" | "punch" => match &ids {
            Some(ids) => commit_or_say(st, ops::punch(&an, ids, &arg).map(|p| p.0), "hole punched"),
            None => st.say(need("steps"), Tone::No),
        },
        "edit" => match ids.as_ref().map(|ids| ops::sel_range(&an, ids)) {
            Some(Ok(r)) => {
                let text: Vec<char> = ops::one_line(&ops::slice(&an, r)).chars().collect();
                let e = Edit { range: r, cursor: text.len(), text, rework: true, hole: None };
                State { mode: Mode::Edit(e), sel: None, ..st }.say("rewrite it · enter keeps it · shift-enter for a new line · esc puts it back", Tone::Cmd)
            }
            _ => st.say(need("steps"), Tone::No),
        },
        "data" => {
            let table = ids.as_ref().and_then(|i| an.step(i[0])).and_then(|s| s.table().cloned()).or_else(|| an.program.tables.keys().next().cloned());
            match table {
                Some(t) => open_data(st, &t),
                None => st.say("this program has no tables", Tone::No),
            }
        }
        "rework" | "debug" | "run" => st.say(format!("{op} isn’t built yet"), Tone::No),
        _ => st.say(format!("{raw} isn’t an operation — try {}", OPS.join(" · ")), Tone::No),
    }
}

fn idle(st: State, ev: Event) -> State {
    let an = st.an.clone();
    match ev {
        Event::Press(None, _) => State { sel: None, anchor: None, ..st },
        Event::Press(Some(Target::Def(name, line)), _) => State { sel: Some(Sel::Def(name, line)), anchor: None, ..st },
        Event::Press(Some(Target::Table(t)), _) => open_data(st, &t),
        Event::Press(Some(Target::Run(_)), _) => st,
        Event::Press(Some(Target::Step(id)), shift) => {
            let Some(s) = an.step(id) else { return st };
            if s.hole().is_some() {
                return if shift { st.say("no agent yet — click the hole to write it yourself", Tone::No) } else { edit_hole(st, s) };
            }
            let anchor = st.anchor.and_then(|a| an.step(a));
            let (sel, anchor) = match anchor.filter(|_| shift).and_then(|a| Some((a, align(&an, s, &a.parent)?))) {
                Some((a, b)) => (Some(range(&an, a, b)), Some(a.id)),
                None => one(s),
            };
            State { sel, anchor, dragging: true, ..st }
        }
        Event::Hover(t, true) if st.dragging => {
            let a = st.anchor.and_then(|a| an.step(a));
            let b = match (&t, a) {
                (Some(Target::Step(id)), Some(a)) => an.step(*id).and_then(|s| align(&an, s, &a.parent)),
                _ => None,
            };
            let sel = match (a, b) {
                (Some(a), Some(b)) => Some(range(&an, a, b)),
                _ => st.sel.clone(),
            };
            State { sel, hover: t, ..st }
        }
        Event::Hover(t, _) => State { hover: t, ..st },
        Event::Release => State { dragging: false, ..st },
        Event::Key(Key::Undo, _) => match st.undo.last().cloned() {
            None => st.say("nothing to undo", Tone::Plain),
            Some(text) => {
                let mut undo = st.undo.clone();
                undo.pop();
                let an = Analysis::new(&text);
                let msg = format!("undone · {}", status(&an));
                State { an: Rc::new(an), undo, sel: None, anchor: None, hover: None, ..st }.say(msg, Tone::Plain)
            }
        },
        Event::Key(Key::Escape, _) => State { sel: None, hover: None, msg: None, ..st },
        Event::Key(Key::Enter, shift) => match &st.sel {
            Some(Sel::Steps(ids)) if ids.len() == 1 && an.step(ids[0]).is_some_and(|s| s.hole().is_some()) && !shift => edit_hole(st.clone(), an.step(ids[0]).unwrap()),
            _ => st,
        },
        Event::Key(Key::Char(c), _) if c.is_alphabetic() => State { mode: Mode::Cmd(c.to_string()), ..st },
        Event::Key(k @ (Key::Left | Key::Right), shift) => reveal(step_along(st, if k == Key::Right { 1 } else { -1 }, shift)),
        Event::Key(k @ (Key::Up | Key::Down), _) => reveal(step_across(st, k == Key::Up)),
        _ => st,
    }
}

/// Scrolls just enough to show the selection's first line.
fn reveal(st: State) -> State {
    let line = match &st.sel {
        Some(Sel::Steps(ids)) => ids.first().and_then(|&i| st.an.step(i)).map(|s| s.range.pos.line),
        Some(Sel::Def(_, l)) => Some(*l),
        None => None,
    };
    let Some(row) = line.and_then(|l| st.row_lines.iter().position(|&x| x == l)) else { return st };
    let (top, rows) = (st.scroll.floor() as usize, st.rows.max(1));
    let scroll = match row {
        r if r < top => r as f32,
        r if r + 1 >= top + rows => (r + 2).saturating_sub(rows) as f32,
        _ => st.scroll,
    };
    State { scroll, ..st }
}

/// ← → move to the previous or next sibling (or step, in reading order); with shift, extend.
fn step_along(st: State, dir: isize, extend: bool) -> State {
    let an = st.an.clone();
    let mut order: Vec<&Step> = an.steps.iter().collect();
    order.sort_by_key(|s| (s.range.pos.line, s.range.pos.col, s.depth));
    let ids = match &st.sel {
        Some(Sel::Steps(ids)) => ids,
        _ => {
            let Some(s) = order.first() else { return st };
            let (sel, anchor) = one(s);
            return State { sel, anchor, ..st };
        }
    };
    let Some(cur) = an.step(if dir > 0 { *ids.last().unwrap() } else { ids[0] }) else { return st };
    let sib = an.siblings(cur);
    let next_sib = sib.iter().find(|x| x.idx as isize == cur.idx as isize + dir).copied();
    if extend {
        let Some(nx) = next_sib else { return st };
        let a = st.anchor.and_then(|a| an.step(a)).filter(|a| a.parent == cur.parent).unwrap_or(cur);
        return State { sel: Some(range(&an, a, nx)), anchor: Some(a.id), ..st };
    }
    let at = order.iter().position(|s| s.id == cur.id).unwrap() as isize + dir;
    let nx = next_sib.or_else(|| usize::try_from(at).ok().and_then(|i| order.get(i).copied()));
    match nx {
        Some(nx) => {
            let (sel, anchor) = one(nx);
            State { sel, anchor, ..st }
        }
        None => st,
    }
}

/// ↑ goes out to the enclosing step, ↓ into the first step inside.
fn step_across(st: State, up: bool) -> State {
    let an = st.an.clone();
    let Some(Sel::Steps(ids)) = &st.sel else { return step_along(st, 1, false) };
    let Some(cur) = an.step(ids[0]) else { return st };
    let nx = match up {
        true => match cur.parent {
            Parent::Step(id, _) => an.step(id),
            Parent::Clause(..) => None,
        },
        false => an.steps.iter().filter(|x| matches!(x.parent, Parent::Step(id, _) if id == cur.id)).min_by_key(|x| x.idx),
    };
    match nx {
        Some(nx) => {
            let (sel, anchor) = one(nx);
            State { sel, anchor, ..st }
        }
        None => st,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "go = a @f( b c ) d\na = nop\nb = nop\nc = nop\nd = nop\nh = ? write me\n";

    fn names(st: &State) -> Vec<String> {
        match &st.sel {
            Some(Sel::Steps(ids)) => ids.iter().map(|&i| ops::slice(&st.an, st.an.step(i).unwrap().range)).collect(),
            _ => vec![],
        }
    }

    fn keys(st: State, ks: &[(Key, bool)]) -> State {
        ks.iter().fold(st, |s, &(k, shift)| update(s, Event::Key(k, shift)))
    }

    fn typed(st: State, s: &str) -> State {
        s.chars().fold(st, |st, c| update(st, Event::Key(if c == '\n' { Key::Enter } else { Key::Char(c) }, c == '\n')))
    }

    fn id(st: &State, text: &str) -> usize {
        st.an.steps.iter().find(|s| ops::slice(&st.an, s.range) == text).unwrap().id
    }

    #[test]
    fn arrows_walk_siblings_in_and_out() {
        let st = keys(State::new(SRC), &[(Key::Right, false)]);
        assert_eq!(names(&st), ["a"]);
        let st = keys(st, &[(Key::Right, false)]);
        assert_eq!(names(&st), ["@f( b c )"]);
        let st = keys(st, &[(Key::Down, false), (Key::Right, true)]);
        assert_eq!(names(&st), ["b", "c"]);
        let st = keys(st, &[(Key::Up, false), (Key::Right, false)]);
        assert_eq!(names(&st), ["d"]);
    }

    #[test]
    fn shift_click_selects_siblings_even_from_inside() {
        let st = State::new(SRC);
        let (a, c) = (id(&st, "a"), id(&st, "c"));
        let st = update(update(st, Event::Press(Some(Target::Step(a)), false)), Event::Release);
        let st = update(st, Event::Press(Some(Target::Step(c)), true));
        assert_eq!(names(&st), ["a", "@f( b c )"]);
    }

    #[test]
    fn commands_change_the_text_and_undo_puts_it_back() {
        let st = State::new(SRC);
        let st = update(st.clone(), Event::Press(Some(Target::Step(id(&st, "d"))), false));
        let st = update(typed(st, "drop"), Event::Key(Key::Enter, false));
        assert!(st.an.text.starts_with("go = a @f( b c )\n"), "{}", st.an.text);
        assert_eq!(st.msg.as_ref().map(|m| m.1), Some(Tone::Ok));
        let st = update(st, Event::Key(Key::Undo, false));
        assert_eq!(st.an.text, SRC);
        let st = update(typed(st, "frobnicate"), Event::Key(Key::Enter, false));
        assert_eq!(st.msg.as_ref().map(|m| m.1), Some(Tone::No));
    }

    #[test]
    fn the_data_view_walks_tables_and_runs() {
        let src = "table ts :: {n: Num}\ntable us :: {n: Num}\nroute GET /x = w\nw = +{status: 200, json: 1}\nrun \"a\"\n  GET /x → 200\nrun \"b\"\n  GET /x → 200\n";
        let st = update(typed(State::new(src), "data"), Event::Key(Key::Enter, false));
        assert_eq!(st.data, Some(Data { table: "ts".into(), run: None }));
        let st = keys(st, &[(Key::Right, false), (Key::Right, false), (Key::Right, false), (Key::Down, false)]);
        assert_eq!(st.data, Some(Data { table: "us".into(), run: Some(1) }));
        let st = keys(st, &[(Key::Left, false), (Key::Left, false), (Key::Escape, false)]);
        assert_eq!(st.data, None);
    }

    #[test]
    fn clicking_a_hole_writes_its_fill() {
        let st = State::new(SRC);
        let h = st.an.holes().next().unwrap().id;
        let st = update(st, Event::Press(Some(Target::Step(h)), false));
        assert!(matches!(st.mode, Mode::Edit(_)));
        let st = typed(st, "+{x: 1}\nsay \"hi\"");
        let st = update(st, Event::Key(Key::Enter, false));
        assert!(st.an.text.ends_with("h = +{x: 1}\n  say \"hi\"\n"), "{}", st.an.text);
        assert_eq!(st.an.holes().count(), 0);
        // a fill that doesn't parse stays open
        let h2 = State::new(SRC);
        let hid = h2.an.holes().next().unwrap().id;
        let st = typed(update(h2, Event::Press(Some(Target::Step(hid)), false)), "+{");
        let st = update(st, Event::Key(Key::Enter, false));
        assert!(matches!(st.mode, Mode::Edit(_)), "{:?}", st.msg);
    }
}

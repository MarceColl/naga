//! The editor's state and the one function that changes it. `update` takes a state and an
//! event and returns the next state; the program text inside the analysis is the only data.
use crate::analysis::{Analysis, Parent, Step};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Escape,
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

#[derive(Clone)]
pub struct State {
    pub an: Rc<Analysis>,
    pub sel: Option<Sel>,
    /// Where a shift-extended selection started.
    pub anchor: Option<usize>,
    pub hover: Option<Target>,
    pub dragging: bool,
    pub mode: Mode,
    pub msg: Option<String>,
    /// First row shown, in rows (fractional while scrolling).
    pub scroll: f32,
    /// Rows that fit on screen, and the program line of each row.
    pub rows: usize,
    pub row_lines: Rc<Vec<usize>>,
}

impl State {
    pub fn new(text: &str) -> State {
        State { an: Rc::new(Analysis::new(text)), sel: None, anchor: None, hover: None, dragging: false, mode: Mode::Idle, msg: None, scroll: 0.0, rows: 0, row_lines: Rc::default() }
    }
}

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

pub fn update(st: State, ev: Event) -> State {
    let an = st.an.clone();
    match ev {
        Event::Press(None, _) => State { sel: None, anchor: None, ..st },
        Event::Press(Some(Target::Def(name, line)), _) => State { sel: Some(Sel::Def(name, line)), anchor: None, ..st },
        Event::Press(Some(Target::Table(_)), _) => st,
        Event::Press(Some(Target::Step(id)), shift) => {
            let Some(s) = an.step(id) else { return st };
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
        Event::Scroll(dy) => State { scroll: (st.scroll + dy).clamp(0.0, st.row_lines.len() as f32), ..st },
        Event::Viewport(rows, lines) => State { rows, row_lines: Rc::new(lines), ..st },
        Event::Key(Key::Escape, _) => State { sel: None, hover: None, msg: None, ..st },
        Event::Key(k @ (Key::Left | Key::Right), shift) => reveal(step_along(st, if k == Key::Right { 1 } else { -1 }, shift)),
        Event::Key(k @ (Key::Up | Key::Down), _) => reveal(step_across(st, k == Key::Up)),
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

    const SRC: &str = "go = a @f( b c ) d\na = nop\nb = nop\nc = nop\nd = nop\n";

    fn names(st: &State) -> Vec<String> {
        match &st.sel {
            Some(Sel::Steps(ids)) => ids.iter().map(|&i| st.an.lines[st.an.step(i).unwrap().range.pos.line - 1][st.an.step(i).unwrap().range.pos.col..st.an.step(i).unwrap().range.end.col].iter().collect()).collect(),
            _ => vec![],
        }
    }

    fn keys(st: State, ks: &[(Key, bool)]) -> State {
        ks.iter().fold(st, |s, &(k, shift)| update(s, Event::Key(k, shift)))
    }

    #[test]
    fn arrows_walk_siblings_in_and_out() {
        let st = State::new(SRC);
        let st = keys(st, &[(Key::Right, false)]);
        assert_eq!(names(&st), ["a"]);
        let st = keys(st, &[(Key::Right, false)]);
        assert_eq!(names(&st), ["@f( b c )"]);
        let st = keys(st, &[(Key::Down, false)]);
        assert_eq!(names(&st), ["b"]);
        let st = keys(st, &[(Key::Right, true)]);
        assert_eq!(names(&st), ["b", "c"]);
        let st = keys(st, &[(Key::Up, false), (Key::Right, false)]);
        assert_eq!(names(&st), ["d"]);
    }

    #[test]
    fn shift_click_selects_siblings_even_from_inside() {
        let st = State::new(SRC);
        let id = |st: &State, text: &str| st.an.steps.iter().find(|s| st.an.lines[0][s.range.pos.col..s.range.end.col].iter().collect::<String>() == text).unwrap().id;
        let (a, c) = (id(&st, "a"), id(&st, "c"));
        let st = update(st, Event::Press(Some(Target::Step(a)), false));
        let st = update(update(st, Event::Release), Event::Press(Some(Target::Step(c)), true));
        assert_eq!(names(&st), ["a", "@f( b c )"]);
    }
}

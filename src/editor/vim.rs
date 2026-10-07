//! Vim over the program's text: normal, insert and visual modes, `:` commands, and the mouse.
//! A normal-mode command is read from the keys typed so far (`2dw`, `gg`, `yy`): an optional
//! count, an optional operator, then a motion or a command of its own.
use crate::ast::Pos;
use crate::editor::state::{Effect, Event, Key, Mode, State, Tone, open_data, run, start_agent};
use crate::editor::text::{self, Motion};
use crate::editor::view::Target;

pub fn event(st: State, ev: Event) -> State {
    match ev {
        Event::Press(t, at, shift) => press(st, t, at, shift),
        Event::Hover(t, at, drag) => {
            let st = State { hover: t, ..st };
            match (drag, st.drag, at) {
                (true, Some(a), Some(b)) if a != b && st.mode != Mode::Insert => {
                    let b = text::clamp(&st.an.lines, b, false);
                    State { mode: Mode::Visual(a, false), cursor: b, goal: b.col, ..st }
                }
                _ => st,
            }
        }
        Event::Release => State { drag: None, ..st },
        Event::Key(k, _) => match st.mode.clone() {
            Mode::Insert => insert(st, k),
            Mode::Cmd(c, sel) => command(st, c, sel, k),
            Mode::Normal | Mode::Visual(..) => normal(st, k),
        },
        _ => st,
    }
}

/// A click puts the cursor there (shift: selects up to there); a table's name opens its rows.
fn press(st: State, t: Option<Target>, at: Option<Pos>, shift: bool) -> State {
    if let Some(Target::Table(name)) = &t {
        return open_data(st, name);
    }
    let Some(p) = at else { return st };
    let insert = st.mode == Mode::Insert;
    let p = text::clamp(&st.an.lines, p, insert);
    let anchor = match &st.mode {
        Mode::Visual(a, _) => *a,
        _ => st.cursor,
    };
    match shift {
        true => State { mode: Mode::Visual(anchor, false), cursor: p, goal: p.col, ..st },
        false => State { mode: if insert { Mode::Insert } else { Mode::Normal }, cursor: p, goal: p.col, drag: Some(p), ..st },
    }
}

// ---------- insert ----------
fn put_text(st: State, s: &str) -> State {
    let (ls, end) = text::insert(&st.an.lines, st.cursor, s);
    let st = st.retext(&text::text(&ls), end);
    State { goal: st.cursor.col, ..st }
}

fn insert(st: State, k: Key) -> State {
    let c = st.cursor;
    match k {
        Key::Escape => {
            let cursor = text::clamp(&st.an.lines, Pos { col: c.col.saturating_sub(1), ..c }, false);
            State { mode: Mode::Normal, cursor, goal: cursor.col, ..st }
        }
        Key::Char(ch) => put_text(st, &ch.to_string()),
        Key::Tab => put_text(st, "  "),
        Key::Enter => {
            // a new line keeps the indentation; a definition's next line is indented under it
            let line = &st.an.lines[c.line - 1];
            let indent = line.iter().take_while(|c| **c == ' ').count();
            let indent = if indent == 0 && line.contains(&'=') { 2 } else { indent };
            put_text(st, &format!("\n{}", " ".repeat(indent)))
        }
        Key::Backspace => {
            let from = match c.col {
                0 if c.line == 1 => return st,
                0 => Pos { line: c.line - 1, col: st.an.lines[c.line - 2].len() },
                col => Pos { col: col - 1, ..c },
            };
            let ls = text::delete(&st.an.lines, from, c);
            st.retext(&text::text(&ls), from)
        }
        Key::Undo => undo(st),
        Key::Left | Key::Right | Key::Up | Key::Down => {
            let m = match k {
                Key::Left => Motion::Left,
                Key::Right => Motion::Right,
                Key::Up => Motion::Up,
                _ => Motion::Down,
            };
            let p = text::clamp(&st.an.lines, text::go(&st.an.lines, c, m, st.goal), true);
            let goal = if matches!(m, Motion::Up | Motion::Down) { st.goal } else { p.col };
            State { cursor: p, goal, ..st }
        }
        _ => st,
    }
}

// ---------- : ----------
fn command(st: State, mut c: String, sel: Option<(Pos, bool)>, k: Key) -> State {
    match k {
        Key::Escape => State { mode: Mode::Normal, ..st },
        Key::Enter => run(st, &c),
        Key::Backspace if c.is_empty() => State { mode: Mode::Normal, ..st },
        Key::Backspace => {
            c.pop();
            State { mode: Mode::Cmd(c, sel), ..st }
        }
        Key::Char(ch) => {
            c.push(ch);
            State { mode: Mode::Cmd(c, sel), ..st }
        }
        _ => st,
    }
}

// ---------- normal and visual ----------
fn motion(s: &str) -> Option<Motion> {
    Some(match s {
        "h" => Motion::Left,
        "l" => Motion::Right,
        "j" => Motion::Down,
        "k" => Motion::Up,
        "w" => Motion::WordNext,
        "b" => Motion::WordBack,
        "e" => Motion::WordEnd,
        "0" => Motion::Start,
        "^" => Motion::FirstNonBlank,
        "$" => Motion::End,
        "gg" => Motion::Top,
        "G" => Motion::Bottom,
        _ => return None,
    })
}

/// Keys that start a longer command: wait for the rest.
fn waits(s: &str) -> bool {
    matches!(s, "d" | "c" | "y" | "g" | "z" | "[" | "]" | "dg" | "cg" | "yg")
}

fn normal(st: State, k: Key) -> State {
    let key = match k {
        Key::Left | Key::Backspace => 'h',
        Key::Right => 'l',
        Key::Down => 'j',
        Key::Up => 'k',
        Key::Undo => 'u',
        Key::Redo => return redo(st),
        Key::Escape => return State { mode: Mode::Normal, keys: String::new(), msg: None, ..st },
        Key::Enter => return change_hole(st),
        Key::Tab | Key::Home => return st,
        Key::Char(c) => c,
    };
    let keys = format!("{}{key}", st.keys);
    let digits = keys.chars().take_while(|c| c.is_ascii_digit() && !(keys.starts_with('0'))).count();
    let (count, rest) = keys.split_at(digits);
    let n: usize = count.parse().unwrap_or(1);
    let st = State { keys: String::new(), ..st };
    let visual = matches!(st.mode, Mode::Visual(..));
    // in visual mode an operator acts on the selection at once
    if rest.is_empty() || waits(rest) && !(visual && matches!(rest, "d" | "c" | "y")) {
        return State { keys, ..st };
    }
    if matches!(rest, "G" | "gg") && !count.is_empty() {
        // a count picks the line: 45G
        let line = n.clamp(1, st.an.lines.len());
        let p = crate::editor::text::go(&st.an.lines, Pos { line, col: 0 }, Motion::FirstNonBlank, 0);
        return reveal(State { cursor: p, goal: p.col, ..st });
    }
    if let Some(m) = motion(rest) {
        return reveal(go_n(st, m, n));
    }
    let (op, m) = rest.split_at(1);
    if matches!(op, "d" | "c" | "y") && !visual {
        if m == op {
            return lines_op(st, op, n);
        }
        if let Some(mo) = motion(m) {
            return motion_op(st, op, mo, n);
        }
        return st;
    }
    match rest {
        "x" if !visual => motion_op(st, "d", Motion::Right, n),
        "x" | "d" | "y" | "c" if visual => visual_op(st, if rest == "x" { "d" } else { rest }),
        "D" => motion_op(st, "d", Motion::End, 1),
        "C" => motion_op(st, "c", Motion::End, 1),
        "p" | "P" => put(st, rest == "p"),
        "i" => State { mode: Mode::Insert, ..st.snapshot() },
        "a" => {
            let c = st.cursor;
            let col = (c.col + 1).min(st.an.lines[c.line - 1].len());
            State { mode: Mode::Insert, cursor: Pos { col, ..c }, ..st.snapshot() }
        }
        "I" => {
            let p = text::go(&st.an.lines, st.cursor, Motion::FirstNonBlank, 0);
            State { mode: Mode::Insert, cursor: p, ..st.snapshot() }
        }
        "A" => {
            let c = st.cursor;
            State { mode: Mode::Insert, cursor: Pos { col: st.an.lines[c.line - 1].len(), ..c }, ..st.snapshot() }
        }
        "o" | "O" => {
            let c = st.cursor;
            let indent = st.an.lines[c.line - 1].iter().take_while(|c| **c == ' ').count();
            let at = if rest == "o" { c.line + 1 } else { c.line };
            let ls = text::insert_lines(&st.an.lines, at, &format!("{}\n", " ".repeat(indent)));
            let st = State { mode: Mode::Insert, ..st.snapshot() };
            st.retext(&text::text(&ls), Pos { line: at, col: indent })
        }
        "v" | "V" => match st.mode {
            Mode::Visual(_, l) if l == (rest == "V") => State { mode: Mode::Normal, ..st },
            Mode::Visual(a, _) => State { mode: Mode::Visual(a, rest == "V"), ..st },
            _ => State { mode: Mode::Visual(st.cursor, rest == "V"), ..st },
        },
        "u" => undo(st),
        ":" => {
            let sel = match st.mode {
                Mode::Visual(a, l) => Some((a, l)),
                _ => None,
            };
            State { mode: Mode::Cmd(String::new(), sel), ..st }
        }
        "ga" => match hole_on_line(&st) {
            Some(h) => start_agent(st.clone(), h, &st.an.step(h).map(|s| s.word.to_string()).unwrap_or_default(), st.an.clone(), None),
            None => st.say("no hole on this line", Tone::No),
        },
        "ZZ" | "ZQ" => State { effect: Some(Effect::Quit), ..st },
        _ => st,
    }
}

fn reveal(st: State) -> State {
    let line = Some(st.cursor.line);
    crate::editor::state::reveal_line(st, line)
}

/// Moves the cursor by `m`, `n` times.
fn go_n(st: State, m: Motion, n: usize) -> State {
    let ls = &st.an.lines;
    let mut p = st.cursor;
    for _ in 0..n {
        p = text::go(ls, p, m, st.goal);
    }
    let p = text::clamp(ls, p, false);
    let goal = if matches!(m, Motion::Up | Motion::Down) { st.goal } else { p.col };
    State { cursor: p, goal, ..st }
}

/// `d`, `c` or `y` over `n` whole lines from the cursor's.
fn lines_op(st: State, op: &str, n: usize) -> State {
    let a = st.cursor.line;
    let b = (a + n - 1).min(st.an.lines.len());
    line_range_op(st, op, a, b)
}

fn line_range_op(st: State, op: &str, a: usize, b: usize) -> State {
    let taken = text::line_text(&st.an.lines, a, b);
    let st = State { reg: Some((taken, true)), mode: Mode::Normal, ..st };
    match op {
        "y" => st,
        "d" => {
            let ls = text::delete_lines(&st.an.lines, a, b);
            st.snapshot().retext(&text::text(&ls), Pos { line: a, col: 0 })
        }
        _ => {
            let indent = st.an.lines[a - 1].iter().take_while(|c| **c == ' ').count();
            let ls = text::insert_lines(&text::delete_lines(&st.an.lines, a, b), a, &format!("{}\n", " ".repeat(indent)));
            State { mode: Mode::Insert, ..st.snapshot() }.retext(&text::text(&ls), Pos { line: a, col: indent })
        }
    }
}

/// `d`, `c` or `y` from the cursor to where `m` (`n` times) goes.
fn motion_op(st: State, op: &str, m: Motion, n: usize) -> State {
    let ls = &st.an.lines;
    let c = st.cursor;
    let mut t = c;
    // `cw` changes to the end of the word, as `ce` does
    let m = if op == "c" && m == Motion::WordNext { Motion::WordEnd } else { m };
    for _ in 0..n {
        t = text::go(ls, t, m, st.goal);
    }
    if m.linewise() {
        let (a, b) = (c.line.min(t.line), c.line.max(t.line));
        return line_range_op(st, op, a, b);
    }
    // a word motion stops at the end of the line
    if m == Motion::WordNext && t.line > c.line {
        t = Pos { line: c.line, col: ls[c.line - 1].len() };
    }
    let (a, mut b) = text::order(c, t);
    if m.inclusive() || m == Motion::Right && op == "d" && n > 0 && b == a {
        b = Pos { col: (b.col + 1).min(ls[b.line - 1].len()), ..b };
    }
    if m.inclusive() {
        b = Pos { col: b.col.max(a.col), ..b };
    }
    charwise_op(st, op, a, b)
}

fn charwise_op(st: State, op: &str, a: Pos, b: Pos) -> State {
    let taken = text::slice(&st.an.lines, a, b);
    let st = State { reg: Some((taken, false)), mode: Mode::Normal, ..st };
    match op {
        "y" => State { cursor: a, ..st },
        _ => {
            let ls = text::delete(&st.an.lines, a, b);
            let mode = if op == "c" { Mode::Insert } else { Mode::Normal };
            State { mode, ..st.snapshot() }.retext(&text::text(&ls), a)
        }
    }
}

fn visual_op(st: State, op: &str) -> State {
    let Some((a, b)) = st.visual() else { return st };
    match st.mode {
        Mode::Visual(_, true) => line_range_op(st, op, a.line, b.line),
        _ => charwise_op(st, op, a, b),
    }
}

/// `p` after the cursor (below, for whole lines), `P` before it.
fn put(st: State, after: bool) -> State {
    let Some((s, lines)) = st.reg.clone() else { return st };
    let c = st.cursor;
    let st = st.snapshot();
    match lines {
        true => {
            let at = if after { c.line + 1 } else { c.line };
            let ls = text::insert_lines(&st.an.lines, at, &s);
            let p = text::go(&ls, Pos { line: at, col: 0 }, Motion::FirstNonBlank, 0);
            st.retext(&text::text(&ls), p)
        }
        false => {
            let at = if after { Pos { col: (c.col + 1).min(st.an.lines[c.line - 1].len()), ..c } } else { c };
            let (ls, end) = text::insert(&st.an.lines, at, &s);
            st.retext(&text::text(&ls), Pos { col: end.col.saturating_sub(1), ..end })
        }
    }
}

fn hole_on_line(st: &State) -> Option<usize> {
    st.an.holes().find(|h| h.range.pos.line == st.cursor.line).map(|h| h.id)
}

/// Enter on a hole's line: the hole goes, and you type its fill where it was.
fn change_hole(st: State) -> State {
    let Some(h) = hole_on_line(&st).and_then(|h| st.an.step(h).cloned()) else { return st };
    let a = h.range.pos;
    let b = Pos { col: st.an.lines[a.line - 1].len(), ..a };
    let ls = text::delete(&st.an.lines, a, b);
    State { mode: Mode::Insert, ..st.snapshot() }.retext(&text::text(&ls), a).say("write the fill · esc when done", Tone::Cmd)
}

fn undo(st: State) -> State {
    let mut undo = st.undo.clone();
    let Some((t, c)) = undo.pop() else { return st.say("nothing to undo", Tone::Plain) };
    let mut redo = st.redo.clone();
    redo.push((st.an.text.clone(), st.cursor));
    State { undo, redo, mode: Mode::Normal, ..st }.retext(&t, c)
}

fn redo(st: State) -> State {
    let mut redo = st.redo.clone();
    let Some((t, c)) = redo.pop() else { return st.say("nothing to redo", Tone::Plain) };
    let mut undo = st.undo.clone();
    undo.push((st.an.text.clone(), st.cursor));
    State { undo, redo, mode: Mode::Normal, ..st }.retext(&t, c)
}

#[cfg(test)]
mod tests {
    use crate::editor::state::State;
    use crate::editor::state::tests::keys;

    fn text(st: &State) -> &str {
        &st.an.text
    }

    #[test]
    fn typing_and_motions() {
        let st = keys(State::new("go = a b\n"), "wwwiz <esc>");
        assert_eq!(text(&st), "go = a z b\n");
        let st = keys(st, "0dwx");
        assert_eq!(text(&st), " a z b\n");
        let st = keys(st, "u");
        assert_eq!(text(&st), "= a z b\n");
        let st = keys(st, "uu");
        assert_eq!(text(&st), "go = a b\n");
        let st = keys(st, "Ax <esc>yyp");
        assert_eq!(text(&st), "go = a bx \ngo = a bx \n");
        let st = keys(st, "ggddjdd");
        assert_eq!(text(&st), "go = a bx ");
        let st = keys(st, "$cbq<esc>");
        assert_eq!(text(&st), "go = a q ");
    }

    #[test]
    fn insert_keeps_indentation_and_counts_repeat() {
        let st = keys(State::new("go = a\n"), "A<cr>b<esc>");
        assert_eq!(text(&st), "go = a\n  b\n");
        let st = keys(State::new("a b c d\n"), "2dw");
        assert_eq!(text(&st), "c d\n");
        let st = keys(State::new("x\ny\nz\n"), "2dd");
        assert_eq!(text(&st), "z\n");
        let st = keys(State::new("ab cd"), "veyP");
        assert_eq!(text(&st), "abab cd");
    }

    #[test]
    fn enter_on_a_hole_writes_its_fill_and_the_image_follows_when_it_parses() {
        let src = "h = ? write me\n";
        let st = keys(State::new(src), "<cr>+{x: ");
        assert!(!st.an.program.errors.is_empty(), "half typed");
        assert_eq!(st.image.text, "h = \n", "the image stays at the last text that parsed");
        let st = keys(st, "1}<esc>");
        assert_eq!(text(&st), "h = +{x: 1}\n");
        assert_eq!(st.image.text, "h = +{x: 1}\n");
    }
}

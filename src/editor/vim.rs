//! Vim over the program's text: normal, insert and visual modes, `:` commands, and the mouse.
//! A normal-mode command is read from the keys typed so far (`2dw`, `gg`, `yy`): an optional
//! count, an optional operator, then a motion or a command of its own.
use crate::ast::Pos;
use crate::editor::state::{Effect, Event, Key, Mode, State, Tone, open_data, run, start_agent};
use crate::editor::text::{self, Motion};
use crate::editor::outline::{self, Fold};
use crate::editor::view::Target;
use std::rc::Rc;

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
        Event::Key(k, shift) => match st.mode.clone() {
            Mode::Insert => insert(st, k),
            Mode::Cmd(c, sel) => command(st, c, sel, k),
            Mode::Normal | Mode::Visual(..) => normal(st, k, shift),
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
    aligned(st, ls, end)
}

/// The text after an insert-mode edit, with the `=` of the definitions around the cursor
/// lined up as you type; the cursor moves with its text.
fn aligned(st: State, ls: text::Lines, c: Pos) -> State {
    let (out, moved) = text::align(&ls, c.line);
    let after_eq = text::head_eq_col(&ls[c.line - 1]).is_some_and(|eq| c.col > eq);
    let c = Pos { col: (c.col as isize + if after_eq { moved } else { 0 }).max(0) as usize, ..c };
    let st = st.retext(&text::text(&out), c);
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
            aligned(st, ls, from)
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
    matches!(s, "d" | "c" | "y" | "g" | "z" | "[" | "]" | "dg" | "cg" | "yg" | "Z")
}

/// Changes the folds; the cursor moves out of whatever closed over it.
fn fold(st: State, f: impl Fn(&crate::editor::analysis::Analysis, &outline::Folds, usize) -> outline::Folds) -> State {
    let folds = Rc::new(f(&st.an, &st.folds, st.cursor.line));
    let st = State { folds, ..st };
    let line = visible_line(&st, st.cursor.line, -1);
    let cursor = if line == st.cursor.line { st.cursor } else { Pos { line, col: 0 } };
    reveal(State { cursor, ..st })
}

/// `line`, or the nearest shown line from it in direction `dir` (falling back to the other way).
fn visible_line(st: &State, line: usize, dir: isize) -> usize {
    let hidden = outline::hidden(&st.an, &st.folds);
    let shown = |l: usize| !hidden.get(l - 1).copied().unwrap_or(false);
    let walk = |d: isize| {
        let mut l = line as isize;
        while l >= 1 && (l as usize) <= hidden.len() {
            if shown(l as usize) {
                return Some(l as usize);
            }
            l += d;
        }
        None
    };
    walk(dir).or_else(|| walk(-dir)).unwrap_or(line)
}

fn normal(st: State, k: Key, shift: bool) -> State {
    let key = match k {
        Key::Left | Key::Backspace => 'h',
        Key::Right => 'l',
        Key::Down => 'j',
        Key::Up => 'k',
        Key::Undo => 'u',
        Key::Redo => return redo(st),
        Key::Escape => return State { mode: Mode::Normal, keys: String::new(), msg: None, ..st },
        Key::Enter => return change_hole(st),
        Key::Tab if shift => return fold(State { keys: String::new(), ..st }, |an, f, _| outline::cycle_all(an, f)),
        Key::Tab => return fold(State { keys: String::new(), ..st }, |an, f, line| outline::cycle(an, f, line).unwrap_or_else(|| f.clone())),
        Key::Home => return st,
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
        "?" => State { help: Some(0), ..st },
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
        "za" => fold(st, |an, f, line| {
            let mut f = f.clone();
            if let Some(h) = outline::headings(an).into_iter().find(|h| h.line == line) {
                let now = f.get(&h.text).copied().unwrap_or_default();
                f.insert(h.text, if now == Fold::Open { Fold::Folded } else { Fold::Open });
            }
            f
        }),
        "zM" => fold(st, |an, _, _| outline::all(an, Fold::Folded)),
        "zR" => fold(st, |_, _, _| outline::Folds::new()),
        "]e" | "[e" => {
            // the next (or previous) error, wrapping, with the folds around it opened
            let c = st.cursor;
            let mut es: Vec<(usize, usize, String)> = st.an.errors.iter().map(|e| (e.line, e.col, e.msg.clone())).collect();
            es.sort();
            let after = |e: &&(usize, usize, String)| (e.0, e.1) > (c.line, c.col);
            let to = match rest {
                "]e" => es.iter().find(after).or(es.first()),
                _ => es.iter().rev().find(|e| (e.0, e.1) < (c.line, c.col)).or(es.last()),
            };
            let Some((line, col, msg)) = to.cloned() else { return st.say("no errors", Tone::Ok) };
            let folds: outline::Folds = st.folds.iter().filter(|(h, _)| {
                let hs = outline::headings(&st.an);
                hs.iter().position(|x| &x.text == *h).is_none_or(|i| !(hs[i].line < line && line <= outline::end(&st.an, &hs, i)))
            }).map(|(h, f)| (h.clone(), *f)).collect();
            let p = text::clamp(&st.an.lines, Pos { line, col }, false);
            reveal(State { cursor: p, goal: p.col, folds: Rc::new(folds), ..st }).say(msg, Tone::No)
        }
        "[[" | "]]" => {
            let line = st.cursor.line;
            let hs = outline::headings(&st.an);
            let hidden = outline::hidden(&st.an, &st.folds);
            let shown = |h: &&outline::Heading| !hidden[h.line - 1];
            let to = match rest {
                "]]" => hs.iter().filter(shown).find(|h| h.line > line),
                _ => hs.iter().rev().filter(shown).find(|h| h.line < line),
            };
            match to {
                Some(h) => reveal(State { cursor: Pos { line: h.line, col: 0 }, goal: 0, ..st }),
                None => st,
            }
        }
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
    // a fold is one line to move over; landing inside one lands on its heading
    let dir = if m == Motion::Down { 1 } else { -1 };
    let line = visible_line(&st, p.line, dir);
    let p = if line == p.line { p } else { text::clamp(ls, Pos { line, col: goal }, false) };
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
    fn folds_open_and_close_and_the_cursor_steps_over_them() {
        use crate::editor::state::{Event, Key, update};
        let src = "* A\na = nop\nb = nop\n* B\nc = nop\n";
        let tab = |st: State, shift| update(st, Event::Key(Key::Tab, shift));
        let st = tab(State::new(src), false);
        let st = keys(st, "j");
        assert_eq!(st.cursor.line, 4, "j steps over the folded section");
        let st = keys(st, "k");
        assert_eq!(st.cursor.line, 1);
        let st = keys(tab(st, false), "jj");
        assert_eq!(st.cursor.line, 3, "open again");
        let st = keys(st, "zM");
        assert_eq!(st.cursor.line, 1, "folding moves the cursor out to the heading");
        let st = keys(st, "]]");
        assert_eq!(st.cursor.line, 4);
        let st = keys(st, "zRj");
        assert_eq!(st.cursor.line, 5);
        let st = tab(st, true);
        assert_eq!(st.cursor.line, 4, "overview");
    }

    #[test]
    fn errors_are_one_jump_away_even_inside_folds() {
        use crate::editor::state::{Event, Key, update};
        let src = "* A\ngo = a\na = +{x: 1} +{x: 2}\n* B\nb = nop\n";
        let st = update(State::new(src), Event::Key(Key::Tab, false));
        let st = keys(st, "]e");
        assert_eq!((st.cursor.line, st.cursor.col), (3, 12));
        assert!(st.msg.as_ref().is_some_and(|m| m.0.contains("x")), "{:?}", st.msg);
        assert!(st.folds.is_empty() || st.folds.values().all(|f| *f == crate::editor::outline::Fold::Open), "the fold around it opens");
    }

    #[test]
    fn typing_lines_up_the_definitions_as_it_goes() {
        let st = keys(State::new("found {-missing} = a\nfound {n} = b\n"), "jA c");
        assert_eq!(text(&st), "found {-missing} = a\nfound {n}        = b c\n", "aligned before esc");
        assert_eq!((st.cursor.line, st.cursor.col), (2, 22), "the cursor stays after what it typed");
        // a longer head pushes the others out, keystroke by keystroke
        let st = keys(st, "<esc>08liXX");
        assert_eq!(text(&st), "found {-missing} = a\nfound {nXX}      = b c\n");
        let st = keys(st, "XXXXXX");
        assert_eq!(text(&st), "found {-missing}  = a\nfound {nXXXXXXXX} = b c\n");
        let st = keys(st, "<bs><bs>");
        assert_eq!(text(&st), "found {-missing} = a\nfound {nXXXXXX}  = b c\n");
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

//! The editor's state and the one function that changes it. `update` takes a state and an
//! event and returns the next state. The buffer's text is the only data: the analysis is
//! recomputed from it on every change, and `image` — what the server runs and what's on
//! disk — follows it whenever it parses.
use crate::ast::Pos;
use crate::check::Checker;
use crate::editor::agent::{self, Agent};
use crate::editor::analysis::{Analysis, Parent, Range, Step};
use crate::editor::claude::Fill;
use crate::editor::debug::{self, Debug, Move};
use crate::editor::ops;
use crate::editor::text;
use crate::editor::view::Target;
use crate::editor::vim;
use crate::value::Name;
use std::cell::RefCell;
use std::rc::Rc;

/// Text typed into a hole by the agent, shown in place while it arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range,
    pub text: Vec<char>,
    pub cursor: usize,
    pub rework: bool,
    pub hole: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// Selecting from the anchor to the cursor; `true` for whole lines.
    Visual(Pos, bool),
    /// A `:` command being typed, with the visual selection it applies to, if any.
    Cmd(String, Option<(Pos, bool)>),
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
    Tab,
    Char(char),
    Undo,
    Redo,
    Home,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Drop the table, then create and seed it again.
    Reset(Name),
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Ok,
    No,
    Cmd,
    Agent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The mouse went down: on this target, at this place in the program.
    Press(Option<Target>, Option<Pos>, bool),
    /// The mouse moved (`drag`: with the button down).
    Hover(Option<Target>, Option<Pos>, bool),
    Release,
    Key(Key, bool),
    /// Lines to scroll by.
    Scroll(f32),
    /// How many code rows fit, and which program line each displayed row shows.
    Viewport(usize, Vec<usize>),
    /// Claude's answer to the agent with this id.
    AgentReply(u64, Result<Fill, String>),
    /// Time passes: the agent types a little more of its fill.
    Tick,
    /// The live server answered a request: `GET /accounts/1 → 200`.
    Served(String),
}

/// Which rows the data view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rows {
    /// The live database's, now.
    Live,
    Seed,
    /// After run n.
    Run(usize),
}

/// A table's rows: live, as seeded, or after a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data {
    pub table: Name,
    pub rows: Rows,
}

#[derive(Clone)]
pub struct State {
    /// The buffer, analysed.
    pub an: Rc<Analysis>,
    /// The last text that parsed: what the server runs and what the file holds.
    pub image: Rc<Analysis>,
    pub checker: Rc<RefCell<Checker>>,
    pub cursor: Pos,
    /// The column up and down aim for.
    pub goal: usize,
    pub mode: Mode,
    /// Keys of a normal-mode command still being typed: a count, `d`, `g`.
    pub keys: String,
    /// Which sections are folded, by heading.
    pub folds: Rc<crate::editor::outline::Folds>,
    /// The text `y`, `d` and `x` took, and whether it is whole lines.
    pub reg: Option<(String, bool)>,
    /// Earlier and undone (text, cursor) pairs.
    pub undo: Vec<(String, Pos)>,
    pub redo: Vec<(String, Pos)>,
    pub data: Option<Data>,
    pub debug: Option<Debug>,
    /// The cheat sheet, scrolled to this line.
    pub help: Option<usize>,
    pub agent: Option<Agent>,
    /// A question for Claude, waiting to be sent: the agent's id and the prompt.
    pub ask: Option<(u64, String)>,
    pub agents: u64,
    /// The live server's port and the last request it answered.
    pub live: Option<(u16, Option<String>)>,
    /// The live database's tables, as of the last look (kept fresh while the data view is open).
    pub live_tables: Option<Rc<crate::runs::Tables>>,
    /// Table changes the live database can't take without losing data.
    pub pending: Vec<String>,
    /// Something only the app can do, waiting to be done.
    pub effect: Option<Effect>,
    pub hover: Option<Target>,
    /// Where a mouse drag started.
    pub drag: Option<Pos>,
    pub msg: Option<(String, Tone)>,
    /// First row shown, in rows (fractional while scrolling).
    pub scroll: f32,
    /// Rows that fit on screen, and the program line of each row.
    pub rows: usize,
    pub row_lines: Rc<Vec<usize>>,
}

pub const OPS: [&str; 14] = ["extract name", "inline", "hole note", "rework", "drop", "swap", "rename name", "fix", "data", "debug 1–9", "reset table", "help", "w", "q"];

impl State {
    pub fn new(text: &str) -> State {
        let checker = Rc::new(RefCell::new(Checker::default()));
        let an = Rc::new(Analysis::with(text, &mut checker.borrow_mut()));
        State {
            image: an.clone(),
            an,
            checker,
            cursor: Pos { line: 1, col: 0 },
            goal: 0,
            mode: Mode::Normal,
            keys: String::new(),
            folds: Rc::default(),
            reg: None,
            undo: vec![],
            redo: vec![],
            data: None,
            debug: None,
            help: None,
            agent: None,
            ask: None,
            agents: 0,
            live: None,
            pending: vec![],
            live_tables: None,
            effect: None,
            hover: None,
            drag: None,
            msg: None,
            scroll: 0.0,
            rows: 0,
            row_lines: Rc::default(),
        }
    }

    pub fn say(self, text: impl Into<String>, tone: Tone) -> State {
        State { msg: Some((text.into(), tone)), ..self }
    }

    /// Keeps the current text for undo, before an edit.
    pub fn snapshot(self) -> State {
        let mut undo = self.undo.clone();
        undo.push((self.an.text.clone(), self.cursor));
        State { undo, redo: vec![], ..self }
    }

    /// The buffer becomes `text`, the cursor `cursor`. The analysis follows at once; the image
    /// only when the text parses.
    pub fn retext(self, text: &str, cursor: Pos) -> State {
        let an = Rc::new(Analysis::with(text, &mut self.checker.borrow_mut()));
        let image = if an.program.errors.is_empty() { an.clone() } else { self.image.clone() };
        let insert = self.mode == Mode::Insert;
        let cursor = text::clamp(&an.lines, cursor, insert);
        State { an, image, cursor, ..self }
    }

    /// The innermost step under the cursor.
    pub fn step_at_cursor(&self) -> Option<&Step> {
        let c = self.cursor;
        let id = *self.an.cells.get(c.line - 1)?.get(c.col)?;
        self.an.step(id)
    }

    /// The visual selection, start to end, end excluded; whole lines when linewise.
    pub fn visual(&self) -> Option<(Pos, Pos)> {
        let (a, lines) = match &self.mode {
            Mode::Visual(a, l) | Mode::Cmd(_, Some((a, l))) => (*a, *l),
            _ => return None,
        };
        let (x, y) = text::order(a, self.cursor);
        Some(match lines {
            true => (Pos { line: x.line, col: 0 }, Pos { line: y.line, col: self.an.lines[y.line - 1].len() }),
            false => (x, Pos { col: (y.col + 1).min(self.an.lines[y.line - 1].len()), ..y }),
        })
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

/// A whole-program change from a command or the agent: kept for undo, said in the bar.
fn commit(st: State, text: &str, ok: &str) -> State {
    let cursor = st.cursor;
    let st = State { mode: Mode::Normal, ..st.snapshot() }.retext(text, cursor);
    let first = st.an.errors.first().map(|e| format!(" — {}", e.msg)).unwrap_or_default();
    let msg = format!("{}{}{first}", if ok.is_empty() { String::new() } else { format!("{ok} · ") }, status(&st.an));
    let tone = if st.an.errors.is_empty() { Tone::Ok } else { Tone::No };
    st.say(msg, tone)
}

fn commit_or_say(st: State, text: ops::R<String>, ok: &str) -> State {
    match text {
        Err(why) => st.say(why, Tone::No),
        Ok(text) if !crate::syntax::parse(&text).errors.is_empty() => st.say("that wouldn’t parse", Tone::No),
        Ok(text) => commit(st, &text, ok),
    }
}

/// Scrolls just enough to show `line`.
pub fn reveal_line(st: State, line: Option<usize>) -> State {
    let Some(row) = line.and_then(|l| st.row_lines.iter().position(|&x| x == l)) else { return st };
    let (top, rows) = (st.scroll.floor() as usize, st.rows.max(1));
    let scroll = match row {
        r if r < top => r as f32,
        r if r + 1 >= top + rows => (r + 2).saturating_sub(rows) as f32,
        _ => st.scroll,
    };
    State { scroll, ..st }
}

pub fn update(st: State, ev: Event) -> State {
    if let Some(a) = st.agent.clone() {
        return agent_event(st, a, ev);
    }
    match ev {
        Event::AgentReply(..) | Event::Tick => st,
        Event::Served(line) => State { live: st.live.map(|(port, _)| (port, Some(line))), ..st },
        Event::Viewport(rows, lines) => State { rows, row_lines: Rc::new(lines), ..st },
        Event::Scroll(dy) => State { scroll: (st.scroll + dy).clamp(0.0, st.row_lines.len() as f32), ..st },
        Event::Key(k, _) if st.help.is_some() => {
            let at = st.help.unwrap_or(0);
            let last = crate::editor::help::TEXT.lines().count().saturating_sub(st.rows.max(1) / 2);
            let page = st.rows.max(2) / 2;
            let at = match k {
                Key::Escape | Key::Char('q') | Key::Char('?') => return State { help: None, ..st },
                Key::Down | Key::Char('j') | Key::Enter => at + 1,
                Key::Up | Key::Char('k') => at.saturating_sub(1),
                Key::Char('d') | Key::Char(' ') => at + page,
                Key::Char('u') => at.saturating_sub(page),
                Key::Char('g') | Key::Home => 0,
                Key::Char('G') => last,
                _ => at,
            };
            State { help: Some(at.min(last)), ..st }
        }
        ev if st.help.is_some() => match ev {
            Event::Press(..) => State { help: None, ..st },
            _ => st,
        },
        ev if st.debug.is_some() => {
            let d = st.debug.clone().unwrap();
            debugging(st, d, ev)
        }
        ev if st.data.is_some() => {
            let d = st.data.clone().unwrap();
            data(st, d, ev)
        }
        ev => vim::event(st, ev),
    }
}

// ---------- the data view ----------
/// Opens the data view on `table`, keeping the run it showed last.
pub fn open_data(st: State, table: &str) -> State {
    // names ignore case: `houseS` in the text is the table houses
    let table = table.to_lowercase();
    let table = table.as_str();
    if !st.an.program.tables.contains_key(table) {
        return st.say(format!("there's no table called {table}"), Tone::No);
    }
    let first = if st.live.is_some() { Rows::Live } else { Rows::Seed };
    let rows = st.data.as_ref().map_or(first, |d| d.rows);
    State { data: Some(Data { table: table.into(), rows }), hover: None, ..st }
}

fn data(st: State, d: Data, ev: Event) -> State {
    let tables: Vec<Name> = st.an.program.tables.keys().cloned().collect();
    let runs = st.an.runs.len();
    let at = tables.iter().position(|t| *t == d.table).unwrap_or(0);
    let k = match ev {
        Event::Key(Key::Char('h'), _) => Key::Left,
        Event::Key(Key::Char('l'), _) => Key::Right,
        Event::Key(Key::Char('j'), _) => Key::Down,
        Event::Key(Key::Char('k'), _) => Key::Up,
        Event::Key(Key::Char('q'), _) => Key::Escape,
        Event::Key(k, _) => k,
        Event::Press(Some(Target::Table(t)), ..) => return State { data: Some(Data { table: t, ..d }), ..st },
        Event::Press(Some(Target::Rows(r)), ..) => return State { data: Some(Data { rows: r, ..d }), ..st },
        Event::Hover(t, ..) => return State { hover: t, ..st },
        _ => return st,
    };
    // the choices, left to right: live (when serving), seed, then each run
    let choices: Vec<Rows> = st.live.iter().map(|_| Rows::Live).chain([Rows::Seed]).chain((0..runs).map(Rows::Run)).collect();
    let i = choices.iter().position(|r| *r == d.rows).unwrap_or(0);
    let d = match k {
        Key::Escape => return State { data: None, ..st },
        Key::Right => Data { rows: choices[(i + 1).min(choices.len() - 1)], ..d },
        Key::Left => Data { rows: choices[i.saturating_sub(1)], ..d },
        Key::Down => Data { table: tables[(at + 1) % tables.len()].clone(), ..d },
        Key::Up => Data { table: tables[(at + tables.len() - 1) % tables.len()].clone(), ..d },
        _ => d,
    };
    State { data: Some(d), ..st }
}

// ---------- the debugger ----------
/// Records run `i` and starts stepping it, keeping the breakpoints.
fn start_debug(st: State, i: usize) -> State {
    let bps = st.debug.as_ref().map(|d| d.bps.clone()).unwrap_or_default();
    match debug::record(&st.image, i, bps) {
        Ok(d) => State { debug: Some(d), hover: None, data: None, ..st },
        Err(e) => st.say(e, Tone::No),
    }
}

fn debugging(st: State, mut d: Debug, ev: Event) -> State {
    let mv = match ev {
        Event::Key(Key::Escape | Key::Char('q'), _) => return State { debug: None, ..st },
        Event::Key(Key::Char(c), _) if c.is_ascii_digit() && c != '0' => return start_debug(st, c as usize - '1' as usize),
        Event::Key(Key::Right | Key::Char('l'), _) => Move::Into,
        Event::Key(Key::Left | Key::Char('h'), _) => Move::Back,
        Event::Key(Key::Down | Key::Char('j'), _) => Move::Over,
        Event::Key(Key::Up | Key::Char('k'), _) => Move::Out,
        Event::Key(Key::Enter, _) => Move::Continue,
        Event::Key(Key::Home | Key::Char('0'), _) => Move::Restart,
        Event::Press(t, at, _) => {
            let line = match t {
                Some(Target::Step(id)) => st.image.step(id).map(|s| s.range.pos.line),
                _ => at.map(|p| p.line),
            };
            if let Some(l) = line {
                match d.bps.iter().position(|&b| b == l) {
                    Some(i) => drop(d.bps.remove(i)),
                    None => d.bps.push(l),
                }
            }
            return State { debug: Some(d), ..st };
        }
        _ => return st,
    };
    d.at = d.go(mv);
    let line = d.current().map(|r| r.pos.line);
    reveal_line(State { debug: Some(d), ..st }, line)
}

// ---------- the agent ----------
/// Hands the hole to Claude. `base` is what its fills are judged against; `original` is the
/// code a rework replaced.
pub fn start_agent(st: State, hole: usize, label: &str, base: Rc<Analysis>, original: Option<String>) -> State {
    let Some(h) = st.an.step(hole).cloned() else { return st };
    let id = st.agents + 1;
    let a = Agent { id, hole, label: label.into(), tries: 0, fill: None, shown: 0, base, original, rejected: None };
    let ask = Some((id, agent::prompt(&st.an, &h, &a)));
    State { agent: Some(a), ask, agents: id, mode: Mode::Normal, hover: None, ..st }.say(format!("agent › {label} · try 1 · asking Claude · esc stops it"), Tone::Agent)
}

fn agent_event(st: State, mut a: Agent, ev: Event) -> State {
    let (label, n) = (a.label.clone(), a.tries);
    match ev {
        Event::Key(Key::Escape, _) => State { agent: None, ..st }.say("agent stopped — the hole is still open", Tone::Plain),
        Event::AgentReply(id, _) if id != a.id => st,
        Event::AgentReply(_, Err(e)) => give_up(st, a, &e),
        Event::AgentReply(_, Ok(fill)) => {
            let why = fill.why.clone();
            a.tries += 1;
            a.fill = Some(fill);
            a.shown = 0;
            State { agent: Some(a), ..st }.say(format!("agent › {label} · try {} · writing · {why}", n + 1), Tone::Agent)
        }
        Event::Tick => {
            let Some(fill) = a.fill.clone() else { return State { agent: Some(a), ..st } };
            let len = fill.text().chars().count();
            if a.shown < len {
                a.shown = (a.shown + 3).min(len);
                return State { agent: Some(a), ..st };
            }
            let Some(h) = st.an.step(a.hole).cloned() else { return give_up(st, a, "the hole moved") };
            let text = ops::fill(&st.an, &h, &fill.code, fill.helpers.as_deref());
            match agent::judge(&a.base, &Analysis::new(&text)) {
                Ok(()) => commit(State { agent: None, ..st }, &text, &format!("agent › {label} · try {n} ✓")),
                Err(why) if n < agent::TRIES => {
                    a.rejected = Some((fill, why.clone()));
                    a.fill = None;
                    let ask = Some((a.id, agent::prompt(&st.an, &h, &a)));
                    State { agent: Some(a), ask, ..st }.say(format!("agent › {label} · try {n} ✗ {why} · asking again"), Tone::No)
                }
                Err(why) => give_up(st, a, &why),
            }
        }
        Event::Viewport(..) | Event::Scroll(_) | Event::Hover(..) | Event::Served(_) => {
            let st = update(State { agent: None, ..st }, ev);
            State { agent: Some(a), ..st }
        }
        _ => State { agent: Some(a), ..st },
    }
}

/// The agent stops: a rework puts the original back, a hole stays open.
fn give_up(st: State, a: Agent, why: &str) -> State {
    let st = State { agent: None, ..st };
    match (&a.original, st.an.step(a.hole).cloned()) {
        (Some(o), Some(h)) => {
            let text = ops::fill(&st.an, &h, o, None);
            commit(st, &text, &format!("rework: nothing better ({why}) — put the original back"))
        }
        _ => st.say(format!("agent › {} · gave up after {} tries — the hole is yours: {why}", a.label, a.tries), Tone::No),
    }
}

// ---------- commands ----------
/// The steps a command works on: the outermost ones inside the visual selection (siblings of
/// the first), or the step under the cursor.
fn chosen(st: &State) -> Option<Vec<usize>> {
    let an = &st.an;
    if let Some((a, b)) = st.visual() {
        let within = |s: &&Step| (s.range.pos.line, s.range.pos.col) >= (a.line, a.col) && (s.range.end.line, s.range.end.col) <= (b.line, b.col);
        let inside: Vec<&Step> = an.steps.iter().filter(within).collect();
        let outer: Vec<&Step> = inside.iter().copied().filter(|s| !matches!(s.parent, Parent::Step(p, _) if inside.iter().any(|o| o.id == p))).collect();
        if let Some(first) = outer.first() {
            let mut ids: Vec<&Step> = outer.iter().copied().filter(|s| s.parent == first.parent).collect();
            ids.sort_by_key(|s| s.idx);
            return Some(ids.iter().map(|s| s.id).collect());
        }
    }
    st.step_at_cursor().map(|s| vec![s.id])
}

/// The word named under the cursor: a definition's head, or a call.
fn word_at_cursor(st: &State) -> Option<Name> {
    let c = st.cursor;
    if let Some(d) = st.an.def_at(c.line).filter(|d| c.col < d.name.chars().count()) {
        return Some(d.name.clone());
    }
    st.step_at_cursor().and_then(|s| s.call().cloned())
}

/// A `:` command on what's chosen: `extract name`, `inline`, `hole note`, …
pub fn run(st: State, raw: &str) -> State {
    let mut words = raw.split_whitespace();
    let op = words.next().unwrap_or("").to_lowercase();
    let arg = words.collect::<Vec<_>>().join(" ");
    let an = st.an.clone();
    let parses = an.program.errors.is_empty();
    let ids = chosen(&st);
    let call = ids.as_ref().filter(|i| i.len() == 1).and_then(|i| an.step(i[0])).and_then(|s| s.call().cloned());
    let st = State { mode: Mode::Normal, ..st };
    let edits = ["extract", "factor", "inline", "drop", "swap", "rename", "hole", "punch", "rework"];
    if edits.contains(&op.as_str()) && !parses {
        return st.say("the program doesn’t parse yet — fix it first", Tone::No);
    }
    let need = |what: &str| format!("put the cursor on {what}, or select some");
    match op.as_str() {
        "help" | "h" => State { help: Some(0), ..st },
        "fix" => {
            // the fix an error on this line proposes: a clause, put before the word's first one,
            // since the first clause that matches is the one that runs
            let fix = an.errors.iter().filter(|e| e.line == st.cursor.line).find_map(|e| e.detail.as_ref()?.fix.clone());
            let Some(fix) = fix else { return st.say("no fix to add on this line", Tone::No) };
            let word = fix.split_whitespace().next().unwrap_or("").to_string();
            let Some(first) = an.program.words.get(word.as_str()).and_then(|w| w.clauses.first()).map(|c| c.chunk_line) else {
                return st.say(format!("{word} isn’t defined"), Tone::No);
            };
            let ls = text::insert_lines(&an.lines, first, &format!("{fix}\n"));
            let st = commit(st, &text::text(&ls), "added the fix — fill the hole");
            State { cursor: Pos { line: first, col: 0 }, ..st }
        }
        "w" | "write" => st.say(format!("saved · {}", status(&an)), Tone::Ok),
        "q" | "quit" => State { effect: Some(Effect::Quit), ..st },
        "extract" | "factor" => match &ids {
            Some(ids) => commit_or_say(st, ops::extract(&an, ids, &arg.to_lowercase()), &format!("extracted {}", arg.to_lowercase())),
            None => st.say(need("a step"), Tone::No),
        },
        "inline" => match &ids {
            Some(ids) => commit_or_say(st, ops::inline(&an, ids), &format!("inlined {}", call.as_deref().unwrap_or(""))),
            None => st.say(need("a call"), Tone::No),
        },
        "drop" => match &ids {
            Some(ids) => commit_or_say(st, ops::drop(&an, ids), "dropped"),
            None => st.say(need("a step"), Tone::No),
        },
        "swap" => match &ids {
            Some(ids) => commit_or_say(st, ops::swap(&an, ids), "swapped"),
            None => st.say("select two steps next to each other", Tone::No),
        },
        "rename" => match word_at_cursor(&st) {
            Some(w) => commit_or_say(st, ops::rename(&an, &w, &arg.to_lowercase()), &format!("renamed {w} to {}", arg.to_lowercase())),
            None => st.say(need("a word's name or a call"), Tone::No),
        },
        "hole" | "punch" => match &ids {
            Some(ids) => commit_or_say(st, ops::punch(&an, ids, &arg).map(|p| p.0), "hole punched"),
            None => st.say(need("a step"), Tone::No),
        },
        "data" => {
            let table = ids.as_ref().and_then(|i| an.step(i[0])).and_then(|s| s.table().cloned());
            match table.or_else(|| (!arg.is_empty()).then(|| arg.as_str().into())).or_else(|| an.program.tables.keys().next().cloned()) {
                Some(t) => open_data(st, &t),
                None => st.say("this program has no tables", Tone::No),
            }
        }
        "debug" | "run" => {
            let i = match arg.parse::<usize>() {
                Ok(n) => n.wrapping_sub(1),
                Err(_) => st.image.runs.iter().position(|r| r.verdict != crate::runs::Verdict::Pass).unwrap_or(0),
            };
            start_debug(st, i)
        }
        "reset" => match an.program.tables.get(arg.as_str()) {
            _ if st.live.is_none() => st.say("reset works on the live database — naga start", Tone::No),
            Some(t) => State { effect: Some(Effect::Reset(t.name.clone())), ..st },
            None => st.say(format!("there's no table called {arg}"), Tone::No),
        },
        "rework" => {
            let Some(ids) = &ids else { return st.say(need("steps"), Tone::No) };
            match ops::punch(&an, ids, &arg) {
                Err(why) => st.say(why, Tone::No),
                Ok((text, original, note)) => {
                    let st = commit(st, &text, "");
                    let hole = st.an.holes().find(|h| h.hole().map(str::trim) == Some(note.as_str())).map(|h| h.id);
                    match hole {
                        Some(h) => start_agent(st, h, "rework", an, Some(original)),
                        None => st.say("rework: the hole went missing", Tone::No),
                    }
                }
            }
        }
        _ => st.say(format!("{raw} isn’t a command — try {}", OPS.join(" · ")), Tone::No),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const SRC: &str = "go = a @f( b c ) d\na = nop\nb = nop\nc = nop\nd = nop\nh = ? write me\n";

    pub fn keys(st: State, s: &str) -> State {
        // `<esc>`, `<cr>`, `<bs>` spell those keys; anything else is typed
        let mut st = st;
        let mut rest = s;
        while !rest.is_empty() {
            let (k, n) = match rest {
                r if r.starts_with("<esc>") => (Key::Escape, 5),
                r if r.starts_with("<cr>") => (Key::Enter, 4),
                r if r.starts_with("<bs>") => (Key::Backspace, 4),
                r if r.starts_with("<undo>") => (Key::Undo, 6),
                r => {
                    let c = r.chars().next().unwrap();
                    (Key::Char(c), c.len_utf8())
                }
            };
            st = update(st, Event::Key(k, false));
            rest = &rest[n..];
        }
        st
    }

    #[test]
    fn commands_work_on_the_step_under_the_cursor_and_undo_puts_it_back() {
        let st = keys(State::new(SRC), "$:drop<cr>");
        assert!(st.an.text.starts_with("go = a @f( b c )\n"), "{}", st.an.text);
        assert_eq!(st.msg.as_ref().map(|m| m.1), Some(Tone::Ok));
        let st = keys(st, "u");
        assert_eq!(st.an.text, SRC);
        let st = keys(st, ":frobnicate<cr>");
        assert_eq!(st.msg.as_ref().map(|m| m.1), Some(Tone::No));
    }

    #[test]
    fn visual_selections_choose_whole_sibling_steps() {
        let st = keys(State::new(SRC), "wwvee:extract ab<cr>");
        assert!(st.an.text.starts_with("go = ab @f( b c ) d\n\nab = a\n") || st.an.text.starts_with("go = a ab"), "{}", st.an.text);
    }

    #[test]
    fn the_data_view_walks_tables_and_runs() {
        let src = "table ts :: {n: Num}\ntable us :: {n: Num}\nroute GET /x = w\nw = +{status: 200, json: 1}\nrun \"a\"\n  GET /x → 200\nrun \"b\"\n  GET /x → 200\n";
        let st = keys(State::new(src), ":data<cr>");
        assert_eq!(st.data, Some(Data { table: "ts".into(), rows: Rows::Seed }));
        let st = keys(st, "lllj");
        assert_eq!(st.data, Some(Data { table: "us".into(), rows: Rows::Run(1) }));
        let st = keys(st, "hh<esc>");
        assert_eq!(st.data, None);
    }

    #[test]
    fn the_data_view_shows_the_live_rows() {
        use crate::value::Value;
        let src = "table ts :: {n: Num}\nseed ts = [{n: 1}]\n";
        let row = |id: f64, n: f64| Value::Rec(Rc::new([("id".into(), Value::Num(id)), ("n".into(), Value::Num(n))].into_iter().collect()));
        let live: crate::runs::Tables = [("ts".into(), vec![row(1.0, 5.0), row(2.0, 7.0)])].into_iter().collect();
        let st = State { live: Some((4170, None)), live_tables: Some(Rc::new(live)), ..State::new(src) };
        let st = keys(st, ":data<cr>");
        assert_eq!(st.data, Some(Data { table: "ts".into(), rows: Rows::Live }));
        let f = crate::editor::view::view(&st);
        let text: Vec<String> = f.code.iter().map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")).collect();
        assert!(text.contains(&"1 5".to_string()) && text.contains(&"2 7".to_string()), "live rows, unmarked: {text:?}");
        assert_eq!(keys(st, "l").data.map(|d| d.rows), Some(Rows::Seed));
    }

    #[test]
    fn an_explained_error_offers_its_fix_as_a_hole() {
        let src = "table notes :: {title: Text}\nreply = .{status json}\ncomplete-note = notes.get complete\ncomplete = -{note}\n";
        let st = State::new(src);
        let e = st.an.errors.iter().find(|e| e.detail.is_some()).expect("an explained error");
        assert_eq!(e.msg, "when notes.get finds no row, complete needs note");
        let d = e.detail.as_ref().unwrap();
        assert_eq!(d.refs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["notes.get", "complete"]);
        assert_eq!(d.fix.as_deref(), Some("complete {-missing} = ? when notes.get finds no row"));
        let st = keys(st, "3G:fix<cr>");
        assert!(st.an.text.ends_with("complete {-missing} = ? when notes.get finds no row\ncomplete = -{note}\n"), "{}", st.an.text);
        assert!(st.an.errors.iter().all(|e| e.detail.is_none()), "the branch is handled now");
    }

    #[test]
    fn a_table_opens_whatever_the_case_it_is_written_in() {
        let st = State::new("table houseS :: {price: Int}\n");
        let st = update(st, Event::Press(Some(Target::Table("houseS".into())), None, false));
        assert_eq!(st.data.map(|d| d.table), Some("houses".into()));
        let st = keys(State::new("table houseS :: {price: Int}\n"), ":data HOUSES<cr>");
        assert_eq!(st.data.map(|d| d.table), Some("houses".into()));
    }

    #[test]
    fn the_cheat_sheet_opens_scrolls_and_closes() {
        let st = keys(State::new(SRC), "?jjj");
        assert_eq!(st.help, Some(3));
        assert_eq!(keys(st, "<esc>").help, None);
        assert_eq!(keys(State::new(SRC), ":help<cr>").help, Some(0));
    }

    #[test]
    fn the_debugger_steps_over_into_and_to_breakpoints() {
        let src = "route GET /x = w\nw = a b +{status: 200, json: 1}\na = +{n: 1} +{m: 2}\nb = +{k: 3}\nrun \"x\"\n  GET /x → 200\n";
        let st = keys(State::new(src), ":debug 1<cr>");
        let line = |st: &State| st.debug.as_ref().and_then(|d| d.current()).map(|r| (r.pos.line, r.pos.col));
        let st = keys(st, "l");
        assert_eq!(line(&st), Some((2, 4)), "into w: its first step, a");
        let st = keys(st, "j");
        assert_eq!(line(&st), Some((2, 6)), "over a: b");
        let st = keys(st, "h");
        assert_eq!(line(&st), Some((3, 12)), "back: a's last step");
        let st = update(st, Event::Press(None, Some(Pos { line: 4, col: 4 }), false));
        let st = keys(st, "0<cr>");
        assert_eq!(line(&st), Some((4, 4)), "continue stops at the breakpoint in b");
        let st = keys(st, "<cr>");
        assert_eq!(st.debug.as_ref().map(|d| d.now().st.status), Some(crate::machine::Status::Done));
        assert!(keys(st, "<esc>").debug.is_none());
    }

    fn reply(st: State, code: &str) -> State {
        let id = st.agent.as_ref().unwrap().id;
        let fill = Fill { code: code.into(), helpers: None, why: "test".into() };
        let mut st = update(st, Event::AgentReply(id, Ok(fill)));
        while st.agent.as_ref().is_some_and(|a| a.fill.is_some()) {
            st = update(st, Event::Tick);
        }
        st
    }

    #[test]
    fn the_agent_retries_with_the_reason_and_keeps_a_fill_that_passes() {
        let src = "route GET /x = w\nw = ? answer one\nrun \"x\"\n  GET /x → 200 1\n";
        let mut st = keys(State::new(src), "j$ga");
        let (id, prompt) = st.ask.take().expect("a question for Claude");
        assert!(prompt.contains("Fill the hole in `w` on line 2: `? answer one`."), "{prompt}");
        let st = reply(st, "+{status: 200, json: 2}");
        let (id2, again) = st.ask.clone().expect("asks again");
        assert_eq!(id2, id);
        assert!(again.contains("Your previous fill was rejected") && again.contains("run “x” answers 200 2"), "{again}");
        let st = reply(st, "+{status: 200, json: 1}");
        assert!(st.agent.is_none());
        assert!(st.an.text.contains("w = +{status: 200, json: 1}"), "{}", st.an.text);
        assert_eq!(st.an.passing(), 1);
    }

    #[test]
    fn a_rework_that_finds_nothing_better_puts_the_original_back() {
        let src = "go = +{a: 1} -{a}\n";
        let mut st = keys(State::new(src), "wv$:rework<cr>");
        assert!(st.an.text.starts_with("go = ? +{a: 1} -{a}"), "{}", st.an.text);
        assert!(st.ask.take().unwrap().1.contains("This is a rework: the hole replaced `+{a: 1} -{a}`"));
        let id = st.agent.as_ref().unwrap().id;
        let st = update(st, Event::AgentReply(id, Err("Claude answered 529: overloaded".into())));
        assert_eq!(st.an.text, "go = +{a: 1} -{a}\n");
    }
}

//! The editor: the program drawn on the GPU, edited live.
mod agent;
mod analysis;
mod model;
mod debug;
mod gpu;
mod help;
mod ops;
mod outline;
mod state;
mod text;
mod vim;
mod view;

use state::{Event, Key, State, update};
use std::rc::Rc;
use std::sync::Arc;
use view::{Frame, view};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// What other threads wake the editor with: the model's answer to an agent.
enum Wake {
    Reply(u64, Result<model::Fill, String>),
    /// A request for the live server, answered on this thread with the editor's program.
    /// The live server answered a request: `GET /notes → 200`.
    Served(String),
    /// An image's runs, run on their own thread.
    Ran(state::Runs),
}

/// How long typing pauses before the image goes live: its tables, the server, its runs.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

struct App {
    proxy: EventLoopProxy<Wake>,
    quit: bool,
    /// The live server's tables, when the editor is serving.
    live: Option<Arc<crate::serve::Live>>,
    state: Option<State>,
    frame: Frame,
    gpu: Option<gpu::Gpu>,
    window: Option<Arc<Window>>,
    cursor: (f32, f32),
    mods: ModifiersState,
    pressed: bool,
    path: String,
    /// When the image goes live, if a new one is waiting.
    settle: Option<std::time::Instant>,
}

impl App {
    fn send(&mut self, ev: Event) {
        // the image (the last text that parsed) is what's live: the file, the tables, the server
        let before = self.state.as_ref().map(|s| s.image.clone());
        let mut next = update(self.state.take().expect("a state"), ev);
        if before.as_ref().is_some_and(|b| b.text != next.image.text) {
            self.settle = Some(std::time::Instant::now() + SETTLE);
        }
        if let Some(live) = &self.live {
            let p = next.image.program.clone();
            let plan = || live.with_store(|s| s.plan(&p)).and_then(|r| r.map_err(|e| e.msg));
            match next.effect.clone() {
                Some(state::Effect::Reset(t)) => {
                    next.msg = Some(match live.with_store(|s| s.reset(&p, &t)) {
                        Ok(Ok(())) => (format!("reset {t}: recreated and seeded"), state::Tone::Ok),
                        Ok(Err(e)) => (format!("reset {t}: {}", e.msg), state::Tone::No),
                        Err(e) => (format!("reset {t}: {e}"), state::Tone::No),
                    });
                    next.plan = Rc::new(plan().unwrap_or_default());
                }
                Some(state::Effect::Plan) => match plan() {
                    Ok(pl) => (next.plan, next.migrating) = (Rc::new(pl), true),
                    Err(e) => next.msg = Some((format!("live database: {e}"), state::Tone::No)),
                },
                // only what was shown is done: if the tables moved since, show them again
                Some(state::Effect::Migrate(shown)) => match plan() {
                    Ok(pl) if pl != *shown => (next.plan, next.msg) = (Rc::new(pl), Some(("the tables changed since — this is what it takes now".into(), state::Tone::No))),
                    Ok(pl) => {
                        let changes: Vec<&crate::store::Change> = pl.changes.iter().collect();
                        next.msg = Some(match live.with_store(|s| s.apply(&p, &changes)) {
                            Ok(Ok(())) => (format!("migrated · {}", pl.changes.iter().map(|c| c.what.as_str()).collect::<Vec<_>>().join(" · ")), state::Tone::Ok),
                            Ok(Err(e)) => (format!("migrate: {} — nothing changed", e.msg), state::Tone::No),
                            Err(e) => (format!("migrate: {e}"), state::Tone::No),
                        });
                        (next.plan, next.migrating) = (Rc::new(plan().unwrap_or_default()), false);
                    }
                    Err(e) => next.msg = Some((format!("live database: {e}"), state::Tone::No)),
                },
                _ => {}
            }
            if matches!(next.effect, Some(state::Effect::Reset(_) | state::Effect::Plan | state::Effect::Migrate(_))) {
                next.effect = None;
            }
            // while the data view is open it shows the live tables as they are now
            if next.data.is_some() {
                next.live_tables = live.with_store(|s| s.snapshot(&p)).ok().and_then(Result::ok).map(Rc::new);
            }
        }
        if next.effect == Some(state::Effect::Quit) {
            self.quit = true;
        }
        // a question for the model goes out on its own thread; the answer comes back as a Wake
        if let Some((id, q)) = next.ask.take() {
            let proxy = self.proxy.clone();
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn(move || {
                    let _ = proxy.send_event(Wake::Reply(id, model::ask(&q)));
                })
                .ok();
        }
        if before.is_some_and(|b| b.text != next.image.text)
            && let Err(e) = std::fs::write(&self.path, &next.image.text)
        {
            eprintln!("naga: {}: {e}", self.path);
        }
        self.frame = view(&next);
        self.state = Some(next);
        if let Some(g) = &self.gpu {
            g.redraw();
        }
    }
    /// Typing paused: the image goes live. Its tables fit it first, then requests and open
    /// pages run on it; its runs run on their own thread.
    fn go_live(&mut self) {
        self.settle = None;
        let Some(mut st) = self.state.take() else { return };
        let p = st.image.program.clone();
        if let Some(live) = &self.live {
            // the tables change only on :migrate; until then, what it would take
            if let Ok(Ok(plan)) = live.with_store(|s| s.plan(&p)) {
                st.plan = Rc::new(plan);
            }
            live.publish(p.clone());
        }
        let (proxy, text) = (self.proxy.clone(), st.image.text.clone());
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || {
                let _ = proxy.send_event(Wake::Ran(state::Runs::new(text, &p)));
            })
            .ok();
        self.frame = view(&st);
        self.state = Some(st);
        if let Some(g) = &self.gpu {
            g.redraw();
        }
    }

    fn pos(&self) -> Option<crate::ast::Pos> {
        self.gpu.as_ref()?.pos_at(&self.frame, self.cursor.0, self.cursor.1)
    }

    fn target(&self) -> Option<view::Target> {
        self.gpu.as_ref()?.target_at(&self.frame, self.cursor.0, self.cursor.1)
    }
}

impl ApplicationHandler<Wake> for App {
    fn user_event(&mut self, _: &ActiveEventLoop, w: Wake) {
        match w {
            Wake::Reply(id, r) => self.send(Event::AgentReply(id, r)),
            Wake::Served(line) => self.send(Event::Served(line)),
            Wake::Ran(runs) => self.send(Event::Ran(Rc::new(runs))),
        }
    }

    /// While the agent types its fill, time moves in 60ths of a second.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quit {
            return el.exit();
        }
        let typing = self.state.as_ref().and_then(|s| s.agent.as_ref()).is_some_and(|a| a.fill.is_some());
        let tick = typing.then(|| std::time::Instant::now() + std::time::Duration::from_millis(16));
        el.set_control_flow(match tick.into_iter().chain(self.settle).min() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            match self.settle {
                Some(t) if t <= std::time::Instant::now() => self.go_live(),
                _ => self.send(Event::Tick),
            }
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        // the code goes edge to edge, under a title bar that keeps only the traffic lights
        let attrs = Window::default_attributes().with_title(format!("naga · {}", self.path)).with_inner_size(winit::dpi::LogicalSize::new(1280.0, 860.0));
        #[cfg(target_os = "macos")]
        let attrs = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
        };
        let window = Arc::new(el.create_window(attrs).expect("a window"));
        let size = window.inner_size();
        self.gpu = Some(pollster::block_on(gpu::Gpu::new(Some(window.clone()), (size.width, size.height), window.scale_factor() as f32)));
        self.window = Some(window);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let (Some(g), Some(w)) = (&mut self.gpu, &self.window) {
                    let size = w.inner_size();
                    g.resize((size.width, size.height), w.scale_factor() as f32);
                    g.redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let scroll = self.state.as_ref().map_or(0.0, |s| s.scroll);
                let Some(g) = &mut self.gpu else { return };
                g.draw(&self.frame, scroll);
                if let Some(ev) = viewport(g, &self.frame, self.state.as_ref()) {
                    self.send(ev);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                let t = self.target();
                if self.state.as_ref().is_some_and(|s| s.hover != t) || self.pressed {
                    self.send(Event::Hover(t, self.pos(), self.pressed));
                }
            }
            WindowEvent::CursorLeft { .. } => self.send(Event::Hover(None, None, false)),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.gpu.as_ref().is_some_and(|g| self.cursor.1 < g.layout.top) =>
            {
                if let Some(w) = &self.window {
                    let _ = w.drag_window();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.pressed = state == ElementState::Pressed;
                let ev = if self.pressed { Event::Press(self.target(), self.pos(), self.mods.shift_key()) } else { Event::Release };
                self.send(ev);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 3.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / self.gpu.as_ref().map_or(25.0, |g| g.layout.code_line),
                };
                self.send(Event::Scroll(lines));
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let command = self.mods.super_key() || self.mods.control_key();
                // no close button, so ⌘W and ⌘Q close it
                if self.mods.super_key() && matches!(&event.logical_key, WKey::Character(c) if c.eq_ignore_ascii_case("w") || c.eq_ignore_ascii_case("q")) {
                    return el.exit();
                }
                let key = match &event.logical_key {
                    WKey::Named(NamedKey::ArrowLeft) => Key::Left,
                    WKey::Named(NamedKey::ArrowRight) => Key::Right,
                    WKey::Named(NamedKey::ArrowUp) => Key::Up,
                    WKey::Named(NamedKey::ArrowDown) => Key::Down,
                    WKey::Named(NamedKey::Escape) => Key::Escape,
                    WKey::Named(NamedKey::Enter) => Key::Enter,
                    WKey::Named(NamedKey::Backspace) => Key::Backspace,
                    WKey::Named(NamedKey::Home) => Key::Home,
                    WKey::Named(NamedKey::Tab) => Key::Tab,
                    WKey::Character(c) if self.mods.control_key() && c.eq_ignore_ascii_case("r") => Key::Redo,
                    WKey::Character(c) if self.mods.control_key() && c == "[" => Key::Escape,
                    WKey::Named(NamedKey::Space) if !command => Key::Char(' '),
                    WKey::Character(c) if command && c.eq_ignore_ascii_case("z") => Key::Undo,
                    WKey::Character(c) if !command => match event.text.as_deref().and_then(|t| t.chars().next()) {
                        Some(ch) => Key::Char(ch),
                        None => return,
                    },
                    _ => return,
                };
                self.send(Event::Key(key, self.mods.shift_key()));
            }
            _ => {}
        }
    }
}

fn key(name: &str) -> Option<(Key, bool)> {
    let (shift, k) = name.strip_prefix("shift+").map_or((false, name), |k| (true, k));
    let k = match k {
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "esc" => Key::Escape,
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "undo" => Key::Undo,
        "home" => Key::Home,
        "tab" => Key::Tab,
        _ => return None,
    };
    Some((k, shift))
}

/// What the state should know about the screen, when that changed.
fn viewport(g: &gpu::Gpu, f: &Frame, st: Option<&State>) -> Option<Event> {
    let lines: Vec<usize> = f.code.iter().map(|l| l.src).collect();
    let st = st?;
    (st.rows != g.rows() || *st.row_lines != lines).then(|| Event::Viewport(g.rows(), lines))
}

/// Draws the editor into a PNG after some events, at 2x like a Retina screen.
fn shot(state: State, out: &str, opts: &std::collections::HashMap<&str, &str>) -> Result<(), String> {
    let (w, h) = opts.get("--size").and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).unwrap_or((1280, 860));
    let mut g = pollster::block_on(gpu::Gpu::new(None, (w * 2, h * 2), 2.0));
    let mut st = state;
    st.scroll = opts.get("--scroll").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let f = view(&st);
    g.draw(&f, st.scroll);
    if let Some(ev) = viewport(&g, &f, Some(&st)) {
        st = update(st, ev);
    }
    // --click line:col, a program line
    if let Some((l, c)) = opts.get("--click").and_then(|rc| rc.split_once(':')).and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?))) {
        let t = view(&st).code.iter().find(|x| x.src == l).and_then(|x| x.target_at(c).cloned());
        let at = Some(crate::ast::Pos { line: l, col: c });
        st = update(update(st, Event::Press(t.clone(), at, false)), Event::Release);
        st = update(st, Event::Hover(t, at, false));
    }
    for c in opts.get("--type").copied().unwrap_or("").chars() {
        st = update(st, Event::Key(Key::Char(c), false));
    }
    for k in opts.get("--keys").map_or(vec![], |k| k.split(',').collect()) {
        let (k, shift) = key(k).ok_or(format!("naga: unknown key {k}"))?;
        st = update(st, Event::Key(k, shift));
    }
    // the typing is over: the runs catch up
    let runs = Rc::new(state::Runs::of(&st.image));
    st = update(st, Event::Ran(runs));
    let f = view(&st);
    g.draw(&f, st.scroll);
    g.save(out)
}

/// Opens `path` in the editor. With `--shot out.png` among `args`, draws one frame offscreen
/// instead, after replaying `--keys`, `--type`, `--click` and `--scroll`.
pub fn edit(path: &str, args: &[String]) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("naga: {path}: {e}"))?;
    let state = State::new(&text);
    let opts: std::collections::HashMap<&str, &str> = args.chunks(2).filter_map(|p| Some((p[0].as_str(), p.get(1)?.as_str()))).collect();
    if let Some(out) = opts.get("--shot") {
        return shot(state, out, &opts);
    }
    let frame = view(&state);
    let el = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    let mut app = App { proxy: el.create_proxy(), quit: false, live: None, state: Some(state), frame, gpu: None, window: None, cursor: (0.0, 0.0), mods: ModifiersState::empty(), pressed: false, path: path.into(), settle: None };
    el.run_app(&mut app).map_err(|e| e.to_string())
}

/// `naga start`: the editor, serving the program it edits on `port`. Each image the editor
/// publishes is what requests run on from then on; they run in parallel, off the editor's
/// thread, against the tables in `db`.
pub fn start(path: &str, port: u16) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("naga: {path}: {e}"))?;
    let mut state = State::new(&text);
    let db = std::path::Path::new(path).with_extension("db");
    let live = crate::serve::Live::connect(state.image.program.clone(), &db)?;
    state.plan = Rc::new(live.with_store(|s| s.plan(&state.image.program)).and_then(|r| r.map_err(|e| e.msg))?);
    let el = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    let proxy = el.create_proxy();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("naga: port {port}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let server = live.clone();
    std::thread::spawn(move || {
        let rt = crate::serve::runtime().expect("a runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("the listener");
            let app = crate::serve::router(server, move |raw, h| {
                let q = if raw.query.is_empty() { String::new() } else { format!("?{}", raw.query) };
                let _ = proxy.send_event(Wake::Served(format!("{} {}{q} → {}", raw.method, raw.path, h.status)));
            });
            let _ = axum::serve(listener, app).await;
        });
    });
    state.live = Some((port, None));
    let frame = view(&state);
    let mut app = App { proxy: el.create_proxy(), quit: false, live: Some(live), state: Some(state), frame, gpu: None, window: None, cursor: (0.0, 0.0), mods: ModifiersState::empty(), pressed: false, path: path.into(), settle: None };
    el.run_app(&mut app).map_err(|e| e.to_string())
}

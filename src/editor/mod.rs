//! The editor: the program drawn on the GPU, edited live.
mod agent;
mod analysis;
mod claude;
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

/// What other threads wake the editor with: Claude's answer to an agent.
enum Wake {
    Reply(u64, Result<claude::Fill, String>),
    /// A request for the live server, answered on this thread with the editor's program.
    Request(crate::serve::Raw, crate::serve::Reply),
}

struct App {
    proxy: EventLoopProxy<Wake>,
    quit: bool,
    /// The live server's tables, when the editor is serving.
    store: Option<crate::store::Store>,
    state: Option<State>,
    frame: Frame,
    gpu: Option<gpu::Gpu>,
    window: Option<Arc<Window>>,
    cursor: (f32, f32),
    mods: ModifiersState,
    pressed: bool,
    path: String,
}

impl App {
    fn send(&mut self, ev: Event) {
        // the image (the last text that parsed) is what's live: the file, the tables, the server
        let before = self.state.as_ref().map(|s| s.image.clone());
        let mut next = update(self.state.take().expect("a state"), ev);
        // the live database follows the program: safe changes at once, the rest pending
        if let Some(store) = &mut self.store {
            if before.as_ref().is_some_and(|b| b.text != next.image.text) {
                match store.migrate(&next.image.program) {
                    Ok(m) => {
                        if !m.applied.is_empty() {
                            next.msg = Some((format!("live · {}", m.applied.join(" · ")), state::Tone::Ok));
                        }
                        next.pending = m.pending;
                    }
                    Err(e) => next.msg = Some((format!("live database: {}", e.msg), state::Tone::No)),
                }
            }
            if let Some(state::Effect::Reset(t)) = next.effect.clone() {
                next.effect = None;
                next.msg = Some(match store.reset(&next.image.program, &t) {
                    Ok(()) => (format!("reset {t}: recreated and seeded"), state::Tone::Ok),
                    Err(e) => (format!("reset {t}: {}", e.msg), state::Tone::No),
                });
                next.pending = store.migrate(&next.image.program).map(|m| m.pending).unwrap_or_default();
            }
        }
        // while the data view is open it shows the live tables as they are now
        if let (Some(store), Some(_)) = (&mut self.store, &next.data) {
            next.live_tables = store.snapshot(&next.image.program).ok().map(Rc::new);
        }
        if next.effect == Some(state::Effect::Quit) {
            self.quit = true;
        }
        // a question for Claude goes out on its own thread; the answer comes back as a Wake
        if let Some((id, prompt)) = next.ask.take() {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                let _ = proxy.send_event(Wake::Reply(id, claude::ask(&prompt)));
            });
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
            Wake::Request(raw, reply) => {
                let (Some(st), Some(store)) = (&self.state, &mut self.store) else { return };
                let h = crate::serve::respond(&st.image.program, store, &raw);
                let line = format!("{} {}{} → {}", raw.method, raw.path, if raw.query.is_empty() { String::new() } else { format!("?{}", raw.query) }, h.status);
                let _ = reply.send((h.status, h.json));
                self.send(Event::Served(line));
            }
        }
    }

    /// While the agent types its fill, time moves in 60ths of a second.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quit {
            return el.exit();
        }
        let typing = self.state.as_ref().and_then(|s| s.agent.as_ref()).is_some_and(|a| a.fill.is_some());
        el.set_control_flow(match typing {
            true => ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_millis(16)),
            false => ControlFlow::Wait,
        });
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. }) {
            self.send(Event::Tick);
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        // frameless: the code goes edge to edge; the strip above it moves the window
        let attrs = Window::default_attributes()
            .with_title(format!("naga · {}", self.path))
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 860.0));
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
    let mut app = App { proxy: el.create_proxy(), quit: false, store: None, state: Some(state), frame, gpu: None, window: None, cursor: (0.0, 0.0), mods: ModifiersState::empty(), pressed: false, path: path.into() };
    el.run_app(&mut app).map_err(|e| e.to_string())
}

/// `naga start`: the editor, serving the program it edits on `port`. Requests run on the
/// editor's thread, on the program as it is at that moment, against the tables in `db`.
pub fn start(path: &str, port: u16) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("naga: {path}: {e}"))?;
    let mut state = State::new(&text);
    let db = std::path::Path::new(path).with_extension("db");
    let (store, m) = crate::serve::open(&state.an.program, &db)?;
    state.pending = m.pending;
    let el = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    let proxy = el.create_proxy();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("naga: port {port}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("the listener");
            let app = crate::serve::router(move |raw, reply| drop(proxy.send_event(Wake::Request(raw, reply))));
            let _ = axum::serve(listener, app).await;
        });
    });
    state.live = Some((port, None));
    let frame = view(&state);
    let mut app = App { proxy: el.create_proxy(), quit: false, store: Some(store), state: Some(state), frame, gpu: None, window: None, cursor: (0.0, 0.0), mods: ModifiersState::empty(), pressed: false, path: path.into() };
    el.run_app(&mut app).map_err(|e| e.to_string())
}

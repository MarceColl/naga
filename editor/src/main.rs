//! `naga-edit file.nag`: the hole editor, drawn on the GPU.
mod analysis;
mod gpu;
mod state;
mod view;

use state::{Event, Key, State, update};
use std::sync::Arc;
use view::{Frame, view};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

struct App {
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
        let next = update(self.state.take().expect("a state"), ev);
        self.frame = view(&next);
        self.state = Some(next);
        if let Some(g) = &self.gpu {
            g.redraw();
        }
    }
    fn target(&self) -> Option<view::Target> {
        let (g, st) = (self.gpu.as_ref()?, self.state.as_ref()?);
        gpu::target_at(&self.frame, &g.layout, self.cursor.0, self.cursor.1, st.scroll)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(format!("naga · {}", self.path)).with_inner_size(winit::dpi::LogicalSize::new(1280.0, 860.0));
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
                    self.send(Event::Hover(t, self.pressed));
                }
            }
            WindowEvent::CursorLeft { .. } => self.send(Event::Hover(None, false)),
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.pressed = state == ElementState::Pressed;
                let ev = if self.pressed { Event::Press(self.target(), self.mods.shift_key()) } else { Event::Release };
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
                let key = match event.logical_key {
                    WKey::Named(NamedKey::ArrowLeft) => Key::Left,
                    WKey::Named(NamedKey::ArrowRight) => Key::Right,
                    WKey::Named(NamedKey::ArrowUp) => Key::Up,
                    WKey::Named(NamedKey::ArrowDown) => Key::Down,
                    WKey::Named(NamedKey::Escape) => Key::Escape,
                    _ => return,
                };
                self.send(Event::Key(key, self.mods.shift_key()));
            }
            _ => {}
        }
    }
}

const USAGE: &str = "usage: naga-edit <file.nag> [--shot out.png [--keys right,shift+right,down,esc] [--click row:col] [--scroll lines] [--size 1280x860]]";

fn key(name: &str) -> Option<(Key, bool)> {
    let (shift, k) = name.strip_prefix("shift+").map_or((false, name), |k| (true, k));
    let k = match k {
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "esc" => Key::Escape,
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
    for k in opts.get("--keys").map_or(vec![], |k| k.split(',').collect()) {
        let (k, shift) = key(k).ok_or(format!("naga-edit: unknown key {k}"))?;
        st = update(st, Event::Key(k, shift));
    }
    if let Some((r, c)) = opts.get("--click").and_then(|rc| rc.split_once(':')).and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?))) {
        let t = view(&st).code.get(r).and_then(|l| l.target_at(c).cloned());
        st = update(update(st, Event::Press(t.clone(), false)), Event::Release);
        st = update(st, Event::Hover(t, false));
    }
    let f = view(&st);
    g.draw(&f, st.scroll);
    g.save(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first().cloned() else {
        eprintln!("{USAGE}");
        std::process::exit(1);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("naga-edit: {path}: {e}");
        std::process::exit(1);
    });
    let state = State::new(&text);
    let opts: std::collections::HashMap<&str, &str> = args[1..].chunks(2).filter_map(|p| Some((p[0].as_str(), p.get(1)?.as_str()))).collect();
    if let Some(out) = opts.get("--shot") {
        if let Err(e) = shot(state, out, &opts) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    let frame = view(&state);
    let mut app = App { state: Some(state), frame, gpu: None, window: None, cursor: (0.0, 0.0), mods: ModifiersState::empty(), pressed: false, path };
    EventLoop::new().expect("an event loop").run_app(&mut app).expect("the editor runs");
}

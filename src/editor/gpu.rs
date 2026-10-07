//! Draws a `Frame`: a pass of solid rectangles (backgrounds, selection, holes, error
//! underlines, the bar's rule), then the text with glyphon. It also answers which target sits
//! under a point, since only it knows where the text went.
use crate::editor::view::{Color, Frame, Line, Style, Target, palette};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, Stretch, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport, Weight, Wrap,
};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

const FONTS: [&[u8]; 5] = [
    include_bytes!("../../fonts/MartianMono-sWdMd.ttf"),
    include_bytes!("../../fonts/MartianMono-sWdBd.ttf"),
    include_bytes!("../../fonts/MartianMono-sWdxBd.ttf"),
    include_bytes!("../../fonts/MartianMono-StdMd.ttf"),
    include_bytes!("../../fonts/MartianMono-StdxBd.ttf"),
];

const SHADER: &str = r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@group(0) @binding(0) var<uniform> screen: vec2<f32>;
@vertex fn vs(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>) -> Out {
    var o: Out;
    o.pos = vec4<f32>(p.x / screen.x * 2.0 - 1.0, 1.0 - p.y / screen.y * 2.0, 0.0, 1.0);
    o.color = c;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return i.color; }
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    p: [f32; 2],
    c: [f32; 4],
}

/// sRGB bytes to the linear floats an sRGB surface expects.
fn linear(c: Color) -> [f32; 4] {
    let f = |x: u8| {
        let x = x as f32 / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

/// Where things go, in physical pixels, for the current window size.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    pub code_size: f32,
    pub code_line: f32,
    pub cell: f32,
    pub bar_size: f32,
    pub bar_line: f32,
    pub pad: f32,
    pub top: f32,
    /// Where the bar starts.
    pub bar_top: f32,
    /// The bar's height with all three shape rows showing.
    pub bar_max: f32,
}


pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    fonts: FontSystem,
    swash: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text: TextRenderer,
    pipeline: wgpu::RenderPipeline,
    screen: wgpu::Buffer,
    bind: wgpu::BindGroup,
    code: Buffer,
    shape: Buffer,
    msg: Buffer,
    stat: Buffer,
    pub layout: Layout,
    out: Out,
    /// Where the code went in the last frame: the first row shown, the top of row 0, and
    /// each shown row's character edges (`n + 1` of them), as laid out — fallback glyphs
    /// need not be one cell wide.
    first: usize,
    code_top: f32,
    edges: Vec<Vec<f32>>,
}

/// Where frames go: a window's surface, or a texture that can be saved as a PNG.
enum Out {
    Window(wgpu::Surface<'static>, Arc<Window>),
    Image(wgpu::Texture),
}

impl Gpu {
    /// Draws into `window`, or, without one, into an image of `size` physical pixels.
    pub async fn new(window: Option<Arc<Window>>, size: (u32, u32), scale: f32) -> Gpu {
        let instance = wgpu::Instance::default();
        let surface = window.as_ref().map(|w| instance.create_surface(w.clone()).expect("a surface for the window"));
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: surface.as_ref(), ..Default::default() }).await.expect("a GPU");
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.expect("a GPU device");
        let format = if window.is_some() { wgpu::TextureFormat::Bgra8UnormSrgb } else { wgpu::TextureFormat::Rgba8UnormSrgb };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let out = match (surface, window) {
            (Some(s), Some(w)) => {
                s.configure(&device, &config);
                Out::Window(s, w)
            }
            _ => Out::Image(device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })),
        };
        let mut fonts = FontSystem::new();
        for f in FONTS {
            fonts.db_mut().load_font_data(f.to_vec());
        }
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let screen = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() }] });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let empty = |fonts: &mut FontSystem| Buffer::new(fonts, Metrics::new(14.0, 20.0));
        let (code, shape, msg, stat) = (empty(&mut fonts), empty(&mut fonts), empty(&mut fonts), empty(&mut fonts));
        let mut gpu = Gpu {
            device,
            queue,
            config,
            fonts,
            swash: SwashCache::new(),
            viewport,
            atlas,
            text,
            pipeline,
            screen,
            bind,
            code,
            shape,
            msg,
            stat,
            layout: Layout { scale: 1.0, width: 1.0, height: 1.0, code_size: 14.0, code_line: 25.0, cell: 9.0, bar_size: 12.0, bar_line: 18.0, pad: 16.0, top: 28.0, bar_top: 0.0, bar_max: 0.0 },
            out,
            first: 0,
            code_top: 0.0,
            edges: vec![],
        };
        gpu.resize(size, scale);
        gpu
    }

    /// Code rows that always fit above the bar, however tall it gets.
    pub fn rows(&self) -> usize {
        let l = self.layout;
        ((l.height - l.bar_max - l.top) / l.code_line).floor().max(1.0) as usize
    }

    pub fn redraw(&self) {
        if let Out::Window(_, w) = &self.out {
            w.request_redraw();
        }
    }

    /// Sizes for `size` physical pixels at `scale`, as the reference's CSS clamps them.
    pub fn resize(&mut self, size: (u32, u32), scale: f32) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        if let Out::Window(s, _) = &self.out {
            s.configure(&self.device, &self.config);
        }
        let size = winit::dpi::PhysicalSize::new(self.config.width, self.config.height);
        let logical = size.width as f32 / scale;
        let code_size = (0.0055 * logical + 8.0).clamp(11.5, 16.0) * scale;
        let bar_size = (0.0035 * logical + 8.5).clamp(11.0, 13.0) * scale;
        let cell = self.advance(code_size);
        self.layout = Layout {
            scale,
            width: size.width as f32,
            height: size.height as f32,
            code_size,
            code_line: code_size * 1.8,
            cell,
            bar_size,
            bar_line: bar_size * 1.55,
            pad: 16.0 * scale,
            top: 28.0 * scale,
            bar_top: self.layout.bar_top,
            bar_max: self.layout.bar_max,
        };
        self.queue.write_buffer(&self.screen, 0, &[size.width as f32, size.height as f32, 0.0, 0.0].map(f32::to_ne_bytes).concat());
    }

    /// The width of one code cell: Martian Mono is monospaced in every weight.
    fn advance(&mut self, size: f32) -> f32 {
        let mut b = Buffer::new(&mut self.fonts, Metrics::new(size, size));
        b.set_text("MMMMMMMMMM", &code_attrs(&Style::plain(palette::FG)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.fonts, false);
        b.layout_runs().next().map_or(size * 0.6, |r| r.line_w / 10.0)
    }

    pub fn draw(&mut self, f: &Frame, scroll: f32) {
        let l = self.layout;
        // the bar: shapes, then the message; the counts sit right of the message's first line
        set_lines(&mut self.fonts, &mut self.stat, &[&f.stat], Metrics::new(l.bar_size, l.bar_line), bar_attrs, None, Wrap::None);
        let stat_px = self.stat.layout_runs().next().map_or(0.0, |r| r.line_w);
        let m = Metrics::new(l.bar_size, l.bar_line);
        let full_w = l.width - 2.0 * l.pad;
        let shapes: Vec<&Line> = f.shape.iter().collect();
        set_lines(&mut self.fonts, &mut self.shape, &shapes, m, bar_attrs, Some(full_w), Wrap::WordOrGlyph);
        // the shapes get three rows at most, the message what it needs beside the counts
        let shape_rows = (self.shape.layout_runs().count() as f32).min(3.0) * f32::from(!f.shape.is_empty());
        set_lines(&mut self.fonts, &mut self.msg, &[&f.msg], m, bar_attrs, Some(full_w - stat_px - 18.0 * l.scale), Wrap::WordOrGlyph);
        let msg_rows = self.msg.layout_runs().count().max(1) as f32;
        let gap = 4.0 * l.scale * f32::from(shape_rows > 0.0);
        let bar_rows = shape_rows + msg_rows;
        let bar_h = bar_rows * l.bar_line + gap + 20.0 * l.scale;
        let bar_top = l.height - bar_h;
        self.layout.bar_top = bar_top;
        self.layout.bar_max = (3.0 + msg_rows) * l.bar_line + 4.0 * l.scale + 20.0 * l.scale;

        // the code, shifted up by the scroll
        let first = scroll.floor() as usize;
        let rows = ((bar_top - l.top) / l.code_line).ceil() as usize + 1;
        let shown: Vec<&Line> = f.code.iter().skip(first).take(rows).collect();
        set_lines(&mut self.fonts, &mut self.code, &shown, Metrics::new(l.code_size, l.code_line), code_attrs, None, Wrap::None);
        let code_top = l.top - scroll.fract() * l.code_line;
        self.edges = shown.iter().map(|line| grid(line, l)).collect();
        for run in self.code.layout_runs() {
            if let Some(e) = self.edges.get_mut(run.line_i) {
                *e = edges(run.text, run.glyphs, run.line_w).iter().map(|x| l.pad + x).collect();
            }
        }
        (self.first, self.code_top) = (first, code_top);

        let mut rects: Vec<Vertex> = vec![];
        let mut rect = |x: f32, y: f32, w: f32, h: f32, c: Color| {
            let c = linear(c);
            for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y), (x + w, y + h), (x, y + h)] {
                rects.push(Vertex { p: [px, py], c });
            }
        };
        let glyph_h = l.code_size * 1.35;
        for (r, line) in shown.iter().enumerate() {
            let y = code_top + r as f32 * l.code_line;
            let e = &self.edges[r];
            let mut col = 0;
            for s in &line.spans {
                let n = s.text.chars().count();
                let (x, x1) = (e[col.min(e.len() - 1)], e[(col + n).min(e.len() - 1)]);
                if let Some(bg) = s.style.bg {
                    rect(x, y + (l.code_line - glyph_h) / 2.0, x1 - x, glyph_h, bg);
                }
                if s.style.err {
                    // a wave: short dashes stepping up and down
                    let base = y + (l.code_line + glyph_h) / 2.0;
                    let step = 2.0 * l.scale;
                    let mut i = 0.0;
                    while i < x1 - x {
                        let up = ((i / step) as i32 % 4) as f32;
                        let dy = if up < 2.0 { up } else { 4.0 - up } * l.scale;
                        rect(x + i, base - dy, step, 2.0 * l.scale, palette::ERR);
                        i += step;
                    }
                }
                col += n;
            }
        }
        if let Some((row, col)) = f.caret.filter(|c| c.0 >= first && c.0 - first < self.edges.len()) {
            let (r, e) = (row - first, &self.edges[row - first]);
            let x = e.get(col).copied().unwrap_or_else(|| e.last().copied().unwrap_or(l.pad));
            rect(x, code_top + r as f32 * l.code_line + (l.code_line - glyph_h) / 2.0, 2.0 * l.scale, glyph_h, palette::FG);
        }
        rect(0.0, bar_top, l.width, bar_h, palette::BG);
        rect(0.0, bar_top, l.width, l.scale.max(1.0), palette::RULE);
        let vertices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytes(&rects), usage: wgpu::BufferUsages::VERTEX });

        self.viewport.update(&self.queue, Resolution { width: self.config.width, height: self.config.height });
        let full = |bottom: f32| TextBounds { left: 0, top: 0, right: l.width as i32, bottom: bottom as i32 };
        let pad_top = 10.0 * l.scale;
        let msg_top = bar_top + pad_top + shape_rows * l.bar_line + gap;
        let areas = [
            TextArea { buffer: &self.code, left: l.pad, top: code_top, scale: 1.0, bounds: full(bar_top), default_color: glyphon::Color::rgb(236, 236, 236), custom_glyphs: &[] },
            TextArea {
                buffer: &self.shape,
                left: l.pad,
                top: bar_top + pad_top,
                scale: 1.0,
                bounds: full(bar_top + pad_top + shape_rows * l.bar_line),
                default_color: glyphon::Color::rgb(236, 236, 236),
                custom_glyphs: &[],
            },
            TextArea { buffer: &self.msg, left: l.pad, top: msg_top, scale: 1.0, bounds: full(l.height), default_color: glyphon::Color::rgb(140, 140, 140), custom_glyphs: &[] },
            TextArea {
                buffer: &self.stat,
                left: l.width - l.pad - stat_px,
                top: msg_top + (msg_rows - 1.0) * l.bar_line,
                scale: 1.0,
                bounds: full(l.height),
                default_color: glyphon::Color::rgb(140, 140, 140),
                custom_glyphs: &[],
            },
        ];
        // the code's rects are drawn under the bar's background, the text over both
        let (code_rects, bar_rects) = (rects.len() as u32 - 12, 12);
        self.text.prepare(&self.device, &self.queue, &mut self.fonts, &mut self.atlas, &self.viewport, areas, &mut self.swash).expect("text fits the atlas");

        let frame = match &self.out {
            Out::Window(s, w) => match s.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                _ => {
                    s.configure(&self.device, &self.config);
                    w.request_redraw();
                    return;
                }
            },
            Out::Image(_) => None,
        };
        let view = match (&frame, &self.out) {
            (Some(f), _) => f.texture.create_view(&wgpu::TextureViewDescriptor::default()),
            (None, Out::Image(t)) => t.create_view(&wgpu::TextureViewDescriptor::default()),
            (None, Out::Window(..)) => unreachable!(),
        };
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.draw(0..code_rects, 0..1);
            pass.draw(code_rects..code_rects + bar_rects, 0..1);
            self.text.render(&self.atlas, &self.viewport, &mut pass).expect("text renders");
        }
        self.queue.submit(Some(enc.finish()));
        if let Some(f) = frame {
            self.queue.present(f);
        }
        self.atlas.trim();
    }

    /// Saves the last frame drawn into an image as a PNG.
    pub fn save(&self, path: &str) -> Result<(), String> {
        let Out::Image(tex) = &self.out else { return Err("only an image can be saved".into()) };
        let (w, h) = (self.config.width, self.config.height);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
        let data = buf.slice(..).get_mapped_range().map_err(|e| format!("{e:?}"))?;
        let pixels: Vec<u8> = data.chunks(row as usize).flat_map(|r| &r[..(w * 4) as usize]).copied().collect();
        let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
        let mut png = png::Encoder::new(file, w, h);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header().and_then(|mut wr| wr.write_image_data(&pixels)).map_err(|e| e.to_string())
    }
}

fn bytes(v: &[Vertex]) -> &[u8] {
    // SAFETY: Vertex is repr(C) plain floats with no padding.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

fn code_attrs(s: &Style) -> Attrs<'static> {
    Attrs::new().family(Family::Name("Martian Mono")).stretch(Stretch::SemiExpanded).weight(Weight(s.weight)).color(glyphon::Color::rgb(s.fg[0], s.fg[1], s.fg[2]))
}

fn bar_attrs(s: &Style) -> Attrs<'static> {
    let w = if s.weight >= 700 { 800 } else { 500 };
    Attrs::new().family(Family::Name("Martian Mono")).stretch(Stretch::Normal).weight(Weight(w)).color(glyphon::Color::rgb(s.fg[0], s.fg[1], s.fg[2]))
}

fn set_lines(fonts: &mut FontSystem, buf: &mut Buffer, lines: &[&Line], m: Metrics, attrs: fn(&Style) -> Attrs<'static>, width: Option<f32>, wrap: Wrap) {
    buf.set_metrics(m);
    buf.set_wrap(wrap);
    buf.set_size(width, None);
    let mut spans: Vec<(&str, Attrs)> = vec![];
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            spans.push(("\n", attrs(&Style::plain(palette::FG))));
        }
        spans.extend(l.spans.iter().map(|s| (s.text.as_str(), attrs(&s.style))));
    }
    buf.set_rich_text(spans, &attrs(&Style::plain(palette::FG)), Shaping::Advanced, None);
    buf.shape_until_scroll(fonts, false);
}

impl Gpu {
    /// What sits under a point of the window, in the last frame drawn.
    pub fn target_at(&self, f: &Frame, x: f32, y: f32) -> Option<Target> {
        let (r, col) = self.cell_at(x, y)?;
        f.code.get(r)?.target_at(col).cloned()
    }

    /// The program line and column under a point; past a line's end, its end.
    pub fn pos_at(&self, f: &Frame, x: f32, y: f32) -> Option<crate::ast::Pos> {
        if y >= self.layout.bar_top || y < self.code_top {
            return None;
        }
        let r = ((y - self.code_top) / self.layout.code_line) as usize;
        let line = f.code.get(self.first + r).filter(|l| l.src > 0)?;
        let e = self.edges.get(r)?;
        let col = e.windows(2).position(|w| x < (w[0] + w[1]) / 2.0).unwrap_or(e.len().saturating_sub(1));
        Some(crate::ast::Pos { line: line.src, col: col.saturating_sub(line.off) })
    }

    /// The code row (counting from the first shown) and character under a point.
    fn cell_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        if y >= self.layout.bar_top || y < self.code_top {
            return None;
        }
        let r = ((y - self.code_top) / self.layout.code_line) as usize;
        let col = self.edges.get(r)?.windows(2).position(|w| x >= w[0] && x < w[1])?;
        Some((self.first + r, col))
    }
}

/// Character edges on an even grid, for rows the layout hasn't placed.
fn grid(line: &Line, l: Layout) -> Vec<f32> {
    let n: usize = line.spans.iter().map(|s| s.text.chars().count()).sum();
    (0..=n).map(|i| l.pad + i as f32 * l.cell).collect()
}

/// The left edge of every character of `text`, then its end, from the shaped glyphs. A glyph
/// covering several characters (a ligature) shares its width among them.
fn edges(text: &str, glyphs: &[glyphon::LayoutGlyph], width: f32) -> Vec<f32> {
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let mut out = vec![f32::NAN; starts.len() + 1];
    for g in glyphs {
        let chars: Vec<usize> = (0..starts.len()).filter(|&c| starts[c] >= g.start && starts[c] < g.end).collect();
        for (k, &c) in chars.iter().enumerate() {
            out[c] = g.x + g.w * k as f32 / chars.len() as f32;
        }
    }
    out[starts.len()] = width;
    // a character no glyph claimed starts where the next one does
    for i in (0..starts.len()).rev() {
        if out[i].is_nan() {
            out[i] = out[i + 1];
        }
    }
    out
}

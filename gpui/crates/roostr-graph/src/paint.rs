//! Drawing primitives for the graph: CSS colors, the world → screen view
//! transform, and world-space triangle meshes. The web page draws an SVG
//! `<g transform="translate(x,y) scale(s)">`, so every shape, stroke width and
//! label scales with the view; here each shape is tessellated once (per level
//! of detail) in world units and mapped to the screen per frame.

use gpui_kit::{Bounds, Hsla, Path, PathVertex, Pixels, Point, Rgba, Window, point, px, size};
use lyon::math::point as lpoint;
use lyon::path::Path as LPath;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, LineCap, StrokeOptions,
    StrokeTessellator, StrokeVertex, VertexBuffers,
};

/// `oklch(l c h)` (CSS Color 4) as an sRGB color, clamped to the gamut.
pub(crate) fn oklch(l: f64, c: f64, h_deg: f64) -> Hsla {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let r = 4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3;
    let g = -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701 * s3;
    let gamma = |c: f64| {
        let c = c.clamp(0., 1.);
        if c <= 0.003_130_8 {
            12.92 * c
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        }
    };
    Rgba {
        r: gamma(r) as f32,
        g: gamma(g) as f32,
        b: gamma(bl) as f32,
        a: 1.,
    }
    .into()
}

/// `#rrggbb` at `alpha`.
pub(crate) fn hex(rgb: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = gpui_kit::rgb(rgb).into();
    c.a = alpha;
    c
}

/// `color` with its alpha multiplied by `k` (SVG `*-opacity` / group opacity).
pub(crate) fn fade(color: Hsla, k: f64) -> Hsla {
    Hsla {
        a: color.a * k as f32,
        ..color
    }
}

/// World → screen: `translate(x, y) scale(s)`, relative to the stage origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub s: f64,
    pub x: f64,
    pub y: f64,
}

/// The view placed on the window: the stage's origin and bounds.
#[derive(Clone, Copy)]
pub(crate) struct Screen {
    pub view: View,
    pub bounds: Bounds<Pixels>,
}

impl Screen {
    pub fn project(&self, x: f64, y: f64) -> Point<Pixels> {
        point(
            self.bounds.origin.x + px((self.view.x + x * self.view.s) as f32),
            self.bounds.origin.y + px((self.view.y + y * self.view.s) as f32),
        )
    }

    /// Whether the world rectangle min..max is on screen.
    fn visible(&self, min: [f32; 2], max: [f32; 2]) -> bool {
        let a = self.project(min[0] as f64, min[1] as f64);
        let b = self.project(max[0] as f64, max[1] as f64);
        let r = self.bounds;
        a.x <= r.origin.x + r.size.width
            && b.x >= r.origin.x
            && a.y <= r.origin.y + r.size.height
            && b.y >= r.origin.y
    }
}

/// Level of detail for a scale: half-octave buckets, so curves are
/// re-flattened a handful of times over a zoom, not every frame.
pub(crate) fn lod(s: f64) -> i32 {
    (s.max(1e-4).log2() * 2.).round() as i32
}

/// Flattening tolerance (world units) of a level of detail: ~0.3 screen px.
pub(crate) fn tolerance(lod: i32) -> f32 {
    0.3 / 2f32.powf(lod as f32 / 2.)
}

/// A world-space triangle list.
#[derive(Default, Clone)]
pub(crate) struct Mesh {
    tris: Vec<[f32; 2]>,
    min: [f32; 2],
    max: [f32; 2],
}

impl Mesh {
    fn push_buffers(&mut self, buf: &VertexBuffers<[f32; 2], u32>) {
        if buf.indices.is_empty() {
            return;
        }
        if self.tris.is_empty() {
            self.min = [f32::MAX; 2];
            self.max = [f32::MIN; 2];
        }
        self.tris.reserve(buf.indices.len());
        for &i in &buf.indices {
            let v = buf.vertices[i as usize];
            self.min = [self.min[0].min(v[0]), self.min[1].min(v[1])];
            self.max = [self.max[0].max(v[0]), self.max[1].max(v[1])];
            self.tris.push(v);
        }
    }

    /// Append the fill of `path` (non-zero rule, as SVG glyph outlines and shapes fill).
    pub fn fill(&mut self, path: &LPath, tol: f32) {
        let mut buf: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
        let options =
            FillOptions::tolerance(tol).with_fill_rule(lyon::tessellation::FillRule::NonZero);
        let result = FillTessellator::new().tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buf, |v: FillVertex| v.position().to_array()),
        );
        if let Err(error) = result {
            log::debug!("graph fill tessellation failed: {error:?}");
        }
        self.push_buffers(&buf);
    }

    /// Append the stroke of `path`; `dash` is an SVG `stroke-dasharray`
    /// applied per subpath (each starts the pattern afresh).
    pub fn stroke(
        &mut self,
        path: &LPath,
        width: f32,
        round: bool,
        dash: Option<&[f32]>,
        tol: f32,
    ) {
        let dashed;
        let path = match dash {
            Some(pattern) => {
                dashed = dash_path(path, pattern, tol);
                &dashed
            }
            None => path,
        };
        let cap = if round { LineCap::Round } else { LineCap::Butt };
        let options = StrokeOptions::tolerance(tol)
            .with_line_width(width)
            .with_line_cap(cap);
        let mut buf: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
        let result = StrokeTessellator::new().tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buf, |v: StrokeVertex| v.position().to_array()),
        );
        if let Err(error) = result {
            log::debug!("graph stroke tessellation failed: {error:?}");
        }
        self.push_buffers(&buf);
    }

    /// Paint in `color` through the screen transform (skipped when off screen).
    pub fn paint(&self, screen: &Screen, color: Hsla, window: &mut Window) {
        if self.tris.is_empty() || color.a <= 0.002 || !screen.visible(self.min, self.max) {
            return;
        }
        let s = screen.view.s as f32;
        let ox = f32::from(screen.bounds.origin.x) + screen.view.x as f32;
        let oy = f32::from(screen.bounds.origin.y) + screen.view.y as f32;
        let map = |v: [f32; 2]| point(px(ox + v[0] * s), px(oy + v[1] * s));
        let mut path = Path::new(map(self.tris[0]));
        path.vertices = self
            .tris
            .iter()
            .map(|&v| PathVertex {
                xy_position: map(v),
                st_position: point(0., 1.),
                content_mask: Default::default(),
            })
            .collect();
        let min = map(self.min);
        let max = map(self.max);
        path.bounds = Bounds {
            origin: min,
            size: size(max.x - min.x, max.y - min.y),
        };
        window.paint_path(path, color);
    }
}

/// SVG dashing: split each subpath of `path` into the "on" runs of `pattern`.
fn dash_path(path: &LPath, pattern: &[f32], tol: f32) -> LPath {
    use lyon::algorithms::measure::{PathMeasurements, SampleType};
    let total: f32 = pattern.iter().sum();
    let mut out = LPath::builder();
    if total <= 0. {
        return path.clone();
    }
    // Subpaths one at a time: SVG restarts the dash pattern on each.
    let mut sub = LPath::builder();
    let flush = |sub: &mut lyon::path::path::Builder, out: &mut lyon::path::path::Builder| {
        let piece = std::mem::replace(sub, LPath::builder()).build();
        let measure = PathMeasurements::from_path(&piece, tol.min(0.05));
        let mut sampler = measure.create_sampler(&piece, SampleType::Distance);
        let length = sampler.length();
        let (mut pos, mut i) = (0f32, 0usize);
        while pos < length {
            let next = (pos + pattern[i % pattern.len()]).min(length);
            if i % 2 == 0 && next > pos {
                sampler.split_range(pos..next, out);
            }
            pos = next;
            i += 1;
        }
    };
    let mut open = false;
    for event in path.iter() {
        match event {
            lyon::path::Event::Begin { at } => {
                if open {
                    flush(&mut sub, &mut out);
                }
                sub.begin(at);
                open = true;
            }
            lyon::path::Event::Line { to, .. } => {
                sub.line_to(to);
            }
            lyon::path::Event::Quadratic { ctrl, to, .. } => {
                sub.quadratic_bezier_to(ctrl, to);
            }
            lyon::path::Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                sub.cubic_bezier_to(ctrl1, ctrl2, to);
            }
            lyon::path::Event::End { close, .. } => {
                sub.end(close);
            }
        }
    }
    if open {
        flush(&mut sub, &mut out);
    }
    out.build()
}

/// A path builder in world units (f64 in, lyon f32 out).
pub(crate) struct Shape(lyon::path::path::Builder);

impl Shape {
    pub fn new() -> Self {
        Self(LPath::builder())
    }

    /// `M a Q c b`: a quadratic curve from `a` through control `c` to `b`.
    pub fn quad(&mut self, a: (f64, f64), c: (f64, f64), b: (f64, f64)) -> &mut Self {
        self.0.begin(lpoint(a.0 as f32, a.1 as f32));
        self.0.quadratic_bezier_to(
            lpoint(c.0 as f32, c.1 as f32),
            lpoint(b.0 as f32, b.1 as f32),
        );
        self.0.end(false);
        self
    }

    /// A straight segment.
    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) -> &mut Self {
        self.0.begin(lpoint(a.0 as f32, a.1 as f32));
        self.0.line_to(lpoint(b.0 as f32, b.1 as f32));
        self.0.end(false);
        self
    }

    /// A full circle, starting at 3 o'clock and running clockwise on screen
    /// (where SVG starts a circle's dash pattern).
    pub fn circle(&mut self, c: (f64, f64), r: f64) -> &mut Self {
        self.0.add_circle(
            lpoint(c.0 as f32, c.1 as f32),
            r as f32,
            lyon::path::Winding::Positive,
        );
        self
    }

    pub fn build(self) -> LPath {
        self.0.build()
    }
}

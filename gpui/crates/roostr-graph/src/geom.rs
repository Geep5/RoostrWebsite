//! Drawing primitives for the graph: CSS colours, the world → screen view transform, and
//! world-space polylines. The web page draws an SVG `<g transform="translate(x,y) scale(s)">`,
//! so every shape, stroke width and label scales with the view; here the world is one Rune
//! element under a live offset and scale (the camera), its shapes flattened once per level of
//! detail in world units.

use rune::{Color, LineCap, Shape};

/// `oklch(l c h)` (CSS Color 4) as an sRGB colour, each channel clamped to the gamut.
pub(crate) fn oklch(l: f64, c: f64, h_deg: f64) -> Color {
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
    roostr_ui::color::from_srgb([gamma(r) as f32, gamma(g) as f32, gamma(bl) as f32], 1.)
}

/// `#rrggbb` at `alpha`.
pub(crate) fn hex(rgb: u32, alpha: f32) -> Color {
    roostr_ui::color::rgb(rgb).alpha(alpha)
}

/// `color` with its alpha multiplied by `k` (SVG `*-opacity` / group opacity).
pub(crate) fn fade(color: Color, k: f64) -> Color {
    color.alpha(color.a * k as f32)
}

/// World → screen: `translate(x, y) scale(s)`, relative to the stage's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub s: f64,
    pub x: f64,
    pub y: f64,
}

impl View {
    /// Where world point (x, y) shows on the stage.
    pub fn project(&self, x: f64, y: f64) -> (f64, f64) {
        (self.x + x * self.s, self.y + y * self.s)
    }

    /// The world point under stage point (x, y).
    pub fn world(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.x) / self.s, (y - self.y) / self.s)
    }
}

/// Level of detail for a scale: half-octave buckets, so curves are re-flattened a handful of
/// times over a zoom, not on every frame.
pub(crate) fn lod(s: f64) -> i32 {
    (s.max(1e-4).log2() * 2.).round() as i32
}

/// The scale a level of detail stands for.
pub(crate) fn lod_scale(lod: i32) -> f64 {
    2f64.powf(lod as f64 / 2.)
}

/// Flattening tolerance (world units) of a level of detail: ~0.3 screen px.
pub(crate) fn tolerance(lod: i32) -> f64 {
    0.3 / lod_scale(lod)
}

/// A polyline in world units.
pub(crate) type Poly = Vec<[f32; 2]>;

fn pt(p: (f64, f64)) -> [f32; 2] {
    [p.0 as f32, p.1 as f32]
}

/// `M a Q c b` flattened to within `tol`.
pub(crate) fn quad(a: (f64, f64), c: (f64, f64), b: (f64, f64), tol: f64) -> Poly {
    // A quadratic's chords of n even steps stray at most |a - 2c + b| / (4 n²).
    let dd = ((a.0 - 2. * c.0 + b.0).powi(2) + (a.1 - 2. * c.1 + b.1).powi(2)).sqrt();
    let n = ((dd / (4. * tol)).sqrt().ceil() as usize).clamp(1, 256);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1. - t;
            pt((
                u * u * a.0 + 2. * u * t * c.0 + t * t * b.0,
                u * u * a.1 + 2. * u * t * c.1 + t * t * b.1,
            ))
        })
        .collect()
}

/// A circle's outline (not repeating its first point), starting at 3 o'clock and running
/// clockwise on screen, where SVG starts a circle's dash pattern.
pub(crate) fn circle(c: (f64, f64), r: f64, tol: f64) -> Poly {
    let step = if r > tol {
        2. * (1. - tol / r).acos()
    } else {
        std::f64::consts::PI / 4.
    };
    let n = ((std::f64::consts::TAU / step).ceil() as usize).clamp(8, 512);
    (0..n)
        .map(|i| {
            let th = i as f64 / n as f64 * std::f64::consts::TAU;
            pt((c.0 + r * th.cos(), c.1 + r * th.sin()))
        })
        .collect()
}

/// SVG dashing of one subpath: the "on" runs of `pattern`, starting the pattern afresh.
/// `closed` runs back to the first point.
pub(crate) fn dash(poly: &[[f32; 2]], closed: bool, pattern: &[f32]) -> Vec<Poly> {
    let total: f32 = pattern.iter().sum();
    if poly.len() < 2 || total <= 0. {
        return vec![poly.to_vec()];
    }
    let mut ring: Vec<[f32; 2]> = poly.to_vec();
    if closed {
        ring.push(poly[0]);
    }
    let mut out = Vec::new();
    let mut cur: Poly = Vec::new();
    let (mut k, mut left) = (0usize, pattern[0]);
    for w in ring.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let mut at = 0f32;
        while at < len {
            let step = left.min(len - at);
            let lerp = |d: f32| {
                let t = if len > 0. { d / len } else { 0. };
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            };
            if k % 2 == 0 {
                if cur.is_empty() {
                    cur.push(lerp(at));
                }
                cur.push(lerp(at + step));
            }
            at += step;
            left -= step;
            if left <= 1e-6 {
                if k % 2 == 0 && cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                k += 1;
                left = pattern[k % pattern.len()];
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// World units per tile side: shapes are cut into canvases this size, so Rune skips the ones
/// off screen instead of emitting every segment of the graph each frame (zoomed in, the
/// object→property lines alone are hundreds of thousands of segments).
const TILE: f32 = 256.;

enum Op {
    Stroke {
        points: Poly,
        closed: bool,
        width: f32,
        cap: LineCap,
        color: Color,
    },
    Fill {
        points: Poly,
        color: Color,
    },
}

/// One tile's drawing and the world box it covers (strokes' widths included).
struct Tile {
    ops: Vec<Op>,
    min: [f32; 2],
    max: [f32; 2],
}

impl Tile {
    fn cover(&mut self, points: &[[f32; 2]], pad: f32) {
        for p in points {
            self.min = [self.min[0].min(p[0] - pad), self.min[1].min(p[1] - pad)];
            self.max = [self.max[0].max(p[0] + pad), self.max[1].max(p[1] + pad)];
        }
    }
}

fn tile_of(p: [f32; 2]) -> (i32, i32) {
    ((p[0] / TILE).floor() as i32, (p[1] / TILE).floor() as i32)
}

/// Shapes in world units, drawn as one canvas element per [`TILE`] they touch, each pinned
/// at its own bounding box in the world.
#[derive(Default)]
pub(crate) struct Pen {
    tiles: Vec<((i32, i32), Tile)>,
}

impl Pen {
    pub fn new() -> Self {
        Self::default()
    }

    fn tile(&mut self, at: (i32, i32)) -> &mut Tile {
        let i = match self.tiles.iter().position(|(k, _)| *k == at) {
            Some(i) => i,
            None => {
                self.tiles.push((
                    at,
                    Tile {
                        ops: Vec::new(),
                        min: [f32::MAX; 2],
                        max: [f32::MIN; 2],
                    },
                ));
                self.tiles.len() - 1
            }
        };
        &mut self.tiles[i].1
    }

    fn put(&mut self, at: (i32, i32), op: Op, pad: f32) {
        let tile = self.tile(at);
        match &op {
            Op::Stroke { points, .. } | Op::Fill { points, .. } => tile.cover(points, pad),
        }
        tile.ops.push(op);
    }

    /// An open polyline `width` world units wide, cut where it crosses into another tile
    /// (each piece ends where the next starts, so butt-capped pieces join seamlessly).
    pub fn stroke(&mut self, poly: &[[f32; 2]], width: f32, cap: LineCap, color: Color) {
        if color.a <= 0.002 || poly.len() < 2 {
            return;
        }
        let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
        let mut start = 0;
        let mut at = tile_of(mid(poly[0], poly[1]));
        for i in 1..poly.len() - 1 {
            let next = tile_of(mid(poly[i], poly[i + 1]));
            if next != at {
                self.put(
                    at,
                    Op::Stroke {
                        points: poly[start..=i].to_vec(),
                        closed: false,
                        width,
                        cap,
                        color,
                    },
                    width / 2.,
                );
                (start, at) = (i, next);
            }
        }
        self.put(
            at,
            Op::Stroke {
                points: poly[start..].to_vec(),
                closed: false,
                width,
                cap,
                color,
            },
            width / 2.,
        );
    }

    /// A closed outline `width` world units wide.
    pub fn ring(&mut self, poly: &[[f32; 2]], width: f32, color: Color) {
        if color.a <= 0.002 || poly.len() < 2 {
            return;
        }
        let (min, max) = bounds(poly);
        if tile_of(min) == tile_of(max) {
            let op = Op::Stroke {
                points: poly.to_vec(),
                closed: true,
                width,
                cap: LineCap::Butt,
                color,
            };
            self.put(tile_of(min), op, width / 2.);
        } else {
            let mut open = poly.to_vec();
            open.push(poly[0]);
            self.stroke(&open, width, LineCap::Butt, color);
        }
    }

    /// A filled disc of radius `r` at `c`: one round-capped stroke of no length, whose
    /// coverage Rune computes exactly, so a dot a pixel wide keeps its true weight.
    pub fn disc(&mut self, c: [f32; 2], r: f32, color: Color) {
        if color.a <= 0.002 {
            return;
        }
        let op = Op::Stroke {
            points: vec![c, c],
            closed: false,
            width: 2. * r,
            cap: LineCap::Round,
            color,
        };
        self.put(tile_of(c), op, r);
    }

    /// The inside of a closed outline (drawn whole, in the tile of its centre).
    pub fn fill(&mut self, poly: &[[f32; 2]], color: Color) {
        if color.a <= 0.002 || poly.len() < 3 {
            return;
        }
        let (min, max) = bounds(poly);
        let centre = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
        self.put(
            tile_of(centre),
            Op::Fill {
                points: poly.to_vec(),
                color,
            },
            1.,
        );
    }

    /// The canvases, each positioned in the world, in the order their tiles were first drawn.
    pub fn build(self) -> rune::El {
        let mut layer = rune::el();
        for (_, tile) in self.tiles {
            let origin = tile.min;
            let local = |points: Poly| -> Poly {
                points
                    .into_iter()
                    .map(|p| [p[0] - origin[0], p[1] - origin[1]])
                    .collect()
            };
            let mut shape = Shape::new();
            for op in tile.ops {
                shape = match op {
                    Op::Stroke {
                        points,
                        closed,
                        width,
                        cap,
                        color,
                    } => shape.stroke_with(local(points), closed, width, cap, color),
                    Op::Fill { points, color } => shape.fill(local(points), color),
                };
            }
            layer = layer.child(
                rune::canvas(shape)
                    .pin(Some(origin[0]), Some(origin[1]), None, None)
                    .size(tile.max[0] - origin[0], tile.max[1] - origin[1]),
            );
        }
        layer
    }
}

fn bounds(poly: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    poly.iter()
        .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashes_restart_per_subpath_and_follow_the_pattern() {
        let line = vec![[0., 0.], [20., 0.]];
        let runs = dash(&line, false, &[4., 2.]);
        let spans: Vec<(f32, f32)> = runs.iter().map(|r| (r[0][0], r[r.len() - 1][0])).collect();
        assert_eq!(spans, vec![(0., 4.), (6., 10.), (12., 16.), (18., 20.)]);
    }

    #[test]
    fn curves_flatten_finer_as_the_view_zooms_in() {
        let coarse = quad((0., 0.), (500., 800.), (1000., 0.), tolerance(lod(0.15)));
        let fine = quad((0., 0.), (500., 800.), (1000., 0.), tolerance(lod(2.3)));
        assert!(
            fine.len() > coarse.len(),
            "{} vs {}",
            fine.len(),
            coarse.len()
        );
        assert_eq!(coarse.first(), Some(&[0., 0.]));
        assert_eq!(fine.last(), Some(&[1000., 0.]));
    }

    #[test]
    fn oklch_matches_css() {
        let bytes = |c: Color| roostr_ui::color::to_srgb(c).map(|v| (v * 255.).round() as i32);
        // CSS red is oklch(0.62796 0.25768 29.2339); white has no chroma.
        let red = bytes(oklch(0.62796, 0.25768, 29.2339));
        assert!(
            (red[0] - 255).abs() <= 1 && red[1] <= 1 && red[2] <= 1,
            "{red:?}"
        );
        assert_eq!(bytes(oklch(1., 0., 0.)), [255, 255, 255]);
    }
}

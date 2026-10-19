//! World-space labels. The web draws them as SVG `<text>`: rotated along a
//! radius, anchored start/middle/end, `dominant-baseline="central"`,
//! letter-spaced, with `<tspan>` runs in different colors and fonts. Each
//! label becomes glyph outlines in world units, tessellated per level of
//! detail like every other shape. On macOS Core Text lays them out in the
//! fonts the web uses (SF Pro, SF Mono); elsewhere usvg's SVG text layout
//! does. Color-emoji glyphs are bitmaps: they are turned with the label and
//! painted centred where the glyph sits.

use std::sync::Arc;

use gpui_kit::{App, Bounds, RenderImage, Window, point, px, size};
use lyon::path::Path as LPath;

use crate::paint::{Mesh, Screen};

/// Load the label fonts off the main thread. The browser has one thread and
/// only the bundled fonts: they load on first use.
pub(crate) fn preload() {
    #[cfg(not(target_family = "wasm"))]
    let _ = std::thread::Builder::new()
        .name("roostr-graph-fonts".into())
        .spawn(backend::preload);
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    Sans,
    Mono,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    Start,
    Middle,
    End,
}

/// One `<tspan>`.
#[derive(Clone, PartialEq)]
pub(crate) struct SpanSpec {
    pub text: String,
    pub family: Family,
    pub size: f64,
    pub weight: u16,
    /// `dx` before the run (world units).
    pub dx: f64,
}

/// One `<text>`: at (x, y), rotated `rotate` degrees about that point.
#[derive(Clone, PartialEq)]
pub(crate) struct LabelSpec {
    pub x: f64,
    pub y: f64,
    pub anchor: Anchor,
    pub rotate: f64,
    /// `dominant-baseline="central"`, else the alphabetic baseline.
    pub central: bool,
    pub letter_spacing: f64,
    pub spans: Vec<SpanSpec>,
}

impl LabelSpec {
    /// A single-run label.
    pub fn plain(
        x: f64,
        y: f64,
        anchor: Anchor,
        family: Family,
        size: f64,
        weight: u16,
        text: impl Into<String>,
    ) -> Self {
        Self {
            x,
            y,
            anchor,
            rotate: 0.,
            central: false,
            letter_spacing: 0.,
            spans: vec![SpanSpec {
                text: text.into(),
                family,
                size,
                weight,
                dx: 0.,
            }],
        }
    }
}

/// A color emoji glyph of a label: its bitmap turned to the label's angle,
/// centred where the glyph sits.
pub(crate) struct Emoji {
    pub cx: f64,
    pub cy: f64,
    /// Box of the turned bitmap (world units).
    pub w: f64,
    pub h: f64,
    pub image: Arc<RenderImage>,
}

/// A laid-out label: per-span outlines (world units), emoji, hit box.
pub(crate) struct Label {
    pub spans: Vec<LPath>,
    pub emoji: Vec<Emoji>,
    /// Rotation centre and angle (radians) of the label's frame.
    origin: (f64, f64),
    angle: f64,
    /// Glyph cells in the unrotated frame: [x0, y0, x1, y1].
    cells: [f64; 4],
}

impl Label {
    /// Whether the world point is on the label's glyph cells (SVG text hit-testing).
    pub fn hit(&self, x: f64, y: f64) -> bool {
        let (dx, dy) = (x - self.origin.0, y - self.origin.1);
        let (sin, cos) = (-self.angle).sin_cos();
        let lx = self.origin.0 + dx * cos - dy * sin;
        let ly = self.origin.1 + dx * sin + dy * cos;
        let [x0, y0, x1, y1] = self.cells;
        lx >= x0 && lx <= x1 && ly >= y0 && ly <= y1
    }

    /// Tessellate each span's outlines at tolerance `tol`.
    pub fn meshes(&self, tol: f32) -> Vec<Mesh> {
        self.spans
            .iter()
            .map(|path| {
                let mut mesh = Mesh::default();
                mesh.fill(path, tol);
                mesh
            })
            .collect()
    }

    /// Paint the label's emoji (they keep their own colors, as SVG does).
    pub fn paint_emoji(&self, screen: &Screen, window: &mut Window, _cx: &mut App) {
        let s = screen.view.s;
        for e in &self.emoji {
            let c = screen.project(e.cx, e.cy);
            let (w, h) = (px((e.w * s) as f32), px((e.h * s) as f32));
            let bounds = Bounds {
                origin: point(c.x - w / 2., c.y - h / 2.),
                size: size(w, h),
            };
            if bounds.intersects(&screen.bounds) {
                let _ = window.paint_image(
                    bounds,
                    bounds,
                    Default::default(),
                    e.image.clone(),
                    0,
                    false,
                );
            }
        }
    }
}

/// Turn an emoji bitmap (RGBA, straight alpha) `angle` radians (clockwise
/// on screen) into a bitmap of its rotated bounding box, bilinear with
/// premultiplied alpha. Returns the image (BGRA, as gpui renders) and its
/// size in pixels relative to the source (dst / src width and height).
fn turn_emoji(src: &image::RgbaImage, angle: f64) -> Option<(Arc<RenderImage>, f64, f64)> {
    if src.width() == 0 || src.height() == 0 {
        return None;
    }
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let (sin, cos) = angle.sin_cos();
    let dw = (sw * cos.abs() + sh * sin.abs()).ceil().max(1.);
    let dh = (sw * sin.abs() + sh * cos.abs()).ceil().max(1.);
    let mut dst = image::RgbaImage::new(dw as u32, dh as u32);
    let texel = |x: i64, y: i64| -> [f64; 4] {
        if x < 0 || y < 0 || x >= sw as i64 || y >= sh as i64 {
            return [0.; 4];
        }
        let p = src.get_pixel(x as u32, y as u32).0;
        let a = p[3] as f64 / 255.;
        [p[0] as f64 * a, p[1] as f64 * a, p[2] as f64 * a, a]
    };
    for (x, y, out) in dst.enumerate_pixels_mut() {
        let (dx, dy) = (x as f64 + 0.5 - dw / 2., y as f64 + 0.5 - dh / 2.);
        // Inverse of the rotation, back into the source.
        let sx = cos * dx + sin * dy + sw / 2. - 0.5;
        let sy = -sin * dx + cos * dy + sh / 2. - 0.5;
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let mut acc = [0f64; 4];
        for (px_, py_, w) in [
            (x0, y0, (1. - fx) * (1. - fy)),
            (x0 + 1, y0, fx * (1. - fy)),
            (x0, y0 + 1, (1. - fx) * fy),
            (x0 + 1, y0 + 1, fx * fy),
        ] {
            let t = texel(px_, py_);
            for k in 0..4 {
                acc[k] += t[k] * w;
            }
        }
        let a = acc[3];
        *out = if a <= 1e-6 {
            image::Rgba([0, 0, 0, 0])
        } else {
            // BGRA, straight alpha.
            image::Rgba([
                (acc[2] / a).round() as u8,
                (acc[1] / a).round() as u8,
                (acc[0] / a).round() as u8,
                (a * 255.).round() as u8,
            ])
        };
    }
    let frame = image::Frame::new(dst);
    Some((Arc::new(RenderImage::new(vec![frame])), dw / sw, dh / sh))
}

/// Lay out every label. Blocking (font loading and shaping): call from a
/// background task.
pub(crate) fn layout_labels(specs: &[LabelSpec]) -> Vec<Label> {
    let mut labels: Vec<Label> = specs.iter().map(empty_label).collect();
    backend::outline(specs, &mut labels);
    for (label, spec) in labels.iter_mut().zip(specs) {
        finish_cells(label, spec);
    }
    labels
}

fn empty_label(spec: &LabelSpec) -> Label {
    Label {
        spans: vec![LPath::new(); spec.spans.len()],
        emoji: Vec::new(),
        origin: (spec.x, spec.y),
        angle: spec.rotate.to_radians(),
        cells: [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
    }
}

/// The label's glyph cells in its unrotated frame: the outline extents along
/// the baseline, one em tall (centred on a central baseline).
fn finish_cells(label: &mut Label, spec: &LabelSpec) {
    let (ox, oy) = label.origin;
    let (sin, cos) = (-label.angle).sin_cos();
    let mut cells = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut add = |x: f64, y: f64| {
        let (dx, dy) = (x - ox, y - oy);
        let lx = ox + dx * cos - dy * sin;
        let ly = oy + dx * sin + dy * cos;
        cells = [
            cells[0].min(lx),
            cells[1].min(ly),
            cells[2].max(lx),
            cells[3].max(ly),
        ];
    };
    for path in &label.spans {
        for p in path.iter().flat_map(|e| [e.from(), e.to()]) {
            add(p.x as f64, p.y as f64);
        }
    }
    for e in &label.emoji {
        add(e.cx - e.w / 2., e.cy);
        add(e.cx + e.w / 2., e.cy);
    }
    if cells[0] > cells[2] {
        return;
    }
    let em = spec.spans.iter().fold(0., |m: f64, s| m.max(s.size));
    let (y0, y1) = if spec.central {
        (oy - em / 2., oy + em / 2.)
    } else {
        (oy - em * 0.8, oy + em * 0.2)
    };
    label.cells = [cells[0], y0, cells[2], y1];
}

/// Core Text: SF Pro (the system font, `-apple-system`) and SF Mono, shaped
/// and outlined as the browser does.
#[cfg(target_os = "macos")]
mod backend {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use core_foundation::array::CFArray;
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::base::{CFRange, CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
    use core_graphics::color_space::{CGColorSpace, kCGColorSpaceSRGB};
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CG_AFFINE_TRANSFORM_IDENTITY, CGPoint};
    use core_graphics::path::{CGPathElementRef, CGPathElementType};
    use core_text::font::{self as ct_font, CTFont, kCTFontSystemFontType};
    use core_text::font_descriptor::{
        self as ct_descriptor, kCTFontCascadeListAttribute, kCTFontColorGlyphsTrait,
        kCTFontFamilyNameAttribute, kCTFontOrientationDefault, kCTFontTraitsAttribute,
        kCTFontWeightTrait,
    };
    use core_text::line::CTLine;
    use core_text::run::CTRun;
    use core_text::string_attributes::{kCTFontAttributeName, kCTKernAttributeName};
    use lyon::math::point as lpoint;
    use lyon::path::Path as LPath;
    use lyon::path::path::Builder;

    use super::{Anchor, Emoji, Family, Label, LabelSpec, turn_emoji};

    /// `.mono` (`ui-monospace, "SF Mono", Menlo, monospace`): the family
    /// names SF Mono goes by, then Menlo.
    const MONO: [&str; 3] = ["SF Mono", ".SF NS Mono", "Menlo"];
    /// Color emoji, preferred for any character it has (as browsers do).
    const EMOJI: &str = "Apple Color Emoji";
    /// Emoji are rasterized at this size (Apple Color Emoji's largest strike).
    const EMOJI_EM: f64 = 160.;

    /// Family, size (bits), CSS weight.
    type FontKey = (Family, u64, u16);

    /// One font per (family, size, weight).
    static FONTS: LazyLock<Mutex<HashMap<FontKey, CTFont>>> = LazyLock::new(Default::default);

    pub(super) fn preload() {
        font(Family::Sans, 17., 500);
        font(Family::Mono, 17., 400);
    }

    /// `kCTFontWeightTrait` (NSFontWeight) of a CSS weight.
    fn weight_trait(weight: u16) -> f64 {
        const TRAITS: [f64; 9] = [-0.8, -0.6, -0.4, 0., 0.23, 0.3, 0.4, 0.56, 0.62];
        TRAITS[(weight.clamp(100, 900) as usize + 50) / 100 - 1]
    }

    fn font(family: Family, size: f64, weight: u16) -> CTFont {
        let key = (family, size.to_bits(), weight);
        let mut fonts = FONTS.lock().unwrap_or_else(|e| e.into_inner());
        fonts
            .entry(key)
            .or_insert_with(|| match family {
                Family::Sans => {
                    let system =
                        ct_font::new_ui_font_for_language(kCTFontSystemFontType, size, None);
                    with_family(&system.family_name(), size, weight)
                }
                Family::Mono => MONO
                    .iter()
                    .find_map(|name| {
                        let font = with_family(name, size, weight);
                        (font.family_name() == *name).then_some(font)
                    })
                    .unwrap_or_else(|| with_family(MONO[0], size, weight)),
            })
            .clone()
    }

    fn with_family(family: &str, size: f64, weight: u16) -> CTFont {
        let (family_key, traits_key, weight_key, cascade_key) = unsafe {
            (
                CFString::wrap_under_get_rule(kCTFontFamilyNameAttribute),
                CFString::wrap_under_get_rule(kCTFontTraitsAttribute),
                CFString::wrap_under_get_rule(kCTFontWeightTrait),
                CFString::wrap_under_get_rule(kCTFontCascadeListAttribute),
            )
        };
        let traits =
            CFDictionary::from_CFType_pairs(&[(weight_key, CFNumber::from(weight_trait(weight)))]);
        // Emoji fall back to Apple Color Emoji, as in browsers; the system
        // font's own cascade picks its smaller UI variant.
        let emoji = ct_descriptor::new_from_attributes(&CFDictionary::from_CFType_pairs(&[(
            family_key.clone(),
            CFString::new(EMOJI).as_CFType(),
        )]));
        let attributes: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
            (family_key, CFString::new(family).as_CFType()),
            (traits_key, traits.as_CFType()),
            (cascade_key, CFArray::from_CFTypes(&[emoji]).as_CFType()),
        ]);
        ct_font::new_from_descriptor(&ct_descriptor::new_from_attributes(&attributes), size)
    }

    /// One `<tspan>` as a line: letter-spacing is a kern after every character.
    fn line(text: &str, font: &CTFont, letter_spacing: f64) -> CTLine {
        let mut string = CFMutableAttributedString::new();
        string.replace_str(&CFString::new(text), CFRange::init(0, 0));
        let range = CFRange::init(0, string.char_len());
        unsafe {
            string.set_attribute(range, kCTFontAttributeName, font);
            if letter_spacing != 0. {
                string.set_attribute(range, kCTKernAttributeName, &CFNumber::from(letter_spacing));
            }
        }
        CTLine::new_with_attributed_string(string.as_concrete_TypeRef() as _)
    }

    /// The font Core Text shaped a run with (the span's, or a fallback).
    fn run_font(run: &CTRun) -> Option<CTFont> {
        let key = unsafe { CFString::wrap_under_get_rule(kCTFontAttributeName) };
        run.attributes()?.find(&key)?.downcast::<CTFont>()
    }

    pub(super) fn outline(specs: &[LabelSpec], labels: &mut [Label]) {
        for (spec, label) in specs.iter().zip(labels) {
            outline_label(spec, label);
        }
    }

    fn outline_label(spec: &LabelSpec, label: &mut Label) {
        // Per span: its line, advance, and baseline below y (central: the
        // middle of the font's ascent and descent sits on y).
        let lines: Vec<(CTLine, f64, f64)> = spec
            .spans
            .iter()
            .map(|span| {
                let font = font(span.family, span.size, span.weight);
                let line = line(&span.text, &font, spec.letter_spacing);
                let width = line.get_typographic_bounds().width;
                let baseline = if spec.central {
                    (font.ascent() - font.descent()) / 2.
                } else {
                    0.
                };
                (line, width, baseline)
            })
            .collect();
        let total: f64 = lines
            .iter()
            .zip(&spec.spans)
            .map(|((_, w, _), s)| w + s.dx)
            .sum();
        let mut pen = match spec.anchor {
            Anchor::Start => 0.,
            Anchor::Middle => -total / 2.,
            Anchor::End => -total,
        };
        let (sin, cos) = spec.rotate.to_radians().sin_cos();
        // Label frame (y down, origin at (x, y)) to world, turned about (x, y).
        let world = |lx: f64, ly: f64| (spec.x + lx * cos - ly * sin, spec.y + lx * sin + ly * cos);
        for (k, (span, (line, width, baseline))) in spec.spans.iter().zip(&lines).enumerate() {
            pen += span.dx;
            let mut builder = LPath::builder();
            for run in line.glyph_runs().iter() {
                let Some(font) = run_font(&run) else {
                    continue;
                };
                let color = font.symbolic_traits() & kCTFontColorGlyphsTrait != 0;
                for (&glyph, at) in run.glyphs().iter().zip(run.positions().iter()) {
                    // Glyph origin in the label frame; glyph space is y up.
                    let (ox, oy) = (pen + at.x, baseline - at.y);
                    let map = |gx: f64, gy: f64| world(ox + gx, oy - gy);
                    if color {
                        label.emoji.extend(emoji(&font, glyph, label.angle, map));
                    } else {
                        glyph_outline(&font, glyph, map, &mut builder);
                    }
                }
            }
            label.spans[k] = builder.build();
            pen += width;
        }
    }

    fn glyph_outline(
        font: &CTFont,
        glyph: u16,
        map: impl Fn(f64, f64) -> (f64, f64),
        builder: &mut Builder,
    ) {
        let Ok(path) = font.create_path_for_glyph(glyph, &CG_AFFINE_TRANSFORM_IDENTITY) else {
            return;
        };
        let to = |p: &CGPoint| {
            let (x, y) = map(p.x, p.y);
            lpoint(x as f32, y as f32)
        };
        let state = RefCell::new((builder, false));
        let apply = |element: CGPathElementRef<'_>| {
            let (builder, open) = &mut *state.borrow_mut();
            let p = element.points();
            match element.element_type {
                CGPathElementType::MoveToPoint => {
                    if *open {
                        builder.end(true);
                    }
                    builder.begin(to(&p[0]));
                    *open = true;
                }
                _ if !*open => {}
                CGPathElementType::AddLineToPoint => {
                    builder.line_to(to(&p[0]));
                }
                CGPathElementType::AddQuadCurveToPoint => {
                    builder.quadratic_bezier_to(to(&p[0]), to(&p[1]));
                }
                CGPathElementType::AddCurveToPoint => {
                    builder.cubic_bezier_to(to(&p[0]), to(&p[1]), to(&p[2]));
                }
                CGPathElementType::CloseSubpath => {
                    builder.end(true);
                    *open = false;
                }
            }
        };
        path.apply(&apply);
        let (builder, open) = state.into_inner();
        if open {
            builder.end(true);
        }
    }

    /// Rasterize a color glyph at `EMOJI_EM` and turn it with the label.
    fn emoji(
        font: &CTFont,
        glyph: u16,
        angle: f64,
        map: impl Fn(f64, f64) -> (f64, f64),
    ) -> Option<Emoji> {
        let big = font.clone_with_font_size(EMOJI_EM);
        let rect = big.get_bounding_rects_for_glyphs(kCTFontOrientationDefault, &[glyph]);
        if rect.size.width <= 0. || rect.size.height <= 0. {
            return None;
        }
        // One pixel of margin keeps the edge texels for bilinear sampling.
        let (w, h) = (
            rect.size.width.ceil() as usize + 2,
            rect.size.height.ceil() as usize + 2,
        );
        let space = unsafe { CGColorSpace::create_with_name(kCGColorSpaceSRGB) }
            .unwrap_or_else(CGColorSpace::create_device_rgb);
        let mut context = CGContext::create_bitmap_context(
            None,
            w,
            h,
            8,
            w * 4,
            &space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
        );
        big.draw_glyphs(
            &[glyph],
            &[CGPoint::new(1. - rect.origin.x, 1. - rect.origin.y)],
            context.clone(),
        );
        let mut pixels = context.data().to_vec();
        for p in pixels.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let bitmap = image::RgbaImage::from_raw(w as u32, h as u32, pixels)?;
        let (image, kx, ky) = turn_emoji(&bitmap, angle)?;
        // Bitmap pixels to world units, and its centre in glyph space.
        let k = font.pt_size() / EMOJI_EM;
        let (cx, cy) = map(
            (rect.origin.x - 1. + w as f64 / 2.) * k,
            (rect.origin.y - 1. + h as f64 / 2.) * k,
        );
        Some(Emoji {
            cx,
            cy,
            w: w as f64 * k * kx,
            h: h as f64 * k * ky,
            image,
        })
    }
}

/// usvg: the labels as one SVG document, laid out with SVG text semantics.
/// SF Pro's outlines are not readable by its text engine (ttf-parser yields
/// no glyph outlines), so the closest system face with real weights stands in.
#[cfg(not(target_os = "macos"))]
mod backend {
    use std::fmt::Write as _;
    use std::sync::{Arc, LazyLock};

    use lyon::math::point as lpoint;
    use lyon::path::Path as LPath;
    use usvg::fontdb;
    use usvg::tiny_skia_path::{PathSegment, Point, Transform};

    use super::{Anchor, Emoji, Family, Label, LabelSpec, turn_emoji};

    /// `.sans` (`font-family: inherit` = `-apple-system`).
    #[cfg(not(target_family = "wasm"))]
    const SANS: &str = "Helvetica Neue";
    /// `.mono` (`ui-monospace, "SF Mono", Menlo, monospace`).
    #[cfg(not(target_family = "wasm"))]
    const MONO: &str = ".SF NS Mono";
    /// In the browser: the app's bundled UI and code fonts.
    #[cfg(target_family = "wasm")]
    const SANS: &str = roostr_core::theme::fonts::UI;
    #[cfg(target_family = "wasm")]
    const MONO: &str = roostr_core::theme::fonts::MONO;
    /// Color emoji, preferred for any character it has (as browsers do).
    #[cfg(not(target_family = "wasm"))]
    const EMOJI: &str = "Apple Color Emoji";
    /// In the browser: the bundled Twemoji, once it has been fetched.
    #[cfg(target_family = "wasm")]
    const EMOJI: &str = roostr_core::theme::fonts::EMOJI;
    /// Color-outline emoji are rasterized this many pixels across.
    const EMOJI_PX: f32 = 128.;

    static FONTS: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
        let mut db = fontdb::Database::new();
        #[cfg(not(target_family = "wasm"))]
        db.load_system_fonts();
        #[cfg(target_family = "wasm")]
        for data in roostr_core::theme::bundled_font_data() {
            db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
        }
        db.set_sans_serif_family(SANS);
        db.set_monospace_family(MONO);
        Arc::new(db)
    });

    /// The bundled fonts and the emoji font, built once it has arrived.
    #[cfg(target_family = "wasm")]
    static FONTS_WITH_EMOJI: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
        let mut db = (**FONTS).clone();
        if let Some(data) = roostr_core::theme::emoji_font_data() {
            db.load_font_source(fontdb::Source::Binary(Arc::new(data)));
        }
        Arc::new(db)
    });

    fn fonts() -> Arc<fontdb::Database> {
        #[cfg(target_family = "wasm")]
        if roostr_core::theme::emoji_font_data().is_some() {
            return FONTS_WITH_EMOJI.clone();
        }
        FONTS.clone()
    }

    /// The system font database takes a few hundred ms to load.
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn preload() {
        LazyLock::force(&FONTS);
    }

    fn has_char(db: &fontdb::Database, id: fontdb::ID, c: char) -> bool {
        db.with_face_data(id, |data, index| {
            ttf_parser::Face::parse(data, index)
                .ok()
                .and_then(|f| f.glyph_index(c))
                .is_some()
        })
        .unwrap_or(false)
    }

    /// usvg's default fallback takes the first face in the database that has
    /// the character (LastResort for emoji); browsers go to the emoji font.
    fn font_resolver() -> usvg::FontResolver<'static> {
        usvg::FontResolver {
            select_font: usvg::FontResolver::default_font_selector(),
            select_fallback: Box::new(|c, exclude, db| {
                let query = fontdb::Query {
                    families: &[fontdb::Family::Name(EMOJI)],
                    ..Default::default()
                };
                if let Some(id) = db.query(&query)
                    && !exclude.contains(&id)
                    && has_char(db, id, c)
                {
                    return Some(id);
                }
                (usvg::FontResolver::default_fallback_selector())(c, exclude, db)
            }),
        }
    }

    fn escape_char(c: char, out: &mut String) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }

    /// Continues an emoji cluster: presentation selector, keycap, skin tone, tag.
    fn extends_emoji(c: char) -> bool {
        matches!(c, '\u{FE0F}' | '\u{20E3}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}')
    }

    /// Write span text, putting each emoji cluster (a pictograph, or any
    /// character asking for emoji presentation with U+FE0F, ZWJ sequences
    /// included) in the emoji font, as browsers render them. Left to the text
    /// face, a text-default character such as 🏷️ shapes to its notdef glyph.
    fn write_text(s: &str, out: &mut String) {
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let emoji = c as u32 >= 0x1F000
                || chars
                    .get(i + 1)
                    .is_some_and(|n| *n == '\u{FE0F}' || *n == '\u{20E3}');
            if !emoji {
                escape_char(c, out);
                i += 1;
                continue;
            }
            let _ = write!(out, r#"<tspan font-family="'{EMOJI}'">"#);
            out.push(c);
            i += 1;
            while i < chars.len() {
                if extends_emoji(chars[i]) {
                    out.push(chars[i]);
                    i += 1;
                } else if chars[i] == '\u{200D}' && i + 1 < chars.len() {
                    out.push(chars[i]);
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    break;
                }
            }
            out.push_str("</tspan>");
        }
    }

    /// Lay out every label in one SVG document.
    pub(super) fn outline(specs: &[LabelSpec], labels: &mut [Label]) {
        let mut svg =
            String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">"#);
        for (i, spec) in specs.iter().enumerate() {
            let anchor = match spec.anchor {
                Anchor::Start => "start",
                Anchor::Middle => "middle",
                Anchor::End => "end",
            };
            let _ = write!(
                svg,
                r#"<text id="l{i}" x="{}" y="{}" text-anchor="{anchor}" letter-spacing="{}" transform="rotate({} {} {})"{}>"#,
                spec.x,
                spec.y,
                spec.letter_spacing,
                spec.rotate,
                spec.x,
                spec.y,
                if spec.central {
                    r#" dominant-baseline="central""#
                } else {
                    ""
                },
            );
            for (k, span) in spec.spans.iter().enumerate() {
                let family = match span.family {
                    Family::Sans => SANS,
                    Family::Mono => MONO,
                };
                // The fill is a sentinel naming the span; colors are applied at paint.
                let _ = write!(
                    svg,
                    r##"<tspan fill="#0000{:02x}" font-family="'{family}'" font-size="{}" font-weight="{}" dx="{}">"##,
                    k + 1,
                    span.size,
                    span.weight,
                    span.dx,
                );
                write_text(&span.text, &mut svg);
                svg.push_str("</tspan>");
            }
            svg.push_str("</text>");
        }
        svg.push_str("</svg>");

        let options = usvg::Options {
            fontdb: fonts(),
            font_family: SANS.into(),
            font_resolver: font_resolver(),
            ..Default::default()
        };
        let tree = match usvg::Tree::from_str(&svg, &options) {
            Ok(tree) => tree,
            Err(error) => {
                log::warn!("graph labels failed to lay out: {error}");
                return;
            }
        };
        walk(tree.root(), Transform::identity(), None, false, labels);
    }

    /// Collect outlines and emoji per label, accumulating group transforms.
    /// `glyphs`: `group` is a text's flattened glyphs, where a group is one
    /// color glyph (a bitmap, or COLR layers).
    fn walk(
        group: &usvg::Group,
        acc: Transform,
        label: Option<usize>,
        glyphs: bool,
        labels: &mut [Label],
    ) {
        let acc = acc.pre_concat(group.transform());
        let label = label.or_else(|| group.id().strip_prefix('l').and_then(|n| n.parse().ok()));
        for node in group.children() {
            match node {
                usvg::Node::Group(g)
                    if glyphs
                        && !g
                            .children()
                            .iter()
                            .any(|n| matches!(n, usvg::Node::Image(_))) =>
                {
                    if let Some(label) = label.and_then(|i| labels.get_mut(i))
                        && let Some(emoji) = color_outline(g, acc, label.angle)
                    {
                        label.emoji.push(emoji);
                    }
                }
                usvg::Node::Group(g) => walk(g, acc, label, false, labels),
                usvg::Node::Text(text) => {
                    let id = text
                        .id()
                        .strip_prefix('l')
                        .and_then(|n| n.parse().ok())
                        .or(label);
                    walk(text.flattened(), acc, id, true, labels);
                }
                usvg::Node::Path(path) => {
                    let Some(label) = label.and_then(|i| labels.get_mut(i)) else {
                        continue;
                    };
                    let Some(usvg::Paint::Color(c)) = path.fill().map(|f| f.paint()) else {
                        continue;
                    };
                    let span = (c.blue as usize).saturating_sub(1);
                    if span >= label.spans.len() {
                        continue;
                    }
                    let mut builder = LPath::builder();
                    for segment in path.data().segments() {
                        let map = |p: Point| {
                            let mut p = p;
                            acc.map_point(&mut p);
                            lpoint(p.x, p.y)
                        };
                        match segment {
                            PathSegment::MoveTo(p) => {
                                builder.begin(map(p));
                            }
                            PathSegment::LineTo(p) => {
                                builder.line_to(map(p));
                            }
                            PathSegment::QuadTo(c, p) => {
                                builder.quadratic_bezier_to(map(c), map(p));
                            }
                            PathSegment::CubicTo(c1, c2, p) => {
                                builder.cubic_bezier_to(map(c1), map(c2), map(p));
                            }
                            PathSegment::Close => builder.end(true),
                        }
                    }
                    let mut merged = LPath::builder();
                    merged.extend_from_paths(&[
                        label.spans[span].as_slice(),
                        builder.build().as_slice(),
                    ]);
                    label.spans[span] = merged.build();
                }
                usvg::Node::Image(image) => {
                    let Some(label) = label.and_then(|i| labels.get_mut(i)) else {
                        continue;
                    };
                    let usvg::ImageKind::PNG(data) = image.kind() else {
                        continue;
                    };
                    let Ok(bitmap) =
                        image::load_from_memory_with_format(data.as_ref(), image::ImageFormat::Png)
                    else {
                        continue;
                    };
                    let sz = image.size();
                    let corners = [
                        Point::from_xy(0., 0.),
                        Point::from_xy(sz.width(), 0.),
                        Point::from_xy(0., sz.height()),
                    ];
                    if let Some(emoji) = place(&bitmap.to_rgba8(), corners, acc, label.angle) {
                        label.emoji.push(emoji);
                    }
                }
            }
        }
    }

    /// A COLR glyph (`group`: its layers in font units, y up, placed and
    /// flipped by the group's transform), rasterized upright.
    fn color_outline(group: &usvg::Group, acc: Transform, angle: f64) -> Option<Emoji> {
        let bbox = group.bounding_box();
        let k = EMOJI_PX / bbox.width().max(bbox.height());
        if !k.is_finite() {
            return None;
        }
        let (w, h) = (
            (bbox.width() * k).ceil() as u32,
            (bbox.height() * k).ceil() as u32,
        );
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        // Font units to pixels: the glyph box's top left at the origin, y down.
        let raster = Transform::from_row(k, 0., 0., -k, -bbox.left() * k, bbox.bottom() * k);
        for child in group.children() {
            // `render_node` first moves the node's absolute layer box to the
            // origin; the layers' placement is `raster` alone.
            let Some(layer) = child.abs_layer_bounding_box() else {
                continue;
            };
            resvg::render_node(
                child,
                raster.pre_translate(layer.x(), layer.y()),
                &mut pixmap.as_mut(),
            );
        }
        let pixels = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let bitmap = image::RgbaImage::from_raw(w, h, pixels)?;
        let corners = [
            Point::from_xy(bbox.left(), bbox.bottom()),
            Point::from_xy(bbox.right(), bbox.bottom()),
            Point::from_xy(bbox.left(), bbox.top()),
        ];
        place(&bitmap, corners, acc.pre_concat(group.transform()), angle)
    }

    /// An emoji bitmap whose top-left, top-right and bottom-left corners are
    /// `corners` under `to_world`, turned with the label (`angle`).
    fn place(
        bitmap: &image::RgbaImage,
        mut corners: [Point; 3],
        to_world: Transform,
        angle: f64,
    ) -> Option<Emoji> {
        to_world.map_points(&mut corners);
        let [tl, tr, bl] = corners;
        let w = (((tr.x - tl.x).powi(2) + (tr.y - tl.y).powi(2)) as f64).sqrt();
        let h = (((bl.x - tl.x).powi(2) + (bl.y - tl.y).powi(2)) as f64).sqrt();
        // The glyph box turned with the label: its bitmap is resampled at that angle.
        let (image, kx, ky) = turn_emoji(bitmap, angle)?;
        Some(Emoji {
            cx: (tr.x + bl.x) as f64 / 2.,
            cy: (tr.y + bl.y) as f64 / 2.,
            w: w * kx,
            h: h * ky,
            image,
        })
    }
}

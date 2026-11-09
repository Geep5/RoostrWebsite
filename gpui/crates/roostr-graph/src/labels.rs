//! World-space labels. The web draws them as SVG `<text>`: rotated along a radius, anchored
//! start/middle/end, `dominant-baseline="central"`, letter-spaced, with `<tspan>` runs in
//! different colours, sizes and fonts. Here each label is a row of Rune text runs in a box
//! measured to fit them, placed in world units and turned about its anchor; the camera's live
//! scale magnifies it with the rest of the world, and Rune rasterizes it crisp at rest.

use rune::{Color, El, TextStyle, el, row, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    /// `-apple-system`: the app's UI font.
    Sans,
    /// `ui-monospace, "SF Mono", Menlo`.
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Anchor {
    Start,
    Middle,
    End,
}

/// One `<tspan>`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpanSpec {
    pub text: String,
    pub family: Family,
    pub size: f64,
    pub weight: u16,
    /// `dx` before the run (world units).
    pub dx: f64,
}

/// One `<text>`: at (x, y), rotated `rotate` degrees about that point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LabelSpec {
    pub x: f64,
    pub y: f64,
    pub anchor: Anchor,
    pub rotate: f64,
    /// `dominant-baseline="central"`, else the alphabetic baseline sits on y.
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

    fn em(&self) -> f64 {
        self.spans.iter().fold(0., |m: f64, s| m.max(s.size))
    }
}

/// How far the middle of the system font's ascent and descent sits above its alphabetic
/// baseline, in ems: where a `central` baseline puts the text (SF's 0.952 and 0.241).
const CENTRAL: f64 = 0.355;

/// The Rune text style of a span.
pub(crate) fn span_style(span: &SpanSpec, letter_spacing: f64) -> TextStyle {
    let style =
        TextStyle::new(span.size as f32, span.weight as f32).tracking(letter_spacing as f32);
    match span.family {
        Family::Sans => style,
        Family::Mono => style.family(rune::Family::SystemMono),
    }
}

/// A laid-out label: where it sits and the glyph cells it covers, for drawing and hit testing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Label {
    pub spec: LabelSpec,
    /// Each span's advance (world units).
    widths: Vec<f64>,
    /// Rotation centre and angle (radians) of the label's frame.
    angle: f64,
    /// Glyph cells in the unrotated frame: [x0, y0, x1, y1].
    cells: [f64; 4],
}

impl Label {
    /// Measure `spec`'s runs as Rune will set them.
    pub fn layout(spec: LabelSpec) -> Self {
        let widths: Vec<f64> = spec
            .spans
            .iter()
            .map(|s| {
                rune::text::measure(&s.text, &[], span_style(s, spec.letter_spacing), None).0 as f64
            })
            .collect();
        let total: f64 = widths.iter().zip(&spec.spans).map(|(w, s)| w + s.dx).sum();
        let x0 = match spec.anchor {
            Anchor::Start => spec.x,
            Anchor::Middle => spec.x - total / 2.,
            Anchor::End => spec.x - total,
        };
        let em = spec.em();
        let (y0, y1) = if spec.central {
            (spec.y - em / 2., spec.y + em / 2.)
        } else {
            (spec.y - em * 0.8, spec.y + em * 0.2)
        };
        Self {
            angle: spec.rotate.to_radians(),
            cells: [x0, y0, x0 + total, y1],
            widths,
            spec,
        }
    }

    /// Whether the world point is on the label's glyph cells (SVG text hit testing).
    pub fn hit(&self, x: f64, y: f64) -> bool {
        let (ox, oy) = (self.spec.x, self.spec.y);
        let (dx, dy) = (x - ox, y - oy);
        let (sin, cos) = (-self.angle).sin_cos();
        let lx = ox + dx * cos - dy * sin;
        let ly = oy + dx * sin + dy * cos;
        let [x0, y0, x1, y1] = self.cells;
        lx >= x0 && lx <= x1 && ly >= y0 && ly <= y1
    }

    /// The label in world units, its spans in `colors` (the last colour for any further span).
    pub fn el(&self, colors: &[Color]) -> El {
        let spec = &self.spec;
        let em = spec.em();
        let h = em * 1.6;
        let [x0, _, x1, _] = self.cells;
        // A pixel of slack so measurement rounding never wraps or clips the last glyph.
        let w = x1 - x0 + 2.;
        let left = match spec.anchor {
            Anchor::Start => x0,
            Anchor::Middle => x0 - 1.,
            Anchor::End => x0 - 2.,
        };
        let centre = if spec.central {
            spec.y
        } else {
            spec.y - CENTRAL * em
        };
        let pivot_x = (spec.x - left) / w;
        let mut line = row().w_full().h_full().items_center();
        line = match spec.anchor {
            Anchor::Start => line,
            Anchor::Middle => line.justify_center(),
            Anchor::End => line.justify_end(),
        };
        for (k, span) in spec.spans.iter().enumerate() {
            let color = colors
                .get(k)
                .or(colors.last())
                .copied()
                .unwrap_or(Color::WHITE);
            let mut run = text(span.text.clone())
                .font(span_style(span, spec.letter_spacing))
                .color(color);
            if span.dx != 0. {
                run = run.ml(span.dx as f32);
            }
            line = line.child(run);
        }
        let mut label = el()
            .pin(
                Some(left as f32),
                Some((centre - h / 2.) as f32),
                None,
                None,
            )
            .size(w as f32, h as f32)
            .child(line);
        if self.angle != 0. {
            label = label.origin(pivot_x as f32, 0.5).rotate(self.angle as f32);
        }
        label
    }

    /// The span advances, for tests.
    #[cfg(test)]
    pub fn width(&self) -> f64 {
        self.widths.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(anchor: Anchor, rotate: f64) -> Label {
        let mut spec = LabelSpec::plain(100., 50., anchor, Family::Sans, 20., 500, "Tasks");
        spec.central = true;
        spec.rotate = rotate;
        Label::layout(spec)
    }

    #[test]
    fn anchors_place_the_text_around_its_point() {
        let start = label(Anchor::Start, 0.);
        let w = start.width();
        assert!(w > 20., "measured {w}");
        assert!(start.hit(100. + w / 2., 50.));
        assert!(!start.hit(100. - w / 2., 50.));
        let end = label(Anchor::End, 0.);
        assert!(end.hit(100. - w / 2., 50.));
        assert!(!end.hit(100. + w / 2., 50.));
        let middle = label(Anchor::Middle, 0.);
        assert!(middle.hit(100. - w / 3., 50.) && middle.hit(100. + w / 3., 50.));
    }

    #[test]
    fn a_turned_label_is_hit_along_its_angle() {
        let down = label(Anchor::Start, 90.);
        let w = down.width();
        assert!(down.hit(100., 50. + w / 2.), "runs down the radius");
        assert!(!down.hit(100. + w / 2., 50.));
    }
}

//! Text helpers from `roostr_core::ui`: [`section_label`] and [`spaced_text`] (CSS
//! `letter-spacing`, which GPUI text lacked and Roostr placed glyph by glyph; Rune tracks text
//! itself).

use rune::{Color, El, Family, Span, TextStyle, row, text};

use crate::theme::{Theme, ty};
use crate::ty::Weights as _;

/// A section heading (`roostr_core::ui::section_label`): 11px, 600, uppercase, letter-spacing
/// 0.05em, muted.
pub fn section_label(t: &Theme, s: impl Into<String>) -> El {
    let s: String = s.into();
    row().child(
        spaced_text(s.to_uppercase(), 0.05)
            .size(ty::XS)
            .weight(ty::SEMIBOLD)
            .color(t.muted)
            .build(t),
    )
}

/// One line with CSS `letter-spacing` (`roostr_core::ui::SpacedText`): runs of text, each in
/// its own colour or the element's, `letter_spacing_em` × size after every character (the last
/// one too, as CSS does). Never wraps. Unset size is 14 (the window text size), colour `t.fg`,
/// line height 1.2 × size.
pub struct SpacedText {
    runs: Vec<(String, Option<Color>)>,
    spacing_em: f32,
    family: Option<Family>,
    size: f32,
    weight: f32,
    color: Option<Color>,
    line_height: Option<f32>,
}

/// `text` with `letter_spacing_em` × font size between characters.
pub fn spaced_text(s: impl Into<String>, letter_spacing_em: f32) -> SpacedText {
    SpacedText::new(letter_spacing_em).run(s, None)
}

impl SpacedText {
    /// Empty; add [`Self::run`]s.
    pub fn new(letter_spacing_em: f32) -> Self {
        SpacedText {
            runs: Vec::new(),
            spacing_em: letter_spacing_em,
            family: None,
            size: ty::MD,
            weight: ty::NORMAL,
            color: None,
            line_height: None,
        }
    }
    /// Append text; `color` `None` = the element's colour.
    pub fn run(mut self, s: impl Into<String>, color: Option<Color>) -> Self {
        self.runs.push((s.into(), color));
        self
    }
    pub fn family(mut self, f: Family) -> Self {
        self.family = Some(f);
        self
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = Some(lh);
        self
    }

    /// The text style without tracking.
    fn style(&self) -> TextStyle {
        let mut s = ty::ui(self.size).weight(self.weight);
        if let Some(lh) = self.line_height {
            s = s.lh(lh);
        }
        if let Some(f) = self.family {
            s = s.family(f);
        }
        s
    }

    pub fn build(self, t: &Theme) -> El {
        let style = self.style();
        let spacing = self.size * self.spacing_em;
        let color = self.color.unwrap_or(t.fg);
        let mut content = String::new();
        let mut spans = Vec::new();
        for (s, c) in &self.runs {
            let start = content.len();
            content.push_str(s);
            if let Some(c) = c
                && !s.is_empty()
            {
                spans.push(Span::new(start, content.len()).color(*c));
            }
        }
        let count = content.chars().count() as f32;
        // CSS: spacing after every character, the last one too; the box is the shaped line
        // plus that much, as Roostr's GPUI version sized it.
        let (w, _) = rune::text::measure(&content, &spans, style, None);
        let tracked = TextStyle {
            tracking: style.tracking + spacing,
            ..style
        };
        let mut e = text(content)
            .font(tracked)
            .color(color)
            .w(w + spacing * count);
        if !spans.is_empty() {
            e = e.spans(spans);
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use rune::{Env, Runtime, col};

    fn view(em: &f32, _: &mut rune::Cx<f32>) -> El {
        col().child(
            row().child(
                spaced_text("ROOSTR", *em)
                    .size(11.0)
                    .build(Theme::dark())
                    .key("t"),
            ),
        )
    }

    fn width(em: f32) -> f32 {
        let mut rt = Runtime::new(
            em,
            view,
            (400.0, 100.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("t"))
            .expect("text")
            .rect[2]
    }

    #[test]
    fn spaced_text_width_grows_with_spacing() {
        let (a, b) = (width(0.0), width(0.1));
        assert!((b - a - 6.0 * 1.1).abs() < 0.01, "{a} → {b}");
    }

    #[test]
    fn section_label_is_uppercase() {
        fn view(_: &(), _: &mut rune::Cx<()>) -> El {
            section_label(Theme::dark(), "Properties").key("s")
        }
        let mut rt = Runtime::new(
            (),
            view,
            (400.0, 100.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("PROPERTIES"))
        );
    }
}

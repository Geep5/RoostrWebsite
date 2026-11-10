//! Motion: the Svelte app's CSS timings as Rune curves and springs, plus Roostr's own touches
//! (press scale, bouncy popups, icon flights, rolling numbers). Every UI crate animates through
//! this module.
//!
//! - Tweens match the Svelte CSS (`transition: … 0.15s ease`); CSS `ease` is
//!   `cubic-bezier(0.25, 0.1, 0.25, 1)`, not an ease-out.
//! - Springs settle a gesture release ([`release_spring`]) and open popups, dialogs and sheets
//!   with a slight bounce ([`POP`], [`SHEET_OPEN`]); their exits stay instant.
//! - Tappable things shrink while held ([`pressable`]).
//! - An object's icon flies from where it was opened to the page it opens ([`IconFlight`]).
//! - Changing counts roll their digits ([`rolling_count`], [`rolling_text`]).
//! - Looping indicators (sync pulse, typing dots, spinners) use `El::looping`.
//! - Reduced motion is Rune's: spatial travel snaps and opacity, presence and colour take a
//!   short fade (`REDUCED_FADE`); press scale is off, nothing flies (the icons crossfade in
//!   place) and rolling digits crossfade. [`reduced`] reports it for code that picks other motion.
//! - Motion never gates input: menus, dialogs and sheets take focus and keys at once, and
//!   anything interactive leaves without an exit animation holding focus.
//!
//! Where an API takes an `Anim` (`El::opacity_motion`, `El::offset_motion`, `El::hover_motion`,
//! `Tx::motion_curve`) pass [`Tween::anim`]; where it takes a `Spring` (`El::motion`,
//! `Tx::color_motion`) pass [`Tween::spring`], a zero-bounce spring of the same length.

use rune::{Anim, Color, Curve, Easing, Effect, El, Spring, SpringPair, TextStyle, Transition, Tx};
use rune_kit::RollingText;

use crate::Theme;

/// CSS `ease`: cubic-bezier(0.25, 0.1, 0.25, 1).
const EASE: [f32; 4] = [0.25, 0.1, 0.25, 1.0];
/// The rail handle's cubic-bezier(0.22, 1, 0.36, 1).
const EASE_OUT_QUINT: [f32; 4] = [0.22, 1.0, 0.36, 1.0];

/// A duration and cubic-bezier curve: one of the Svelte app's CSS transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    /// Seconds.
    pub duration: f32,
    curve: [f32; 4],
}

impl Tween {
    /// `ms` milliseconds of CSS `ease`.
    pub const fn ease(ms: u32) -> Self {
        Self::bezier(ms, EASE)
    }

    /// `ms` milliseconds of cubic-bezier(`curve`).
    pub const fn bezier(ms: u32, curve: [f32; 4]) -> Self {
        Self {
            duration: ms as f32 / 1000.0,
            curve,
        }
    }

    /// The same curve over `seconds`.
    pub const fn with_duration(self, seconds: f32) -> Self {
        Self {
            duration: seconds,
            ..self
        }
    }

    pub const fn easing(self) -> Easing {
        let [x1, y1, x2, y2] = self.curve;
        Easing::cubic(x1, y1, x2, y2)
    }

    /// The tween as a Rune curve (CSS transition semantics: retargets add).
    pub const fn curve(self) -> Curve {
        Curve::tween(self.duration, self.easing())
    }

    /// The tween as an animation, for `El::opacity_motion`, `El::hover_motion`, `Tx::motion_curve`.
    pub const fn anim(self) -> Anim {
        Anim::new(self.curve())
    }

    /// A zero-bounce spring as long as the tween, for APIs that take only springs
    /// (`El::motion`, `El::token_motion`, `Tx::color_motion`).
    pub const fn spring(self) -> Spring {
        Spring::new(self.duration, 0.0)
    }
}

impl From<Tween> for Anim {
    fn from(t: Tween) -> Anim {
        t.anim()
    }
}

/// Hover reveal of row handles and actions (`opacity 0.1s ease`).
pub const HOVER_REVEAL: Tween = Tween::ease(100);
/// Fades: tooltips, drawer tabs, chevrons, menus appearing (`0.12s ease`).
pub const FADE: Tween = Tween::ease(120);
/// Controls: switch knob and track, section and sort arrows, hover colours (`0.15s ease`).
pub const CONTROL: Tween = Tween::ease(150);
/// The rail resize handle (`all 0.15s cubic-bezier(0.22, 1, 0.36, 1)`).
pub const HANDLE: Tween = Tween::bezier(150, EASE_OUT_QUINT);
/// Grid columns and the drawer tabs riding them (`0.16s ease`).
pub const COLUMNS: Tween = Tween::ease(160);
/// A bottom sheet settling back after a short drag (`transform 0.18s ease`).
pub const SHEET_SNAP: Tween = Tween::ease(180);
/// The editor's toggle-block disclosure (`transform 0.2s ease`).
pub const DISCLOSURE: Tween = Tween::ease(200);

/// The theme switch's colour spring (`Tx::color_motion`): Rune's calm token, as its own
/// settings window switches appearance.
pub const THEME_SWITCH: Spring = Spring::new(0.4, 0.0);

/// Menus, popovers, selects and dialogs opening: quick, with a slight bounce past their place.
pub const POP: Spring = Spring::new(0.3, 0.15);
/// A sheet rising open (the phone sheets, the settings sheet): a little longer than [`POP`],
/// with the same slight bounce as it lands.
pub const SHEET_OPEN: Spring = Spring::new(0.38, 0.12);
/// How far a held tappable shrinks ([`pressable`]).
pub const PRESS_SCALE: f32 = 0.97;
/// How far a held full-width row shrinks ([`pressable_row`]): a wide row at [`PRESS_SCALE`]
/// would pull its ends in by several points, so rows dip by about the same few points as a
/// card does.
pub const ROW_PRESS_SCALE: f32 = 0.985;
/// How small a popup starts as it pops open ([`pop`]).
const POP_SCALE: f32 = 0.96;

/// CSS `ease`, the Svelte app's default transition curve.
pub const fn css_ease() -> Easing {
    COLUMNS.easing()
}

/// Settling after a gesture lets go (a sheet flung open or shut): critically damped, settled
/// in about `seconds`.
pub const fn release_spring(seconds: f32) -> Spring {
    Spring::new(seconds, 0.0)
}

/// Whether motion snaps to its end state this commit (the OS "reduce motion" setting, or the
/// view's override).
pub fn reduced<S>(cx: &rune::Cx<S>) -> bool {
    cx.is_reduced_motion()
}

/// A popover, menu or dialog panel appearing: opacity 0 → 1 and a slight scale-up while it
/// slides `from_y` points into place, on [`POP`] (it overshoots a touch and settles). It takes
/// input from its first frame and leaves at once when closed (no exit animation holds focus).
/// Put it on the panel: `.transition(motion::appear(-4.0))`.
pub fn appear(from_y: f32) -> Transition {
    pop(Effect {
        offset: rune::Vec2::new(0.0, from_y),
        // Grows out of the side it comes from: the top when it drops, the bottom when it rises.
        anchor: rune::Vec2::new(0.5, if from_y > 0.0 { 1.0 } else { 0.0 }),
        ..Effect::scale(POP_SCALE).combined(Effect::opacity(0.0))
    })
}

/// `insertion` springing to rest on [`POP`], gone at once on removal: the one entrance every
/// Roostr popup, dialog and editor panel opens with.
pub fn pop(insertion: Effect) -> Transition {
    Transition {
        insertion,
        removal: Effect::IDENTITY,
        springs: SpringPair::new(POP, Spring::NONE),
        ..Transition::identity()
    }
}

/// A tappable element (row, card, tile, pill, menu item, button) that shrinks to
/// [`PRESS_SCALE`] about its centre while a press on it is held and springs back on release.
/// Layout and hit testing keep its resting size; Reduce Motion turns it off.
pub fn pressable(e: El) -> El {
    e.press_scale(PRESS_SCALE)
}

/// [`pressable`] for a full-width row (a list, sidebar or table row), on [`ROW_PRESS_SCALE`].
pub fn pressable_row(e: El) -> El {
    e.press_scale(ROW_PRESS_SCALE)
}

/// A count whose changed digits roll to the new value (up as it rises, down as it falls);
/// the first build shows it as is. `key` names the label and must be unique in the window.
pub fn rolling_count(
    t: &Theme,
    key: impl Into<String>,
    n: usize,
    font: TextStyle,
    color: Color,
) -> El {
    rolling_text(t, key, n.to_string(), n as f64, font, color)
}

/// [`rolling_count`] for a label carrying a number (`"7 objects"`, `"42%"`): the characters that
/// changed roll, in the direction `value` moved.
pub fn rolling_text(
    t: &Theme,
    key: impl Into<String>,
    text: impl Into<String>,
    value: f64,
    font: TextStyle,
    color: Color,
) -> El {
    RollingText::new(key, text)
        .value(value)
        .font(font)
        .color(color)
        .build(&t.rk)
}

/// The matched id (`El::matched`) of an object's icon at one place it shows: `place` names that
/// spot uniquely in the window and includes the object id (`"tree:<row key>"`,
/// `"table:<set>:<object>"`), so no two present icons share an id.
pub fn icon_id(place: &str) -> String {
    format!("obj-icon:{place}")
}

/// Shared-element flight of an object's icon from where it was opened (a sidebar row, a
/// pinned card, a table row, a phone tile) to the header of the page that opens. Keep one in
/// the app state.
///
/// - Every source icon goes through [`IconFlight::source`]; it carries its [`icon_id`] in each
///   commit, so the runtime knows where it was seen.
/// - The press that opens the object calls [`IconFlight::launch`] with that id before
///   navigating. In that commit the source icon remounts under another key without the id (the
///   matched element leaves) and the page header icon, through [`IconFlight::target`], mounts
///   with it: Rune flies the icon from the row to the header on its layout spring.
/// - Only the launched-from source drops the id, and only the target of the launched object
///   takes it, so one present element holds each id at a time.
/// - The app calls [`IconFlight::settle`] after each poll. A launch whose page no longer shows
///   ends there, so its source holds its id again and flies next time. A source a newer launch
///   replaced gets its id back there too, a commit after the old page left (getting it back in
///   the same commit would fly the old header's icon back to it).
/// - Under Reduce Motion nothing flies: the header icon appears in place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconFlight {
    /// The object launched and the matched id of the icon it flies from.
    launched: Option<(String, String)>,
    /// The source of the launch before, kept without its id until [`IconFlight::settle`].
    retired: Option<String>,
}

impl IconFlight {
    /// Nothing launched: what a view reads before any launch (`s.get::<IconFlight>()
    /// .unwrap_or(&IconFlight::IDLE)`).
    pub const IDLE: IconFlight = IconFlight {
        launched: None,
        retired: None,
    };

    /// The object `object` is opening from the icon matched as `from` (an [`icon_id`]), in the
    /// handler whose commit opens its page. The flight draws outside every clip until it lands
    /// (`Tx::unclipped_flights`): the header sits in the page's scroll viewport, whose clip would
    /// otherwise hide the icon for the stretch it crosses the sidebar.
    pub fn launch(&mut self, object: &str, from: String, tx: &mut Tx) {
        if let Some((_, old)) = self.launched.take()
            && old != from
        {
            self.retired = Some(old);
        }
        self.launched = Some((object.to_owned(), from));
        tx.unclipped_flights();
    }

    /// After a poll, once the commit of the last launch has been drawn: the retired source
    /// takes its id back, and the launch ends unless `shown(object)` says its page still shows.
    /// True when that changed what the views draw.
    pub fn settle(&mut self, shown: impl Fn(&str) -> bool) -> bool {
        let mut changed = self.retired.take().is_some();
        if self.launched.as_ref().is_some_and(|(o, _)| !shown(o)) {
            self.launched = None;
            changed = true;
        }
        changed
    }

    /// Whether a launch is under way (its page still shows): commits then may match its
    /// icon again and should let those flights out of clips too (`Tx::unclipped_flights`).
    pub fn active(&self) -> bool {
        self.launched.is_some()
    }

    /// The matched id `object`'s page header icon takes, if it was launched.
    pub fn launched(&self, object: &str) -> Option<&str> {
        self.launched
            .as_ref()
            .filter(|(o, _)| o == object)
            .map(|(_, id)| id.as_str())
    }

    /// A source icon at `id` ([`icon_id`]): keyed and matched there, or, when its object was
    /// launched from here (or the launch before was), keyed apart without the id.
    pub fn source(&self, icon: El, id: &str) -> El {
        let launched_here = self.launched.as_ref().is_some_and(|(_, from)| from == id)
            || self.retired.as_deref() == Some(id);
        if launched_here {
            icon.key(format!("{id}#launched"))
        } else {
            icon.key(id).matched(id)
        }
    }

    /// The page header icon of `object`, keyed by `key` (stable for that object). When `object`
    /// was launched, it takes the source's id, keyed by it so it mounts fresh and flies in.
    pub fn target(&self, icon: El, object: &str, key: &str) -> El {
        match self.launched(object) {
            Some(id) => icon.key(format!("{key}<{id}")).matched(id),
            None => icon.key(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_ease_like_css() {
        // CSS `ease` is past 80% at the halfway point; an ease-out would be at 87.5%.
        let half = CONTROL.easing().eval(0.5);
        assert!((0.80..0.82).contains(&half), "{half}");
        assert_eq!(CONTROL.easing().eval(1.0), 1.0);
        assert_eq!(CONTROL.duration, 0.15);
        assert_eq!(HANDLE.duration, 0.15);
        assert_eq!(DISCLOSURE.duration, 0.2);
    }

    #[test]
    fn tweens_become_rune_curves() {
        match FADE.curve() {
            Curve::Tween { duration, .. } => assert_eq!(duration, 0.12),
            other => panic!("{other:?}"),
        }
        assert_eq!(SHEET_SNAP.spring().duration, 0.18);
    }

    /// A sidebar of one row whose 20 pt icon opens a page with a 40 pt header icon.
    #[derive(Default)]
    struct App {
        flight: IconFlight,
        open: bool,
    }

    const FROM: [f32; 4] = [0.0, 0.0, 20.0, 20.0];
    const TO: [f32; 4] = [100.0, 0.0, 40.0, 40.0];

    fn view(s: &App, _: &mut rune::Cx<App>) -> El {
        let id = icon_id("tree:x");
        let icon = |side: f32| rune::el().size(side, side).fill(Color::WHITE).radius(4.0);
        let sidebar = rune::el()
            .key("sidebar")
            .size(100.0, 200.0)
            .child(s.flight.source(icon(20.0), &id));
        let mut page = rune::el().key("page").size(200.0, 200.0);
        if s.open {
            page = page.child(s.flight.target(icon(40.0), "x", "page-icon"));
        }
        rune::row().size(300.0, 200.0).child(sidebar).child(page)
    }

    fn open(reduced: bool) -> (rune::Runtime<App>, String) {
        let mut rt = rune::Runtime::new(
            App::default(),
            view,
            (300.0, 200.0),
            rune::Env::new(true, 1.0),
        );
        rt.set_system_reduced_motion(reduced);
        rt.frame(0.0);
        let id = icon_id("tree:x");
        assert_eq!(rt.presented(&id).expect("source").0, FROM);
        rt.update(|s, tx| {
            s.flight.launch("x", icon_id("tree:x"), tx);
            s.open = true;
        });
        rt.frame(1.0 / 120.0);
        (rt, format!("page-icon<{id}"))
    }

    #[test]
    fn an_opened_objects_icon_flies_from_its_row_to_the_header() {
        let (mut rt, target) = open(false);
        let first = rt.presented(&target).expect("target").0;
        assert!(
            (first[0] - FROM[0]).abs() < 8.0 && (first[2] - FROM[2]).abs() < 8.0,
            "starts where the row icon was: {first:?}"
        );
        // The row keeps its icon, under its launched key and without the id.
        assert!(rt.presented("obj-icon:tree:x#launched").is_some());
        for _ in 0..240 {
            rt.frame(1.0 / 120.0);
        }
        assert_eq!(rt.presented(&target).expect("target").0, TO);
    }

    #[test]
    fn under_reduced_motion_the_header_icon_appears_in_place() {
        let (rt, target) = open(true);
        assert_eq!(rt.presented(&target).expect("target").0, TO);
    }

    #[test]
    fn only_the_launched_object_takes_the_id() {
        let mut f = IconFlight::default();
        f.launch("x", icon_id("tree:x"), &mut Tx::default());
        assert_eq!(f.launched("x"), Some("obj-icon:tree:x"));
        assert_eq!(f.launched("y"), None);
    }

    /// Whether a source icon at `place` holds its id when `f` draws it.
    fn holds_id(f: &IconFlight, place: &str) -> bool {
        fn view(s: &(IconFlight, String), _: &mut rune::Cx<(IconFlight, String)>) -> El {
            s.0.source(rune::el().size(10.0, 10.0), &s.1)
        }
        let id = icon_id(place);
        let mut rt = rune::Runtime::new(
            (f.clone(), id.clone()),
            view,
            (20.0, 20.0),
            rune::Env::new(true, 1.0),
        );
        rt.frame(0.0);
        rt.presented(&id).is_some()
    }

    #[test]
    fn a_replaced_source_gets_its_id_back_a_poll_later() {
        let mut f = IconFlight::default();
        f.launch("x", icon_id("tree:x"), &mut Tx::default());
        f.launch("y", icon_id("tree:y"), &mut Tx::default());
        // The old page's header leaves in this commit: the old source must not take the id yet.
        assert!(!holds_id(&f, "tree:x"));
        assert!(!holds_id(&f, "tree:y"));
        assert!(f.settle(|o| o == "y"));
        assert!(holds_id(&f, "tree:x"));
        assert_eq!(f.launched("y"), Some("obj-icon:tree:y"));
        assert!(!f.settle(|o| o == "y"), "nothing left to settle");
    }

    #[test]
    fn a_launch_ends_when_its_page_goes_so_the_source_flies_again() {
        let mut f = IconFlight::default();
        f.launch("x", icon_id("tree:x"), &mut Tx::default());
        assert!(f.settle(|_| false));
        assert_eq!(f, IconFlight::IDLE);
        assert!(holds_id(&f, "tree:x"));
    }
}

//! The graph page (`routes/app/graph/+page.svelte`): the view state (world →
//! screen transform, hover/pin, level), pointer handling (drag pans, scroll
//! and pinch zoom about the cursor, click pins, double-click opens), the
//! eased zoom, the finder and the layered drawing.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::nav::{self, Route};
use roostr_core::relations::{current_space_id, space_relations};
use roostr_core::{Navigator, Store};
use web_time::Instant;

use crate::model::build_schema_graph;
use crate::paint::{View, hex, lod};
use crate::panel;
use crate::scene::{
    Built, Derived, Frame, Geo, LabelGeo, Labels, Layers, Sel, SelGeo, label_specs, prop_radius,
};
use crate::text::layout_labels;

/// The levels: pill label and the hint under it.
pub(crate) const LEVELS: [(&str, &str); 3] = [
    ("Types", "Types as clusters, sized by object count"),
    (
        "Schema",
        "Properties appear; lines show which types use them",
    ),
    ("Objects", "Individual objects and their values"),
];
/// Object labels show at most this many at once.
const LABEL_CAP: usize = 600;
/// Eased zoom duration.
const ZOOM_MS: f64 = 700.;

/// A finder result.
#[derive(Clone)]
pub(crate) struct Found {
    pub is_obj: bool,
    pub name: String,
    pub sub: String,
    pub color: Hsla,
    pub sel: Sel,
    pub x: f64,
    pub y: f64,
}

struct Anim {
    from: View,
    t0: Instant,
    s2: f64,
    /// World point that starts at the screen centre, and where it ends.
    fc: (f64, f64),
    to: (f64, f64),
}

struct Drag {
    x: f32,
    y: f32,
    vx: f64,
    vy: f64,
    moved: bool,
}

/// The per-space schema graph page.
pub struct GraphView {
    /// `?focus=<objectId>`: pin that object and zoom to it ("show in graph").
    focus: Option<String>,
    /// (space, default space) the graph was built for.
    built_for: Option<(String, String)>,
    /// Lite: the space was held whole when built (else it has placeholders).
    built_held: bool,
    built: Option<Rc<Built>>,
    labels: Option<Rc<Labels>>,
    label_texts: Vec<String>,
    label_task: Option<Task<()>>,
    status: SharedString,
    load: Option<Task<()>>,
    hover: Option<Sel>,
    pin: Option<Sel>,
    level: usize,
    v: View,
    /// Stage size and window origin, from the last layout.
    stage: Size<Pixels>,
    origin: Rc<Cell<Point<Pixels>>>,
    touched: bool,
    /// A built graph waits for the stage size to fit (and focus).
    pending_fit: bool,
    anim: Option<Anim>,
    drag: Option<Drag>,
    dragging: bool,
    label_objs: Vec<(usize, usize)>,
    label_timer: Option<Task<()>>,
    query: Entity<InputState>,
    found: Vec<Found>,
    /// The query `found` was computed for.
    found_for: Option<String>,
    hit: usize,
    geo: Option<Rc<Geo>>,
    label_geo: Option<Rc<LabelGeo>>,
    sel_geo: Option<Rc<SelGeo>>,
    _subs: Vec<Subscription>,
}

impl GraphView {
    /// The graph of the active space; `focus` pins that object and zooms to it.
    pub fn new(focus: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find an object or property…"));
        let store = Store::global(cx);
        let subs = vec![
            cx.observe(&store, |this, _, cx| this.sync(cx)),
            cx.observe(&Navigator::global(cx), |this, _, cx| this.sync(cx)),
            cx.subscribe(&query, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.refind(false, cx);
                }
            }),
        ];
        let mut this = Self {
            focus: focus.filter(|f| !f.is_empty()),
            built_for: None,
            built_held: true,
            built: None,
            labels: None,
            label_texts: Vec::new(),
            label_task: None,
            status: "Loading graph…".into(),
            load: None,
            hover: None,
            pin: None,
            level: 0,
            v: View {
                s: 0.3,
                x: 0.,
                y: 0.,
            },
            stage: Size::default(),
            origin: Rc::new(Cell::new(Point::default())),
            touched: false,
            pending_fit: false,
            anim: None,
            drag: None,
            dragging: false,
            label_objs: Vec::new(),
            label_timer: None,
            query,
            found: Vec::new(),
            found_for: None,
            hit: 0,
            geo: None,
            label_geo: None,
            sel_geo: None,
            _subs: subs,
        };
        // Labels laid out before the browser's emoji font arrived lack emoji.
        #[cfg(target_family = "wasm")]
        this._subs.push(
            cx.observe_global::<roostr_core::theme::EmojiFontLoaded>(|this, cx| {
                this.label_texts.clear();
                this.relabel(cx);
            }),
        );
        this.sync(cx);
        this
    }

    // ------------------------------------------------------------ building

    /// Built once per space (and once the definitions first load): rebuilding
    /// on every sync refresh would reset the view and drop what is pinned.
    /// Store changes still re-render the panel and relabel renamed properties.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (loaded, default) = {
            let store = Store::global(cx).read(cx);
            (
                store.loaded,
                store
                    .channels
                    .first()
                    .map(|c| c.id.clone())
                    .unwrap_or_default(),
            )
        };
        let channel = current_space_id(cx);
        if !loaded || channel.is_empty() {
            cx.notify();
            return;
        }
        // Lite: a space finishing its download rebuilds with real nodes.
        let held = roostr_core::lite_ui::space_held(&channel, cx);
        let key = (channel.clone(), default.clone());
        if self.built_for.as_ref() != Some(&key) || self.built_held != held {
            self.built_for = Some(key);
            self.built_held = held;
            self.build(channel.clone(), channel == default, cx);
        }
        self.relabel(cx);
        cx.notify();
    }

    fn build(&mut self, channel: String, is_default: bool, cx: &mut Context<Self>) {
        let store = Store::global(cx);
        let (relations, types, rows) = {
            let s = store.read(cx);
            let relations: Vec<_> = space_relations(&s.relations, &channel)
                .into_iter()
                .cloned()
                .collect();
            (
                relations,
                s.types.clone(),
                s.query_all(serde_json::json!({}), cx),
            )
        };
        // Lite: the directory's per-type counts give the not-loaded placeholders.
        let counts = match roostr_core::lite::status(cx) {
            Some(status) if !status.is_full(&channel) => roostr_core::lite::lite(cx)
                .map(|l| l.type_counts(&channel))
                .unwrap_or_default(),
            _ => Default::default(),
        };
        self.status = "Loading graph…".into();
        self.load = Some(cx.spawn(async move |this, cx| {
            let result = match rows.await {
                Ok(rows) => Ok(cx
                    .background_spawn(async move {
                        build_schema_graph(rows, &channel, is_default, &relations, &types, &counts)
                    })
                    .await),
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(g) => {
                        this.status = if g.types.is_empty() {
                            "Nothing in this space yet.".into()
                        } else {
                            SharedString::default()
                        };
                        this.built = Some(Rc::new(Built::new(g)));
                        this.hover = None;
                        this.pin = None;
                        this.geo = None;
                        this.sel_geo = None;
                        this.labels = None;
                        this.label_geo = None;
                        this.label_texts.clear();
                        this.label_objs.clear();
                        this.pending_fit = true;
                        this.relabel(cx);
                        this.refind(true, cx);
                        this.try_fit();
                    }
                    Err(error) => this.status = format!("Graph failed: {error}").into(),
                }
                cx.notify();
            });
        }));
    }

    /// Lay the world-space labels out again when their texts change (a
    /// property renamed, the graph rebuilt). Text layout runs off the main thread.
    fn relabel(&mut self, cx: &mut Context<Self>) {
        let Some(built) = self.built.clone() else {
            return;
        };
        let channel = self
            .built_for
            .as_ref()
            .map(|k| k.0.clone())
            .unwrap_or_default();
        let texts: Vec<String> = {
            let store = Store::global(cx).read(cx);
            let rels = space_relations(&store.relations, &channel);
            built
                .g
                .props
                .iter()
                .map(|p| match rels.iter().find(|r| r.key == p.key) {
                    Some(r) => panel::prop_label(r),
                    None => p.name.clone(),
                })
                .collect()
        };
        if texts == self.label_texts && (self.labels.is_some() || self.label_task.is_some()) {
            return;
        }
        self.label_texts = texts.clone();
        let specs = label_specs(&built.g, &texts);
        let (types, props) = (built.g.types.len(), built.g.props.len());
        self.label_task = Some(cx.spawn(async move |this, cx| {
            let labels = cx
                .background_spawn(async move { layout_labels(&specs) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.built.as_ref().is_some_and(|b| Rc::ptr_eq(b, &built)) {
                    this.labels = Some(Rc::new(Labels::split(labels, types, props)));
                    this.label_geo = None;
                }
                this.label_task = None;
                cx.notify();
            });
        }));
    }

    // ---------------------------------------------------------------- view

    fn wide(&self) -> bool {
        f32::from(self.stage.width) > 900.
    }

    /// Screen x of the world origin: left of centre when the side panel takes the right edge.
    fn centre_x(&self) -> f64 {
        let w = f32::from(self.stage.width) as f64;
        if self.wide() { (w - 300.) / 2. } else { w / 2. }
    }

    fn fit_view(&self) -> View {
        let (w, h) = (
            f32::from(self.stage.width) as f64,
            f32::from(self.stage.height) as f64,
        );
        let span = f64::min(if self.wide() { w - 360. } else { w * 0.95 }, h);
        let extent = self.built.as_ref().map_or(1750., |b| b.g.extent);
        View {
            s: f64::max(0.02, span / (2. * extent)),
            x: self.centre_x(),
            y: h / 2.,
        }
    }

    fn apply_view(&mut self, cx: &mut Context<Self>) {
        let s = self.v.s;
        self.level = if s < 0.44 {
            0
        } else if s < 1.65 {
            1
        } else {
            2
        };
        self.schedule_labels(cx);
        cx.notify();
    }

    /// Fit once a built graph and the stage size are both known; `?focus`
    /// then pins its object and zooms to it.
    fn try_fit(&mut self) -> bool {
        if !self.pending_fit
            || self.built.is_none()
            || self.stage.width <= px(0.)
            || self.stage.height <= px(0.)
        {
            return false;
        }
        self.pending_fit = false;
        self.touched = false;
        self.v = self.fit_view();
        if let (Some(focus), Some(b)) = (self.focus.clone(), self.built.clone())
            && let Some(&(t, j)) = b.obj_index.get(&focus)
        {
            self.pin = Some(Sel::Obj(t, j));
            let o = b.object(t, j);
            self.zoom_to(2.3, o.x, o.y);
        }
        true
    }

    /// The stage was laid out at `bounds`: refit until the user takes over.
    fn set_stage(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if size == self.stage {
            return;
        }
        self.stage = size;
        if !self.try_fit() && !self.touched {
            self.v = self.fit_view();
        }
        self.apply_view(cx);
    }

    /// Eased zoom to scale `s2` centred on world point (cx, cy).
    fn zoom_to(&mut self, s2: f64, cx: f64, cy: f64) {
        self.touched = true;
        let from = self.v;
        let h = f32::from(self.stage.height) as f64;
        let fc = (
            (self.centre_x() - from.x) / from.s,
            (h / 2. - from.y) / from.s,
        );
        self.anim = Some(Anim {
            from,
            t0: Instant::now(),
            s2,
            fc,
            to: (cx, cy),
        });
    }

    /// Advance the zoom animation; true while it runs.
    fn step_anim(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(anim) = &self.anim else { return false };
        let k = (anim.t0.elapsed().as_secs_f64() * 1000. / ZOOM_MS).min(1.);
        let e = if k < 0.5 {
            4. * k * k * k
        } else {
            1. - (-2. * k + 2.).powi(3) / 2.
        };
        let s = (anim.from.s.ln() + (anim.s2.ln() - anim.from.s.ln()) * e).exp();
        let h = f32::from(self.stage.height) as f64;
        let wx = anim.fc.0 + (anim.to.0 - anim.fc.0) * e;
        let wy = anim.fc.1 + (anim.to.1 - anim.fc.1) * e;
        self.v = View {
            s,
            x: self.centre_x() - wx * s,
            y: h / 2. - wy * s,
        };
        if k >= 1. {
            self.anim = None;
        }
        self.apply_view(cx);
        self.anim.is_some()
    }

    fn go_level(&mut self, l: usize, cx: &mut Context<Self>) {
        let Some(b) = self.built.clone() else { return };
        let fit = self.fit_view();
        let d = b.derive(self.sel());
        match l {
            0 => self.zoom_to(fit.s, 0., 0.),
            1 => self.zoom_to(f64::max(0.62, fit.s * 1.9), 0., 0.),
            _ => {
                if let Some(o) = d.sel_obj_ref(&b) {
                    self.zoom_to(2.3, o.x, o.y);
                } else {
                    let t =
                        d.sel_type.map(|t| &b.g.types[t]).or_else(|| {
                            b.g.types.iter().fold(
                                None,
                                |best: Option<&crate::model::GraphType>, t| match best {
                                    Some(a) if t.objects.len() <= a.objects.len() => Some(a),
                                    _ => Some(t),
                                },
                            )
                        });
                    if let Some(t) = t {
                        self.zoom_to(2.3, t.x, t.y);
                    }
                }
            }
        }
        cx.notify();
    }

    fn schedule_labels(&mut self, cx: &mut Context<Self>) {
        self.label_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let _ = this.update(cx, |this, cx| this.update_labels(cx));
        }));
    }

    /// Object labels: only those in view once zoomed far enough to read them.
    fn update_labels(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.built.clone().filter(|_| self.v.s >= 1.4) else {
            if !self.label_objs.is_empty() {
                self.label_objs.clear();
                cx.notify();
            }
            return;
        };
        let v = self.v;
        let (w, h) = (
            f32::from(self.stage.width) as f64,
            f32::from(self.stage.height) as f64,
        );
        // A label runs right of its dot: widen the left edge so ones starting off-screen still show.
        let (x0, y0) = (-v.x / v.s - 80., -v.y / v.s - 10.);
        let (x1, y1) = ((w - v.x) / v.s + 10., (h - v.y) / v.s + 10.);
        let mut out = Vec::new();
        for t in &b.g.types {
            for (j, o) in t.objects.iter().enumerate() {
                if o.x >= x0 && o.x <= x1 && o.y >= y0 && o.y <= y1 {
                    out.push((t.index, j));
                }
                if out.len() >= LABEL_CAP {
                    break;
                }
            }
        }
        self.label_objs = out;
        cx.notify();
    }

    fn sel(&self) -> Option<&Sel> {
        self.pin.as_ref().or(self.hover.as_ref())
    }

    // ------------------------------------------------------------- pointer

    /// What is under the window point: a type, a property, or (zoomed in)
    /// the object whose dot it is. Topmost first, as the SVG stacks them.
    fn hit_at(&self, pos: Point<Pixels>) -> Option<Sel> {
        let b = self.built.as_ref()?;
        let g = &b.g;
        let origin = self.origin.get();
        let v = self.v;
        let wx = (f32::from(pos.x - origin.x) as f64 - v.x) / v.s;
        let wy = (f32::from(pos.y - origin.y) as f64 - v.y) / v.s;
        let layers = Layers::at(v.s);
        let near = |x: f64, y: f64, r: f64| (x - wx).powi(2) + (y - wy).powi(2) <= r * r;
        // Invisible layers leave the render tree; faint ones take no pointer.
        if layers.schema_labels >= 0.3 {
            if let Some(labels) = &self.labels {
                for t in g.types.iter().rev() {
                    if labels.types.get(t.index).is_some_and(|l| l.hit(wx, wy)) {
                        return Some(Sel::Type(t.index));
                    }
                }
            }
            for (i, p) in g.props.iter().enumerate().rev() {
                let on_text = self
                    .labels
                    .as_ref()
                    .is_some_and(|l| l.props.get(i).is_some_and(|l| l.hit(wx, wy)));
                if on_text || near(p.x, p.y, prop_radius(p) + 8.) {
                    return Some(Sel::Prop(p.key.clone()));
                }
            }
        }
        for t in g.types.iter().rev() {
            if !near(t.x, t.y, t.rad + 5.) {
                continue;
            }
            if t.objects.iter().any(|o| near(o.x, o.y, 4.3)) {
                // Dots are too small to aim at until zoomed in: before that they stand for their type.
                if v.s <= 1.2 {
                    return Some(Sel::Type(t.index));
                }
                // A type's dots are one shape: the nearest dot to the point is the one hit.
                let mut best = (0, f64::INFINITY);
                for (j, o) in t.objects.iter().enumerate() {
                    let dd = (o.x - wx).powi(2) + (o.y - wy).powi(2);
                    if dd < best.1 {
                        best = (j, dd);
                    }
                }
                return Some(Sel::Obj(t.index, best.0));
            }
            if near(t.x, t.y, t.rad) {
                return Some(Sel::Type(t.index));
            }
        }
        if layers.far >= 0.3
            && let Some(labels) = &self.labels
        {
            for t in g.types.iter().rev() {
                if labels.far.get(t.index).is_some_and(|l| l.hit(wx, wy)) {
                    return Some(Sel::Type(t.index));
                }
            }
        }
        None
    }

    fn on_down(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.touched = true;
        self.anim = None;
        self.drag = Some(Drag {
            x: f32::from(pos.x),
            y: f32::from(pos.y),
            vx: self.v.x,
            vy: self.v.y,
            moved: false,
        });
        cx.notify();
    }

    fn on_move(&mut self, pos: Point<Pixels>, hovered: bool, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.drag {
            let dx = f32::from(pos.x) - drag.x;
            let dy = f32::from(pos.y) - drag.y;
            if dx.abs() + dy.abs() > 4. {
                drag.moved = true;
                self.dragging = true;
            }
            if drag.moved {
                self.v.x = drag.vx + dx as f64;
                self.v.y = drag.vy + dy as f64;
                self.apply_view(cx);
                return;
            }
        } else if !hovered {
            return;
        }
        let next = self.hit_at(pos);
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }

    fn on_up(
        &mut self,
        pos: Point<Pixels>,
        click_count: usize,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.take() else { return };
        // A tap (no drag) pins what is under it.
        if !drag.moved {
            self.pin = self.hit_at(pos);
        }
        self.dragging = false;
        cx.notify();
        if click_count >= 2
            && hovered
            && let (Some(Sel::Obj(t, j)), Some(b)) = (self.hit_at(pos), &self.built)
        {
            let id = b.object(t, j).id.clone();
            nav::navigate(Route::Object(id), cx);
        }
    }

    /// Scroll zooms about the cursor (`deltaY` in CSS pixels, down = out).
    fn on_wheel(&mut self, pos: Point<Pixels>, delta_y: f64, ctrl: bool, cx: &mut Context<Self>) {
        self.touched = true;
        self.anim = None;
        let origin = self.origin.get();
        let mx = f32::from(pos.x - origin.x) as f64;
        let my = f32::from(pos.y - origin.y) as f64;
        let s2 = (self.v.s * (-delta_y * if ctrl { 0.01 } else { 0.0016 }).exp()).clamp(0.02, 7.);
        self.zoom_about(s2, mx, my, cx);
    }

    /// A one-finger drag on a touch screen pans (the web's pointer drag);
    /// the platform delivers it as scroll steps that follow the finger.
    fn on_pan(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.touched = true;
        self.anim = None;
        self.v.x += f32::from(delta.x) as f64;
        self.v.y += f32::from(delta.y) as f64;
        self.apply_view(cx);
    }

    /// Trackpad pinch about its centre.
    fn on_pinch(&mut self, pos: Point<Pixels>, delta: f32, cx: &mut Context<Self>) {
        self.touched = true;
        self.anim = None;
        let origin = self.origin.get();
        let mx = f32::from(pos.x - origin.x) as f64;
        let my = f32::from(pos.y - origin.y) as f64;
        let s2 = (self.v.s * (1. + delta as f64)).clamp(0.02, 7.);
        self.zoom_about(s2, mx, my, cx);
    }

    fn zoom_about(&mut self, s2: f64, mx: f64, my: f64, cx: &mut Context<Self>) {
        let f = s2 / self.v.s;
        self.v = View {
            s: s2,
            x: mx - (mx - self.v.x) * f,
            y: my - (my - self.v.y) * f,
        };
        self.apply_view(cx);
    }

    // -------------------------------------------------------------- finder

    /// Objects and properties by name; names that start with the query first,
    /// then the rest; properties ahead of objects.
    fn refind(&mut self, force: bool, cx: &mut Context<Self>) {
        let q = self.query.read(cx).value().trim().to_lowercase();
        // Only a new query (or graph) recomputes the matches and resets the highlight.
        if !force && self.found_for.as_deref() == Some(q.as_str()) {
            return;
        }
        self.found_for = Some(q.clone());
        self.hit = 0;
        self.found.clear();
        let Some(b) = &self.built else { return };
        if q.is_empty() {
            cx.notify();
            return;
        }
        let mut found: Vec<Found> = Vec::new();
        for p in
            b.g.props
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&q))
        {
            found.push(Found {
                is_obj: false,
                name: if p.icon.is_empty() {
                    p.name.clone()
                } else {
                    format!("{} {}", p.icon, p.name)
                },
                sub: "Property".into(),
                color: hex(0x8a90a0, 1.),
                sel: Sel::Prop(p.key.clone()),
                x: p.x,
                y: p.y,
            });
        }
        for t in &b.g.types {
            for (j, o) in t
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| o.name.to_lowercase().contains(&q))
            {
                found.push(Found {
                    is_obj: true,
                    name: o.name.clone(),
                    sub: t.name.clone(),
                    color: t.color,
                    sel: Sel::Obj(t.index, j),
                    x: o.x,
                    y: o.y,
                });
            }
        }
        let rank = |f: &Found| {
            let lower = f.name.to_lowercase();
            usize::from(
                !lower
                    .trim_start_matches(|c: char| !c.is_alphabetic())
                    .starts_with(&q),
            )
        };
        found.sort_by_key(rank);
        found.truncate(12);
        self.found = found;
        cx.notify();
    }

    pub(crate) fn found_items(&self) -> &[Found] {
        &self.found
    }

    pub(crate) fn hit_index(&self) -> usize {
        self.hit
    }

    pub(crate) fn set_hit(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.hit != i {
            self.hit = i;
            cx.notify();
        }
    }

    pub(crate) fn fly_to(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.found.get(i).cloned() else {
            return;
        };
        self.pin = Some(f.sel.clone());
        // Objects show their dots from the object level; properties from the schema level.
        self.zoom_to(
            if f.is_obj {
                2.3
            } else {
                f64::max(0.8, self.v.s)
            },
            f.x,
            f.y,
        );
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
        cx.notify();
    }

    fn finder(&self, narrow: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.query.read(cx).focus_handle(cx).is_focused(window);
        let state = self.query.clone();
        let query_text = self.query.read(cx).value().trim().to_string();
        let input = div()
            .w_full()
            .bg(hex(0x14161c, 0.85))
            .border_1()
            .border_color(hex(if focused { 0x4a5266 } else { 0x2a2f3a }, 1.))
            .rounded(px(8.))
            .px(px(12.))
            .py(px(8.))
            .text_size(px(14.))
            .text_color(hex(0xe4e7ee, 1.))
            .child(
                Input::new(&state)
                    .appearance(false)
                    .cleanable(true)
                    .text_size(px(14.))
                    .p_0()
                    .h(px(18.))
                    .w_full(),
            );
        let mut col = div()
            .id("graph-finder")
            .absolute()
            .flex()
            .gap(px(6.))
            .occlude()
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                if !this.found.is_empty() {
                    this.hit = (this.hit + 1) % this.found.len();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                if !this.found.is_empty() {
                    this.hit = (this.hit + this.found.len() - 1) % this.found.len();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                if this.found.get(this.hit).is_some() {
                    cx.stop_propagation();
                    this.fly_to(this.hit, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.query.update(cx, |q, cx| q.set_value("", window, cx));
                cx.notify();
            }));
        col = if narrow {
            col.bottom(px(76.))
                .left(px(12.))
                .right(px(12.))
                .flex_col_reverse()
        } else {
            col.top(px(24.)).left(px(28.)).w(px(320.)).flex_col()
        };
        col = col.child(input);
        if !query_text.is_empty() {
            col = col.child(panel::found_list(self, cx));
        }
        col.into_any_element()
    }

    fn levels(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut pill = div()
            .id("graph-levels")
            .occlude()
            .flex()
            .items_center()
            .gap(px(4.))
            .p(px(4.))
            .bg(hex(0x14171f, 0.9))
            .border_1()
            .border_color(hsla(0., 0., 1., 0.08))
            .rounded_full();
        for (i, (label, _)) in LEVELS.iter().enumerate() {
            let on = i == self.level;
            pill = pill.child(
                div()
                    .id(("graph-level", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded_full()
                    .px(px(16.))
                    .py(px(8.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(hex(0xe7e9ee, 1.)).text_color(hex(0x0c0e13, 1.))
                    })
                    .when(!on, |d| d.text_color(hex(0x9aa0ae, 1.)))
                    .child(
                        div()
                            .font_family(roostr_core::theme::fonts::MONO)
                            .text_size(px(10.))
                            .opacity(0.6)
                            .child((i + 1).to_string()),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| this.go_level(i, cx))),
            );
        }
        pill = pill.child(
            div()
                .font_family(roostr_core::theme::fonts::MONO)
                .text_size(px(11.))
                .text_color(hex(0x6f7686, 1.))
                .pl(px(6.))
                .pr(px(12.))
                .min_w(px(40.))
                .text_right()
                .child(format!("{}%", (self.v.s * 100.).round())),
        );
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(24.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(hex(0x8a90a0, 1.))
                    .text_center()
                    .child(LEVELS[self.level].1),
            )
            .child(pill)
            .into_any_element()
    }

    /// Geometry for the current level of detail and selection, rebuilt when either changes.
    fn frame(&mut self) -> Option<Rc<Frame>> {
        let b = self.built.clone()?;
        let lod = lod(self.v.s);
        if self.geo.as_ref().is_none_or(|g| g.lod != lod) {
            self.geo = Some(Rc::new(Geo::build(&b, lod)));
        }
        if let Some(labels) = &self.labels
            && self.label_geo.as_ref().is_none_or(|g| g.lod != lod)
        {
            self.label_geo = Some(Rc::new(LabelGeo::build(labels, lod)));
        }
        let sel = self.sel().cloned();
        let d: Derived = b.derive(sel.as_ref());
        if self
            .sel_geo
            .as_ref()
            .is_none_or(|g| g.key != (sel.clone(), lod))
        {
            self.sel_geo = Some(Rc::new(SelGeo::build(&b, &d, lod)));
        }
        Some(Rc::new(Frame {
            b,
            labels: self.labels.clone(),
            geo: self.geo.clone()?,
            label_geo: self.label_geo.clone(),
            sel_geo: self.sel_geo.clone()?,
            d,
            view: self.v,
            label_objs: self.label_objs.clone(),
        }))
    }
}

/// One drawing layer: a canvas over the whole stage at a group opacity.
fn layer(
    opacity: f64,
    frame: &Rc<Frame>,
    paint: fn(&Frame, Bounds<Pixels>, &mut Window, &mut App),
) -> Option<AnyElement> {
    if opacity <= 0. {
        return None;
    }
    let frame = frame.clone();
    Some(
        div()
            .absolute()
            .inset_0()
            .opacity(opacity as f32)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| paint(&frame, bounds, window, cx),
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.step_anim(cx) {
            window.request_animation_frame();
        }
        let viewport = window.viewport_size();
        let narrow = f32::from(viewport.width) <= 900.;
        let height = px(f32::max(400., f32::from(viewport.height) - 120.));
        let frame = self.frame();
        let layers = Layers::at(self.v.s);

        let mut stage = div()
            .relative()
            .w_full()
            .h(height)
            .rounded(px(12.))
            .overflow_hidden()
            .bg(hex(0x0c0e13, 1.))
            .text_color(hex(0xe7e9ee, 1.))
            .child(
                canvas(
                    |_, _, _| (),
                    |bounds, (), window, _| Frame::paint_background(bounds, window),
                )
                .absolute()
                .inset_0(),
            );
        if let Some(frame) = &frame {
            stage = stage
                .children(layer(layers.schema, frame, |f, b, w, cx| {
                    f.paint_schema(b, w, cx)
                }))
                .children(layer(layers.far, frame, |f, b, w, cx| {
                    f.paint_far(b, w, cx)
                }))
                .children(layer(1., frame, |f, b, w, _| f.paint_clusters(b, w)))
                .children(layer(layers.schema_labels, frame, |f, b, w, cx| {
                    f.paint_schema_labels(b, w, cx)
                }))
                .children(layer(layers.obj, frame, |f, b, w, cx| {
                    f.paint_object_labels(b, w, cx)
                }));
        }

        // Pointer: one hitbox over the stage; moves and releases are followed
        // window-wide while dragging (pointer capture).
        let view = cx.entity().downgrade();
        let origin = self.origin.clone();
        let stage_size = self.stage;
        let dragging = self.dragging;
        stage = stage.child(
            canvas(
                move |bounds, window, _| {
                    origin.set(bounds.origin);
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                },
                move |bounds, hitbox, window, cx| {
                    Frame::paint_corners(bounds, window);
                    window.set_cursor_style(
                        if dragging {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        },
                        &hitbox,
                    );
                    if bounds.size != stage_size {
                        let view = view.clone();
                        let size = bounds.size;
                        window.defer(cx, move |_, cx| {
                            let _ = view.update(cx, |this, cx| this.set_stage(size, cx));
                        });
                    }
                    let (v1, h1) = (view.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble
                            && e.button == MouseButton::Left
                            && h1.is_hovered(window)
                        {
                            let _ = v1.update(cx, |this, cx| this.on_down(e.position, cx));
                        }
                    });
                    let (v2, h2) = (view.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            let hovered = h2.is_hovered(window);
                            let _ = v2.update(cx, |this, cx| this.on_move(e.position, hovered, cx));
                        }
                    });
                    let (v3, h3) = (view.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let hovered = h3.is_hovered(window);
                            let _ = v3.update(cx, |this, cx| {
                                this.on_up(e.position, e.click_count, hovered, cx)
                            });
                        }
                    });
                    let (v4, h4) = (view.clone(), hitbox.clone());
                    window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && h4.is_hovered(window) {
                            if roostr_core::layout::is_touch() {
                                // The canvas pans on both axes; a touch pan
                                // is otherwise locked to the one it started on.
                                window.pan_touch_freely();
                                let delta = e.delta.pixel_delta(px(50.));
                                let _ = v4.update(cx, |this, cx| this.on_pan(delta, cx));
                            } else {
                                // CSS deltaY: positive scrolls down, i.e. content moving up.
                                let dy = -f32::from(e.delta.pixel_delta(px(50.)).y) as f64;
                                let ctrl = e.modifiers.control;
                                let _ = v4
                                    .update(cx, |this, cx| this.on_wheel(e.position, dy, ctrl, cx));
                            }
                            cx.stop_propagation();
                        }
                    });
                    let (v5, h5) = (view.clone(), hitbox);
                    window.on_mouse_event(move |e: &PinchEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble && h5.is_hovered(window) {
                            let _ =
                                v5.update(cx, |this, cx| this.on_pinch(e.position, e.delta, cx));
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );

        if !self.status.is_empty() {
            stage = stage.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hex(0x8a90a0, 1.))
                    .child(self.status.clone()),
            );
        }
        if let Some(b) = self.built.clone()
            && self.status.is_empty()
        {
            let d = b.derive(self.sel());
            let channel = self
                .built_for
                .as_ref()
                .map(|k| k.0.clone())
                .unwrap_or_default();
            stage = stage
                .child(self.finder(narrow, window, cx))
                .child(panel::side_panel(&b, &d, &channel, narrow, height, cx))
                .child(self.levels(cx));
        }
        stage
    }
}

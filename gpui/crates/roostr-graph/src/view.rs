//! The graph page (`routes/app/graph/+page.svelte`): the page state (world → screen view,
//! hover/pin, level), pointer handling (drag pans, scroll and pinch zoom about the cursor,
//! click pins, double-click opens), the eased zoom, the finder, and the layered drawing.
//!
//! Pages live in the [`Graphs`] slot, one per `Route::Graph { focus }` a tab of the active
//! space shows (as the shell kept one GPUI view per such route). The camera is a pair of Rune
//! live values on the world element, so a pan or zoom that changes nothing else the view
//! shows (its level, its level of detail, the object labels) draws without re-running it.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use roostr_core::nav::{self, Route};
use roostr_core::relations::{current_space_id, space_relations};
use roostr_core::{NavEvent, State, StoreEvent};
use roostr_rt::{Executor, Task};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Input, InputState, Theme};
use rune::{
    Color, Cursor, Cx, Drag, El, Key, KeyEvent, Live, LiveText, MagnifyEvent, Phase, PointerEvent,
    Tx, WheelEvent, col, el, memo, row, text,
};
use web_time::Instant;

use crate::geom::{View, hex, lod};
use crate::live::{Hub, Local};
use crate::model::{GraphType, build_schema_graph};
use crate::panel;
use crate::scene::{self, Built, Derived, Geo, Labels, Layers, Scene, Sel, SelGeo, prop_radius};

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
/// How often the graph asks this computer's harness and engine what they're doing.
const POLL: Duration = Duration::from_secs(2);
/// One pulse's trip from an agent to the object it works on (ms).
const PULSE_MS: f64 = 1400.;
/// A frame of the eased zoom and of the pulses.
const FRAME: Duration = Duration::from_millis(16);
/// Object labels follow the view this long after it stops moving.
const LABEL_DELAY: Duration = Duration::from_millis(120);

/// A page's identity: the `focus` of the `Route::Graph` it shows.
pub(crate) type PageKey = Option<String>;

/// Every open graph page, and this computer's engine and harness as last read.
#[derive(Default)]
pub struct Graphs {
    pub(crate) pages: HashMap<PageKey, Page>,
    /// Numbers each built graph (and label layout), so memoized layers rebuild for a new one.
    generation: u64,
    /// This computer's engine and harness (`live`), as last read.
    pub(crate) local: Option<Local>,
    /// Reads `local` every couple of seconds while a page is open; none without a harness.
    poll: Option<Task<()>>,
    /// Moves the pulses while an agent here is working.
    pulse: Option<Task<()>>,
    /// The pulses' clock: 0..1 through a trip.
    pub(crate) phase: f64,
    epoch: Option<Instant>,
}

impl Graphs {
    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }
}

/// A finder result.
#[derive(Clone)]
pub(crate) struct Found {
    pub is_obj: bool,
    pub name: String,
    pub sub: String,
    pub color: Color,
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

/// A press on the stage: where the view was, and whether it has moved past a click.
struct PanDrag {
    vx: f64,
    vy: f64,
    moved: bool,
}

/// The camera the world element draws through, and the layer opacities it sets.
pub(crate) struct Camera {
    pub pan: Live<[f32; 2]>,
    pub zoom: Live<f32>,
    pub far: Live<f32>,
    pub schema: Live<f32>,
    pub obj: Live<f32>,
    pub schema_labels: Live<f32>,
    /// The zoom percentage in the level pill.
    pub pct: LiveText,
}

impl Camera {
    fn new() -> Self {
        Self {
            pan: Live::new([0., 0.]),
            zoom: Live::new(1.),
            far: Live::new(1.),
            schema: Live::new(0.),
            obj: Live::new(0.),
            schema_labels: Live::new(0.),
            pct: LiveText::new("30%"),
        }
    }

    fn show(&self, v: View) {
        self.pan.set([v.x as f32, v.y as f32]);
        self.zoom.set(v.s as f32);
        let l = Layers::at(v.s);
        self.far.set(l.far as f32);
        self.schema.set(l.schema as f32);
        self.obj.set(l.obj as f32);
        self.schema_labels.set(l.schema_labels as f32);
        self.pct.set(format!("{}%", (v.s * 100.).round()));
    }
}

/// One graph page.
pub(crate) struct Page {
    /// `?focus=<objectId>`: pin that object and zoom to it ("show in graph").
    focus: Option<String>,
    /// (space, default space) the graph was built for.
    pub(crate) built_for: Option<(String, String)>,
    /// Lite: the space was held whole when built (else it has placeholders).
    built_held: bool,
    pub(crate) built: Option<Rc<Built>>,
    /// Which build `built` is (and its labels' layout): memoized layers' version.
    generation: u64,
    pub(crate) labels: Option<Rc<Labels>>,
    labels_generation: u64,
    geo: Option<Rc<Geo>>,
    /// The selection's highlight geometry, and the (selection, level of detail) it is for.
    sel_geo: Option<(Option<Sel>, i32, Rc<SelGeo>)>,
    pub(crate) status: String,
    load: Option<Task<()>>,
    pub(crate) hover: Option<Sel>,
    pub(crate) pin: Option<Sel>,
    pub(crate) level: usize,
    /// The level of detail the geometry is flattened for.
    pub(crate) lod: i32,
    pub(crate) v: View,
    /// Stage rect in window points, from the last layout.
    pub(crate) stage: [f32; 4],
    /// Finder, side panel and level pill rects in stage points: the stage's pointer
    /// handling stays out of them, as GPUI's `occlude` kept it.
    overlays: HashMap<&'static str, [f32; 4]>,
    touched: bool,
    /// A built graph waits for the stage size to fit (and focus).
    pending_fit: bool,
    anim: Option<Anim>,
    anim_task: Option<Task<()>>,
    drag: Option<PanDrag>,
    pub(crate) dragging: bool,
    pub(crate) label_objs: Vec<(usize, usize)>,
    label_timer: Option<Task<()>>,
    pub(crate) query: InputState,
    pub(crate) found: Vec<Found>,
    /// The query `found` was computed for.
    found_for: Option<String>,
    pub(crate) hit: usize,
    pub(crate) cam: Camera,
}

impl Page {
    fn new(focus: Option<String>) -> Self {
        let v = View {
            s: 0.3,
            x: 0.,
            y: 0.,
        };
        let cam = Camera::new();
        cam.show(v);
        Self {
            focus: focus.filter(|f| !f.is_empty()),
            built_for: None,
            built_held: true,
            built: None,
            generation: 0,
            labels: None,
            labels_generation: 0,
            geo: None,
            sel_geo: None,
            status: "Loading graph…".into(),
            load: None,
            hover: None,
            pin: None,
            level: 0,
            lod: lod(v.s),
            v,
            stage: [0.; 4],
            overlays: HashMap::new(),
            touched: false,
            pending_fit: false,
            anim: None,
            anim_task: None,
            drag: None,
            dragging: false,
            label_objs: Vec::new(),
            label_timer: None,
            query: InputState::new(),
            found: Vec::new(),
            found_for: None,
            hit: 0,
            cam,
        }
    }

    pub(crate) fn sel(&self) -> Option<&Sel> {
        self.pin.as_ref().or(self.hover.as_ref())
    }

    fn w(&self) -> f64 {
        self.stage[2] as f64
    }

    fn h(&self) -> f64 {
        self.stage[3] as f64
    }

    fn wide(&self) -> bool {
        self.stage[2] > 900.
    }

    /// Phone-sized: the finder drops to the bottom and the panel spans the top. GPUI asked
    /// the window (≤ 900 wide); a view here knows its stage, which ends at the window's right
    /// edge less the page's margin.
    pub(crate) fn narrow(&self) -> bool {
        self.stage[0] + self.stage[2] <= 900.
    }

    /// Screen x of the world origin: left of centre when the side panel takes the right edge.
    fn centre_x(&self) -> f64 {
        if self.wide() {
            (self.w() - 300.) / 2.
        } else {
            self.w() / 2.
        }
    }

    fn fit_view(&self) -> View {
        let (w, h) = (self.w(), self.h());
        let span = f64::min(if self.wide() { w - 360. } else { w * 0.95 }, h);
        let extent = self.built.as_ref().map_or(1750., |b| b.g.extent);
        View {
            s: f64::max(0.02, span / (2. * extent)),
            x: self.centre_x(),
            y: h / 2.,
        }
    }

    /// Bring the derived geometry up to date with the level of detail and the selection.
    fn refresh(&mut self) {
        let Some(b) = self.built.clone() else {
            self.geo = None;
            self.sel_geo = None;
            return;
        };
        if self.geo.as_ref().is_none_or(|g| g.lod != self.lod) {
            self.geo = Some(Rc::new(Geo::build(&b, self.lod)));
        }
        let sel = self.sel().cloned();
        if self
            .sel_geo
            .as_ref()
            .is_none_or(|(s, l, _)| *s != sel || *l != self.lod)
        {
            let d = b.derive(sel.as_ref());
            let geo = Rc::new(SelGeo::build(&b, &d, self.lod));
            self.sel_geo = Some((sel, self.lod, geo));
        }
    }

    fn in_overlay(&self, x: f32, y: f32) -> bool {
        self.overlays
            .values()
            .any(|r| x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3])
    }

    /// What is under the stage point: a type, a property, or (zoomed in) the object whose dot
    /// it is. Topmost first, as the SVG stacks them.
    pub(crate) fn hit_at(&self, x: f64, y: f64) -> Option<Sel> {
        let b = self.built.as_ref()?;
        let g = &b.g;
        let (wx, wy) = self.v.world(x, y);
        let layers = Layers::at(self.v.s);
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
            if t.objects.iter().any(|o| near(o.x, o.y, scene::DOT_R)) {
                // Dots are too small to aim at until zoomed in: before that they stand for their type.
                if self.v.s <= 1.2 {
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
}

// ------------------------------------------------------------------ state access

fn graphs(s: &mut State) -> &mut Graphs {
    s.slot::<Graphs>()
}

fn page_mut<'a>(s: &'a mut State, key: &PageKey) -> Option<&'a mut Page> {
    graphs(s).pages.get_mut(key)
}

/// The page key of the route the active tab shows, when it is the graph.
fn route_key(s: &State) -> Option<PageKey> {
    match s.nav.route() {
        Route::Graph { focus } => Some(focus.clone()),
        _ => None,
    }
}

// ------------------------------------------------------------------ registration

/// Install the graph's listeners: pages open for the `Route::Graph` routes the active
/// space's tabs show, build the active space's graph, and rebuild it when the space changes,
/// finishes a lite download or first loads; store refreshes relabel renamed properties.
pub fn register(s: &mut State) {
    s.on_nav(on_nav);
    s.on_store(on_store);
}

fn on_nav(s: &mut State, _: &NavEvent) {
    reconcile(s);
}

fn on_store(s: &mut State, event: &StoreEvent) {
    if matches!(event, StoreEvent::Live(_)) {
        return;
    }
    let keys: Vec<PageKey> = graphs(s).pages.keys().cloned().collect();
    for key in keys {
        sync(s, &key);
    }
}

/// Pages live while a tab of the active space shows their route; the shown route gets one.
fn reconcile(s: &mut State) {
    let live: Vec<PageKey> = s
        .nav
        .tabs()
        .iter()
        .filter_map(|t| match &t.route {
            Route::Graph { focus } => Some(focus.clone()),
            _ => None,
        })
        .collect();
    graphs(s).pages.retain(|k, _| live.contains(k));
    if let Some(key) = route_key(s) {
        ensure_page(s, &key);
    }
    let keys: Vec<PageKey> = graphs(s).pages.keys().cloned().collect();
    for key in &keys {
        sync(s, key);
    }
    if keys.is_empty() {
        let g = graphs(s);
        g.poll = None;
        g.pulse = None;
    }
}

/// The page for `key`, opened (and built) if it isn't yet.
fn ensure_page(s: &mut State, key: &PageKey) {
    if graphs(s).pages.contains_key(key) {
        return;
    }
    graphs(s).pages.insert(key.clone(), Page::new(key.clone()));
    start_poll(s);
    sync(s, key);
}

/// Built once per space (and once the definitions first load): rebuilding on every sync
/// refresh would reset the view and drop what is pinned. Store changes still relabel renamed
/// properties.
fn sync(s: &mut State, key: &PageKey) {
    if !s.store.loaded {
        return;
    }
    let channel = current_space_id(s);
    if channel.is_empty() {
        return;
    }
    let default = s
        .store
        .channels
        .first()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    // Lite: a space finishing its download rebuilds with real nodes.
    let held = roostr_core::lite_ui::space_held(&channel, s);
    let stale = {
        let Some(page) = page_mut(s, key) else { return };
        let for_key = Some((channel.clone(), default.clone()));
        let stale = page.built_for != for_key || page.built_held != held;
        if stale {
            page.built_for = for_key;
            page.built_held = held;
        }
        stale
    };
    if stale {
        build(s, key, channel.clone(), channel == default);
    }
    relabel(s, key);
}

fn build(s: &mut State, key: &PageKey, channel: String, is_default: bool) {
    let relations: Vec<_> = space_relations(&s.store.relations, &channel)
        .into_iter()
        .cloned()
        .collect();
    let types = s.store.types.clone();
    let rows = s.store.query_all(serde_json::json!({}));
    // Lite: the directory's per-type counts give the not-loaded placeholders.
    let counts = match roostr_core::lite::status(s) {
        Some(status) if !status.is_full(&channel) => roostr_core::lite::lite(s)
            .map(|l| l.type_counts(&channel))
            .unwrap_or_default(),
        _ => Default::default(),
    };
    let key2 = key.clone();
    let task = s.rt.spawn(async move |cx| {
        let result = match rows.await {
            Ok(rows) => Ok(cx
                .background(async move {
                    build_schema_graph(rows, &channel, is_default, &relations, &types, &counts)
                })
                .await),
            Err(error) => Err(error),
        };
        cx.update(|s| match result {
            Ok(g) => loaded(s, &key2, g),
            Err(error) => {
                if let Some(page) = page_mut(s, &key2) {
                    page.status = format!("Graph failed: {error}");
                }
            }
        });
    });
    if let Some(page) = page_mut(s, key) {
        page.status = "Loading graph…".into();
        page.load = Some(task);
    }
}

fn loaded(s: &mut State, key: &PageKey, g: crate::model::SchemaGraph) {
    let rt = s.rt.clone();
    let generation = graphs(s).next_generation();
    let Some(page) = page_mut(s, key) else { return };
    page.status = if g.types.is_empty() {
        "Nothing in this space yet.".into()
    } else {
        String::new()
    };
    page.built = Some(Rc::new(Built::new(g)));
    page.generation = generation;
    page.hover = None;
    page.pin = None;
    page.labels = None;
    page.geo = None;
    page.sel_geo = None;
    page.label_objs.clear();
    page.pending_fit = true;
    relabel(s, key);
    refind(s, key, true);
    let Some(page) = page_mut(s, key) else { return };
    try_fit(&rt, key, page);
    page.refresh();
}

/// Lay the world-space labels out again when their texts change (a property renamed, the
/// graph rebuilt).
fn relabel(s: &mut State, key: &PageKey) {
    let Some(page) = s.get::<Graphs>().and_then(|g| g.pages.get(key)) else {
        return;
    };
    let Some(built) = page.built.clone() else {
        return;
    };
    let channel = page
        .built_for
        .as_ref()
        .map(|k| k.0.clone())
        .unwrap_or_default();
    let texts: Vec<String> = {
        let rels = space_relations(&s.store.relations, &channel);
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
    if page.labels.as_ref().is_some_and(|l| l.texts == texts) {
        return;
    }
    let labels = Rc::new(Labels::layout(&built.g, texts));
    let generation = graphs(s).next_generation();
    if let Some(page) = page_mut(s, key) {
        page.labels = Some(labels);
        page.labels_generation = generation;
    }
}

// ------------------------------------------------------------------ live

/// Ask this computer's harness and engine what they're doing, every [`POLL`], while a graph
/// is open.
fn start_poll(s: &mut State) {
    let Some(harness) = s.harness.clone() else {
        return;
    };
    if graphs(s).poll.is_some() {
        return;
    }
    let task = s.rt.spawn(async move |cx| {
        loop {
            let backend = cx.read(|s| s.store.backend());
            let h = harness.clone();
            let local = cx
                .background(async move {
                    let machine_id = h
                        .get_json("/machine")
                        .await
                        .ok()
                        .and_then(|j| j.get("id")?.as_str().map(str::to_string))
                        .unwrap_or_default();
                    let turns = h
                        .get_json("/agent/status")
                        .await
                        .ok()
                        .map(|j| Local::turns_from(&j));
                    let engine = match backend.engine_status() {
                        Some(status) => status.await.ok().and_then(|m| Local::engine_from(&m)),
                        None => None,
                    };
                    Local {
                        machine_id,
                        turns,
                        engine,
                    }
                })
                .await;
            let open = cx.update(|s| {
                let g = graphs(s);
                if g.pages.is_empty() {
                    return false;
                }
                if g.local.as_ref() != Some(&local) {
                    g.local = Some(local);
                }
                start_pulse(s);
                true
            });
            if !open {
                return;
            }
            cx.timer(POLL).await;
        }
    });
    graphs(s).poll = Some(task);
}

/// Whether any open page's hub has an agent working (its pulses move).
fn pulsing(s: &mut State) -> bool {
    let g = graphs(s);
    let Some(local) = &g.local else { return false };
    g.pages.values().any(|p| {
        p.built.as_ref().is_some_and(|b| {
            Hub::build(b, local, 0.)
                .is_some_and(|h| h.agents.iter().any(|(_, w)| *w) || !h.pulses.is_empty())
        })
    })
}

/// Move the pulses a frame at a time while an agent here works.
fn start_pulse(s: &mut State) {
    if graphs(s).pulse.is_some() || !pulsing(s) {
        return;
    }
    let now = s.rt.now();
    let g = graphs(s);
    g.epoch.get_or_insert(now);
    let task = s.rt.spawn(async move |cx| {
        loop {
            cx.timer(FRAME).await;
            let go = cx.update(|s| {
                let now = s.rt.now();
                let on = pulsing(s);
                let g = graphs(s);
                let ms = now.duration_since(*g.epoch.get_or_insert(now)).as_millis() as f64;
                g.phase = (ms % PULSE_MS) / PULSE_MS;
                if !on {
                    g.pulse = None;
                }
                on
            });
            if !go {
                return;
            }
        }
    });
    graphs(s).pulse = Some(task);
}

/// This computer on `page`'s graph, when it serves the space.
fn hub_of(g: &Graphs, page: &Page) -> Option<Hub> {
    Hub::build(page.built.as_ref()?, g.local.as_ref()?, g.phase)
}

// ------------------------------------------------------------------ the view

/// Apply a new view: level, layer opacities, level of detail, and the object labels after
/// a pause. True when the view must re-run (not just the camera's live values).
fn apply_view(rt: &Executor<State>, key: &PageKey, page: &mut Page, hub: bool) -> bool {
    let s = page.v.s;
    page.cam.show(page.v);
    let level = if s < 0.44 {
        0
    } else if s < 1.65 {
        1
    } else {
        2
    };
    let lod = lod(s);
    let rerun = hub || level != page.level || lod != page.lod;
    page.level = level;
    page.lod = lod;
    page.refresh();
    schedule_labels(rt, key, page);
    rerun
}

fn schedule_labels(rt: &Executor<State>, key: &PageKey, page: &mut Page) {
    let key = key.clone();
    page.label_timer = Some(rt.spawn(async move |cx| {
        cx.timer(LABEL_DELAY).await;
        cx.update(|s| update_labels(s, &key));
    }));
}

/// Object labels: only those in view once zoomed far enough to read them.
fn update_labels(s: &mut State, key: &PageKey) {
    let Some(page) = page_mut(s, key) else { return };
    let Some(b) = page.built.clone().filter(|_| page.v.s >= 1.4) else {
        page.label_objs.clear();
        return;
    };
    let v = page.v;
    let (w, h) = (page.w(), page.h());
    // A label runs right of its dot: widen the left edge so ones starting off-screen still show.
    let (x0, y0) = (-v.x / v.s - 80., -v.y / v.s - 10.);
    let (x1, y1) = ((w - v.x) / v.s + 10., (h - v.y) / v.s + 10.);
    let mut out = Vec::new();
    'types: for t in &b.g.types {
        for (j, o) in t.objects.iter().enumerate() {
            if o.x >= x0 && o.x <= x1 && o.y >= y0 && o.y <= y1 {
                out.push((t.index, j));
            }
            if out.len() >= LABEL_CAP {
                break 'types;
            }
        }
    }
    page.label_objs = out;
}

/// Fit once a built graph and the stage size are both known; `?focus` then pins its object
/// and zooms to it.
fn try_fit(rt: &Executor<State>, key: &PageKey, page: &mut Page) -> bool {
    if !page.pending_fit || page.built.is_none() || page.w() <= 0. || page.h() <= 0. {
        return false;
    }
    page.pending_fit = false;
    page.touched = false;
    page.v = page.fit_view();
    apply_view(rt, key, page, false);
    if let (Some(focus), Some(b)) = (page.focus.clone(), page.built.clone())
        && let Some(&(t, j)) = b.obj_index.get(&focus)
    {
        page.pin = Some(Sel::Obj(t, j));
        let o = b.object(t, j);
        zoom_to(rt, key, page, 2.3, o.x, o.y);
    }
    true
}

/// The stage was laid out at `rect` (window points): refit until the person takes over.
fn set_stage(s: &mut State, key: &PageKey, rect: [f32; 4]) {
    ensure_page(s, key);
    let rt = s.rt.clone();
    let Some(page) = page_mut(s, key) else { return };
    if rect == page.stage {
        return;
    }
    let resized = rect[2] != page.stage[2] || rect[3] != page.stage[3];
    page.stage = rect;
    if !resized {
        return;
    }
    if !try_fit(&rt, key, page) && !page.touched {
        page.v = page.fit_view();
        apply_view(&rt, key, page, false);
    }
}

/// Eased zoom to scale `s2` centred on world point (cx, cy).
fn zoom_to(rt: &Executor<State>, key: &PageKey, page: &mut Page, s2: f64, cx: f64, cy: f64) {
    page.touched = true;
    let from = page.v;
    let fc = (
        (page.centre_x() - from.x) / from.s,
        (page.h() / 2. - from.y) / from.s,
    );
    page.anim = Some(Anim {
        from,
        t0: rt.now(),
        s2,
        fc,
        to: (cx, cy),
    });
    let key = key.clone();
    page.anim_task = Some(rt.spawn(async move |cx| {
        loop {
            if !cx.update(|s| step_anim(s, &key)) {
                return;
            }
            cx.timer(FRAME).await;
        }
    }));
}

/// Advance the zoom animation; true while it runs.
fn step_anim(s: &mut State, key: &PageKey) -> bool {
    let rt = s.rt.clone();
    let now = rt.now();
    let Some(page) = page_mut(s, key) else {
        return false;
    };
    let Some(anim) = &page.anim else { return false };
    let k = (now.duration_since(anim.t0).as_secs_f64() * 1000. / ZOOM_MS).min(1.);
    let e = if k < 0.5 {
        4. * k * k * k
    } else {
        1. - (-2. * k + 2.).powi(3) / 2.
    };
    let sc = (anim.from.s.ln() + (anim.s2.ln() - anim.from.s.ln()) * e).exp();
    let wx = anim.fc.0 + (anim.to.0 - anim.fc.0) * e;
    let wy = anim.fc.1 + (anim.to.1 - anim.fc.1) * e;
    page.v = View {
        s: sc,
        x: page.centre_x() - wx * sc,
        y: page.h() / 2. - wy * sc,
    };
    if k >= 1. {
        page.anim = None;
    }
    apply_view(&rt, key, page, false);
    page.anim.is_some()
}

fn go_level(s: &mut State, key: &PageKey, l: usize) {
    let rt = s.rt.clone();
    let Some(page) = page_mut(s, key) else { return };
    let Some(b) = page.built.clone() else { return };
    let fit = page.fit_view();
    let d = b.derive(page.sel());
    match l {
        0 => zoom_to(&rt, key, page, fit.s, 0., 0.),
        1 => zoom_to(&rt, key, page, f64::max(0.62, fit.s * 1.9), 0., 0.),
        _ => {
            if let Some(o) = d.sel_obj_ref(&b) {
                zoom_to(&rt, key, page, 2.3, o.x, o.y);
            } else {
                // The selected type, else the biggest (the first of the biggest).
                let t = d.sel_type.map(|t| &b.g.types[t]).or_else(|| {
                    b.g.types
                        .iter()
                        .fold(None, |best: Option<&GraphType>, t| match best {
                            Some(a) if t.objects.len() <= a.objects.len() => Some(a),
                            _ => Some(t),
                        })
                });
                if let Some(t) = t {
                    zoom_to(&rt, key, page, 2.3, t.x, t.y);
                }
            }
        }
    }
}

fn set_hover(page: &mut Page, next: Option<Sel>, tx: &mut Tx) {
    if next == page.hover {
        tx.live_only();
        return;
    }
    page.hover = next;
    page.refresh();
    // Highlights switch at once, as the SVG's do.
    tx.instant();
}

// ------------------------------------------------------------------ pointer

fn on_drag(s: &mut State, key: &PageKey, d: &Drag, tx: &mut Tx) {
    let rt = s.rt.clone();
    let hub = s
        .get::<Graphs>()
        .and_then(|g| hub_of(g, g.pages.get(key)?))
        .is_some();
    let Some(page) = page_mut(s, key) else { return };
    match d.phase {
        Phase::Down => {
            if page.in_overlay(d.x, d.y) {
                return;
            }
            page.touched = true;
            page.anim = None;
            page.anim_task = None;
            page.drag = Some(PanDrag {
                vx: page.v.x,
                vy: page.v.y,
                moved: false,
            });
        }
        Phase::Move => {
            let Some(drag) = &mut page.drag else { return };
            let was = drag.moved;
            if d.dx.abs() + d.dy.abs() > 4. {
                drag.moved = true;
                page.dragging = true;
            }
            if drag.moved {
                page.v.x = drag.vx + d.dx as f64;
                page.v.y = drag.vy + d.dy as f64;
                if !apply_view(&rt, key, page, hub) && was {
                    tx.live_only();
                }
                return;
            }
            let next = page.hit_at(d.x as f64, d.y as f64);
            set_hover(page, next, tx);
        }
        Phase::Up => {
            let Some(drag) = page.drag.take() else { return };
            page.dragging = false;
            // A tap (no drag) pins what is under it.
            let hit = page.hit_at(d.x as f64, d.y as f64);
            if !drag.moved && !d.cancelled() {
                page.pin = hit.clone();
                page.refresh();
                tx.instant();
            }
            let inside = d.x >= 0. && d.y >= 0. && d.x <= d.w && d.y <= d.h;
            if d.clicks >= 2
                && inside
                && let (Some(Sel::Obj(t, j)), Some(b)) = (hit, &page.built)
            {
                let id = b.object(t, j).id.clone();
                nav::navigate(Route::Object(id), s);
            }
        }
    }
}

fn on_pointer_move(s: &mut State, key: &PageKey, e: PointerEvent, tx: &mut Tx) {
    let Some(page) = page_mut(s, key) else { return };
    if page.drag.is_some() || page.in_overlay(e.x, e.y) {
        tx.live_only();
        return;
    }
    let next = page.hit_at(e.x as f64, e.y as f64);
    set_hover(page, next, tx);
}

/// Scroll zooms about the cursor (CSS `deltaY`, down = out); on a touch screen a one-finger
/// drag arrives as scroll steps and pans.
fn on_wheel(s: &mut State, key: &PageKey, e: WheelEvent, tx: &mut Tx) -> bool {
    let rt = s.rt.clone();
    let hub = s
        .get::<Graphs>()
        .and_then(|g| hub_of(g, g.pages.get(key)?))
        .is_some();
    let Some(page) = page_mut(s, key) else {
        return false;
    };
    if page.in_overlay(e.x, e.y) {
        return false;
    }
    page.touched = true;
    page.anim = None;
    page.anim_task = None;
    if roostr_core::layout::is_touch() {
        page.v.x += e.dx as f64;
        page.v.y += e.dy as f64;
    } else {
        // CSS deltaY is positive scrolling down, content moving up: Rune's `dy` negated.
        let delta_y = -e.dy as f64;
        let k = if e.mods.ctrl { 0.01 } else { 0.0016 };
        let s2 = (page.v.s * (-delta_y * k).exp()).clamp(0.02, 7.);
        zoom_about(page, s2, e.x as f64, e.y as f64);
    }
    if !apply_view(&rt, key, page, hub) {
        tx.live_only();
    }
    true
}

/// Trackpad pinch about the pointer.
fn on_magnify(s: &mut State, key: &PageKey, e: MagnifyEvent, tx: &mut Tx) -> bool {
    let rt = s.rt.clone();
    let hub = s
        .get::<Graphs>()
        .and_then(|g| hub_of(g, g.pages.get(key)?))
        .is_some();
    let Some(page) = page_mut(s, key) else {
        return false;
    };
    if page.in_overlay(e.x, e.y) {
        return false;
    }
    page.touched = true;
    page.anim = None;
    page.anim_task = None;
    let s2 = (page.v.s * (1. + e.delta as f64)).clamp(0.02, 7.);
    zoom_about(page, s2, e.x as f64, e.y as f64);
    if !apply_view(&rt, key, page, hub) {
        tx.live_only();
    }
    true
}

fn zoom_about(page: &mut Page, s2: f64, mx: f64, my: f64) {
    let f = s2 / page.v.s;
    page.v = View {
        s: s2,
        x: mx - (mx - page.v.x) * f,
        y: my - (my - page.v.y) * f,
    };
}

// ------------------------------------------------------------------ finder

/// Objects and properties by name; names that start with the query first, then the rest;
/// properties ahead of objects.
fn refind(s: &mut State, key: &PageKey, force: bool) {
    let Some(page) = page_mut(s, key) else { return };
    let q = page.query.value().trim().to_lowercase();
    // Only a new query (or graph) recomputes the matches and resets the highlight.
    if !force && page.found_for.as_deref() == Some(q.as_str()) {
        return;
    }
    page.found_for = Some(q.clone());
    page.hit = 0;
    page.found.clear();
    let Some(b) = page.built.clone() else { return };
    if q.is_empty() {
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
    page.found = found;
}

pub(crate) fn set_hit(s: &mut State, key: &PageKey, i: usize) {
    if let Some(page) = page_mut(s, key) {
        page.hit = i;
    }
}

pub(crate) fn fly_to(s: &mut State, key: &PageKey, i: usize, tx: &mut Tx) {
    let rt = s.rt.clone();
    let Some(page) = page_mut(s, key) else { return };
    let Some(f) = page.found.get(i).cloned() else {
        return;
    };
    page.pin = Some(f.sel.clone());
    page.refresh();
    // Objects show their dots from the object level; properties from the schema level.
    let s2 = if f.is_obj {
        2.3
    } else {
        f64::max(0.8, page.v.s)
    };
    zoom_to(&rt, key, page, s2, f.x, f.y);
    page.query.clear();
    tx.instant();
    refind(s, key, false);
}

fn finder_key(s: &mut State, key: &PageKey, e: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(page) = page_mut(s, key) else {
        return false;
    };
    let n = page.found.len();
    match e.key {
        Key::Down => {
            if n > 0 {
                page.hit = (page.hit + 1) % n;
            }
            true
        }
        Key::Up => {
            if n > 0 {
                page.hit = (page.hit + n - 1) % n;
            }
            true
        }
        Key::Enter => {
            if page.found.get(page.hit).is_none() {
                return false;
            }
            let hit = page.hit;
            fly_to(s, key, hit, tx);
            true
        }
        Key::Escape => {
            page.query.clear();
            refind(s, key, false);
            true
        }
        _ => false,
    }
}

/// Remember where an overlay sits, in stage points.
fn note_overlay(s: &mut State, key: &PageKey, name: &'static str, rect: [f32; 4]) {
    if let Some(page) = page_mut(s, key) {
        let r = [
            rect[0] - page.stage[0],
            rect[1] - page.stage[1],
            rect[2],
            rect[3],
        ];
        if page.overlays.get(name) != Some(&r) {
            page.overlays.insert(name, r);
        }
    }
}

fn finder(page: &Page, key: &PageKey, t: &Theme) -> El {
    let focused = page.query.is_focused();
    let query_text = page.query.value().trim().to_string();
    let (k1, k2, k3, k4) = (key.clone(), key.clone(), key.clone(), key.clone());
    let input = Input::new("graph-find", &page.query)
        .appearance(false)
        .cleanable(true)
        .text_size(14.)
        .text_color(hex(0xe4e7ee, 1.))
        .px(0.)
        .h(18.)
        .placeholder("Find an object or property…")
        .on_edit(move |s: &mut State| {
            &mut graphs(s)
                .pages
                .entry(k1.clone())
                .or_insert_with(|| Page::new(k1.clone()))
                .query
        })
        .on_change(move |s: &mut State, _| refind(s, &k2, false))
        .on_key(move |s: &mut State, e, tx| finder_key(s, &k3, e, tx))
        .build(t);
    let field = el()
        .w_full()
        .fill(hex(0x14161c, 0.85))
        .border(1., hex(if focused { 0x4a5266 } else { 0x2a2f3a }, 1.))
        .radius(8.)
        .px(12.)
        .py(8.)
        .child(input);
    let narrow = page.narrow();
    let mut column = if narrow {
        col().pin(Some(12.), None, Some(12.), Some(76.))
    } else {
        col().pin(Some(28.), Some(24.), None, None).w(320.)
    }
    .key("graph-finder")
    .gap(6.)
    .on_layout(move |s: &mut State, r| note_overlay(s, &k4, "finder", r));
    let list = (!query_text.is_empty()).then(|| panel::found_list(page, key));
    if narrow {
        // `flex-col-reverse`: the results above the field.
        if let Some(list) = list {
            column = column.child(list);
        }
        column.child(field)
    } else {
        column = column.child(field);
        if let Some(list) = list {
            column = column.child(list);
        }
        column
    }
}

fn levels(page: &Page, key: &PageKey) -> El {
    let mut pill = row()
        .key("graph-levels")
        .items_center()
        .gap(4.)
        .padding(4.)
        .fill(hex(0x14171f, 0.9))
        .border(1., Color::WHITE.alpha(0.08))
        .radius(999.);
    for (i, (label, _)) in LEVELS.iter().enumerate() {
        let on = i == page.level;
        let ink = if on {
            hex(0x0c0e13, 1.)
        } else {
            hex(0x9aa0ae, 1.)
        };
        let k = key.clone();
        let mut button = row()
            .key(format!("graph-level-{i}"))
            .items_center()
            .gap(8.)
            .px(16.)
            .py(8.)
            .radius(999.)
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut State| go_level(s, &k, i))
            .child(
                text((i + 1).to_string())
                    .font(roostr_ui::ty::mono(10.))
                    .color(ink)
                    .opacity(0.6),
            )
            .child(
                text(*label)
                    .font(roostr_ui::ty::ui(13.).weight(500.))
                    .color(ink),
            );
        button = if on {
            button.fill(hex(0xe7e9ee, 1.))
        } else {
            button.hover(Color::WHITE.alpha(0.06))
        };
        pill = pill.child(button);
    }
    pill = pill.child(
        el().pl(6.).pr(12.).min_w(40.).justify_end().child(
            text("100%")
                .live_text(&page.cam.pct)
                .font(roostr_ui::ty::mono(11.))
                .color(hex(0x6f7686, 1.)),
        ),
    );
    let k = key.clone();
    col()
        .pin(Some(0.), None, Some(0.), Some(24.))
        .items_center()
        .gap(10.)
        .child(
            text(LEVELS[page.level].1)
                .font(roostr_ui::ty::ui(13.))
                .color(hex(0x8a90a0, 1.)),
        )
        .child(pill.on_layout(move |s: &mut State, r| note_overlay(s, &k, "levels", r)))
}

/// The world: every layer under the camera, at its live opacity. Memoized per graph, level of
/// detail and selection, so a view run that changes none of them reuses the drawing.
fn world(page: &Page, b: &Built, labels: &Labels, geo: &Geo, sel_geo: &SelGeo) -> El {
    let sel = page.sel().cloned();
    let d: Derived = b.derive(sel.as_ref());
    let scene = Scene {
        b,
        labels,
        geo,
        sel_geo,
        d: &d,
    };
    let v = (page.generation, page.labels_generation, page.lod);
    let layer = |live: &Live<f32>, child: El| {
        el().pin(Some(0.), Some(0.), None, None)
            .live_opacity(live)
            .child(child)
    };
    let mut world = el()
        .key("graph-world")
        .pin(Some(0.), Some(0.), None, None)
        .size(1., 1.)
        .origin(0., 0.)
        .live_offset(&page.cam.pan)
        .live_scale(&page.cam.zoom)
        .inert()
        .child(layer(
            &page.cam.schema,
            el().child(memo("graph-edges", (v, d.any), || scene.edges()))
                .child(memo("graph-schema", (v, &sel), || scene.schema())),
        ))
        .child(layer(
            &page.cam.far,
            memo("graph-far", (v, &sel), || scene.far()),
        ))
        .child(memo("graph-clusters", (v, &sel), || scene.clusters()))
        .child(layer(
            &page.cam.schema_labels,
            memo("graph-schema-labels", (v, &sel), || scene.schema_labels()),
        ));
    if !page.label_objs.is_empty() {
        world = world.child(layer(
            &page.cam.obj,
            memo("graph-object-labels", (v, &sel, &page.label_objs), || {
                scene.object_labels(&page.label_objs)
            }),
        ));
    }
    world
}

/// The graph page for the active tab's `Route::Graph`: the window's height
/// less the header's, at least 400 pt (`GraphView::render`), as the page
/// sits in the main column's scroll. Outside a window root (a bare host)
/// it fills its parent.
pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let window_h = cx.layout_with("kit-root", |r| r[3]);
    let t = Theme::dark();
    let Some(key) = route_key(s) else {
        return el();
    };
    let graphs = s.get::<Graphs>();
    let page = graphs.and_then(|g| g.pages.get(&key));
    let k = key.clone();
    let mut surface = el()
        .key("graph-stage")
        .pin(Some(0.), Some(0.), Some(0.), Some(0.))
        .on_layout(move |s: &mut State, r| set_stage(s, &k, r));
    let stage = el().key("graph").w_full().radius(12.).clip_rounded(12.);
    let mut stage = match window_h {
        Some(h) => stage.h((h - 120.).max(400.)),
        None => stage.h_full().min_h(400.),
    };
    let Some(page) = page else {
        return stage.fill(hex(0x0c0e13, 1.)).child(surface);
    };
    stage = scene::background(stage, page.stage[2], page.stage[3]);
    let ready = page.status.is_empty();
    // Until the stage has a size the view is unfitted: draw nothing over the background.
    let sized = page.stage[2] > 0. && page.stage[3] > 0.;
    if let (Some(b), Some(labels), Some(geo), Some((_, _, sel_geo))) =
        (&page.built, &page.labels, &page.geo, &page.sel_geo)
        && ready
        && sized
    {
        surface = surface.child(world(page, b, labels, geo, sel_geo));
        if let Some(hub) = graphs.and_then(|g| hub_of(g, page)) {
            let v = page.v;
            surface = surface.child(
                scene::hub(
                    &hub,
                    |x, y| v.project(x, y),
                    v.s,
                    [page.stage[2], page.stage[3]],
                )
                .inert(),
            );
        }
    }
    let (k1, k2, k3, k4, k5) = (
        key.clone(),
        key.clone(),
        key.clone(),
        key.clone(),
        key.clone(),
    );
    surface = surface
        .cursor(if page.dragging {
            Cursor::Grabbing
        } else {
            Cursor::Grab
        })
        .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| on_drag(s, &k1, d, tx))
        .on_pointer_move_local(move |s: &mut State, e, tx| on_pointer_move(s, &k2, e, tx))
        .on_wheel(move |s: &mut State, e, tx| on_wheel(s, &k3, e, tx))
        .on_magnify(move |s: &mut State, e, tx| on_magnify(s, &k4, e, tx));
    stage = stage.child(surface);
    if !ready {
        stage = stage.child(
            el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .center()
                .child(
                    text(page.status.clone())
                        .font(roostr_ui::ty::ui(14.))
                        .color(hex(0x8a90a0, 1.)),
                ),
        );
        return stage;
    }
    if let Some(b) = page.built.as_ref().filter(|_| sized) {
        let d = b.derive(page.sel());
        let channel = page
            .built_for
            .as_ref()
            .map(|k| k.0.clone())
            .unwrap_or_default();
        stage = stage
            .child(finder(page, &key, t))
            .child(
                panel::side_panel(s, b, &d, &channel, page, t)
                    .on_layout(move |s: &mut State, r| note_overlay(s, &k5, "panel", r)),
            )
            .child(levels(page, &key));
    }
    stage
}

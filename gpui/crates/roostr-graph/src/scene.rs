//! What the graph page's SVG draws, layer by layer (`+page.svelte` markup): geometry
//! flattened once per graph and level of detail, the selection's highlight geometry, and the
//! world-space elements of each layer.

use std::collections::HashMap;

use rune::{Color, El, LineCap, el, text};

use crate::geom::{Pen, Poly, circle, dash, fade, hex, quad, tolerance};
use crate::labels::{Anchor, Family, Label, LabelSpec, SpanSpec};
use crate::live::Hub;
use crate::model::{DOT_SPACING, GraphObject, GraphProperty, GraphType, SchemaGraph};

/// Type→property curve bend: 0 straight, toward 1 pulled to the centre.
const CURVATURE: f64 = 0.55;
/// Object dot radius (world units).
pub(crate) const DOT_R: f64 = 4.3;
const WHITE: u32 = 0xffffff;

/// What is hovered or pinned: a type, a property (by key), or one object of a type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Sel {
    Type(usize),
    Prop(String),
    Obj(usize, usize),
}

/// A graph plus its lookups.
pub(crate) struct Built {
    pub g: SchemaGraph,
    /// Every object in the graph by id, with its type: where a connection line ends.
    pub obj_index: HashMap<String, (usize, usize)>,
    prop_ix: HashMap<String, usize>,
}

impl Built {
    pub fn new(g: SchemaGraph) -> Self {
        let mut obj_index = HashMap::new();
        for t in &g.types {
            for (j, o) in t.objects.iter().enumerate() {
                obj_index.entry(o.id.clone()).or_insert((t.index, j));
            }
        }
        let prop_ix = g
            .props
            .iter()
            .enumerate()
            .map(|(i, p)| (p.key.clone(), i))
            .collect();
        Self {
            g,
            obj_index,
            prop_ix,
        }
    }

    pub fn prop_index(&self, key: &str) -> Option<usize> {
        self.prop_ix.get(key).copied()
    }

    pub fn object(&self, t: usize, j: usize) -> &GraphObject {
        &self.g.types[t].objects[j]
    }

    /// The selection's derived pieces (`selType`, `selProp`, `selObj`, `selLinks`).
    pub fn derive(&self, sel: Option<&Sel>) -> Derived {
        let sel_type = match sel {
            Some(Sel::Type(t)) | Some(Sel::Obj(t, _)) => {
                Some(*t).filter(|t| *t < self.g.types.len())
            }
            _ => None,
        };
        let sel_prop = match sel {
            Some(Sel::Prop(key)) => self.prop_index(key),
            _ => None,
        };
        let sel_obj = match (sel, sel_type) {
            (Some(Sel::Obj(_, j)), Some(t)) => {
                Some(*j).filter(|j| *j < self.g.types[t].objects.len())
            }
            _ => None,
        };
        let (mut out, mut inc) = (Vec::new(), Vec::new());
        if let (Some(t), Some(j)) = (sel_type, sel_obj) {
            let o = self.object(t, j);
            out = o
                .links
                .iter()
                .filter_map(|id| self.obj_index.get(id).copied())
                .collect();
            for ty in &self.g.types {
                for (k, other) in ty.objects.iter().enumerate() {
                    if (ty.index, k) != (t, j) && other.links.contains(&o.id) {
                        inc.push((ty.index, k));
                    }
                }
            }
        }
        Derived {
            any: sel.is_some(),
            sel: sel.cloned(),
            sel_type,
            sel_prop,
            sel_obj,
            out,
            inc,
        }
    }
}

/// The selection as the page derives it.
#[derive(Clone, Default)]
pub(crate) struct Derived {
    /// Something is hovered or pinned (`!!sel`).
    pub any: bool,
    pub sel: Option<Sel>,
    pub sel_type: Option<usize>,
    /// Index into `props`.
    pub sel_prop: Option<usize>,
    /// Object index within `sel_type`.
    pub sel_obj: Option<usize>,
    /// The selected object's connections: what it points at, and what points at it.
    pub out: Vec<(usize, usize)>,
    pub inc: Vec<(usize, usize)>,
}

impl Derived {
    pub fn type_on(&self, b: &Built, t: usize) -> bool {
        if !self.any {
            return true;
        }
        match (self.sel_type, self.sel_prop) {
            (Some(st), _) => st == t,
            (None, Some(p)) => b.g.props[p].users.contains(&t),
            _ => false,
        }
    }

    pub fn prop_on(&self, b: &Built, p: usize) -> bool {
        if !self.any {
            return true;
        }
        let key = &b.g.props[p].key;
        match (self.sel_type, self.sel_obj) {
            (Some(t), Some(j)) => b.object(t, j).held.contains(key),
            (Some(t), None) => b.g.types[t].uses(key),
            _ => self.sel_prop == Some(p),
        }
    }

    /// `!!selType && !selType.usage.has(p.key)`.
    pub fn prop_idle(&self, b: &Built, p: usize) -> bool {
        self.sel_type
            .is_some_and(|t| !b.g.types[t].uses(&b.g.props[p].key))
    }

    pub fn sel_obj_ref<'a>(&self, b: &'a Built) -> Option<&'a GraphObject> {
        Some(b.object(self.sel_type?, self.sel_obj?))
    }
}

fn xy(x: f64, y: f64) -> (f64, f64) {
    (x, y)
}

/// A point (an object dot, or a type's centre) → property, bent toward the centre.
fn edge(s: (f64, f64), p: (f64, f64), tol: f64) -> Poly {
    let c = (
        ((s.0 + p.0) / 2.) * (1. - CURVATURE),
        ((s.1 + p.1) / 2.) * (1. - CURVATURE),
    );
    quad(s, c, p, tol)
}

pub(crate) fn prop_radius(p: &GraphProperty) -> f64 {
    4. + (p.users.len() as f64).sqrt() * 2.4
}

/// A label along the radius at angle `a`; flipped on the left half so it never reads upside
/// down. `out`: text runs away from the centre.
fn radial(a: f64, dist: f64, out: bool) -> (f64, f64, Anchor, f64) {
    let (x, y) = (a.cos() * dist, a.sin() * dist);
    let deg = a.to_degrees();
    let flip = a.cos() < -0.0001;
    let anchor = if out == flip {
        Anchor::End
    } else {
        Anchor::Start
    };
    (x, y, anchor, if flip { deg + 180. } else { deg })
}

fn type_title(t: &GraphType, upper: bool) -> String {
    let name = if upper {
        t.name.to_uppercase()
    } else {
        t.name.clone()
    };
    if t.icon.is_empty() {
        name
    } else {
        format!("{} {name}", t.icon)
    }
}

/// Lite: the label's "· N not loaded" run, when the type has placeholders.
fn shadow_span(t: &GraphType, size: f64, dx: f64) -> Option<SpanSpec> {
    (t.shadow > 0).then(|| SpanSpec {
        text: format!("· {}", roostr_core::lite_ui::shadow_label(t.shadow)),
        family: Family::Sans,
        size,
        weight: 300,
        dx,
    })
}

/// Every world-space label, in [`Labels`] order: far type labels, schema type labels,
/// property labels, then the four centre texts.
pub(crate) fn label_specs(g: &SchemaGraph, prop_labels: &[String]) -> Vec<LabelSpec> {
    let mut specs = Vec::new();
    for t in &g.types {
        let (x, y, anchor, rotate) = radial(t.a, g.rt + t.rad + 40., true);
        specs.push(LabelSpec {
            x,
            y,
            anchor,
            rotate,
            central: true,
            letter_spacing: 3.,
            spans: vec![
                SpanSpec {
                    text: type_title(t, true),
                    family: Family::Sans,
                    size: 60.,
                    weight: 500,
                    dx: 0.,
                },
                SpanSpec {
                    text: t.objects.len().to_string(),
                    family: Family::Sans,
                    size: 60.,
                    weight: 400,
                    dx: 18.,
                },
            ]
            .into_iter()
            .chain(shadow_span(t, 60., 18.))
            .collect(),
        });
    }
    for t in &g.types {
        let (x, y, anchor, rotate) = radial(t.a, g.rt + t.rad + 18., true);
        specs.push(LabelSpec {
            x,
            y,
            anchor,
            rotate,
            central: true,
            letter_spacing: 0.,
            spans: vec![
                SpanSpec {
                    text: type_title(t, false),
                    family: Family::Sans,
                    size: 22.,
                    weight: 500,
                    dx: 0.,
                },
                SpanSpec {
                    text: t.objects.len().to_string(),
                    family: Family::Mono,
                    size: 17.,
                    weight: 400,
                    dx: 8.,
                },
            ]
            .into_iter()
            .chain(shadow_span(t, 17., 8.))
            .collect(),
        });
    }
    for (p, label) in g.props.iter().zip(prop_labels) {
        let (x, y, anchor, rotate) = radial(p.a, g.rp - prop_radius(p) - 14., false);
        specs.push(LabelSpec {
            x,
            y,
            anchor,
            rotate,
            central: true,
            letter_spacing: 0.,
            spans: vec![SpanSpec {
                text: label.clone(),
                family: Family::Mono,
                size: 17.,
                weight: 400,
                dx: 0.,
            }],
        });
    }
    let n = g.props.len();
    specs.push(LabelSpec::plain(
        0.,
        -10.,
        Anchor::Middle,
        Family::Sans,
        30.,
        500,
        format!("{n} properties in use"),
    ));
    specs.push(LabelSpec::plain(
        0.,
        26.,
        Anchor::Middle,
        Family::Sans,
        18.,
        400,
        "any type can hold any of them",
    ));
    specs.push(LabelSpec::plain(
        0.,
        -18.,
        Anchor::Middle,
        Family::Sans,
        96.,
        500,
        n.to_string(),
    ));
    let mut shared = LabelSpec::plain(
        0.,
        58.,
        Anchor::Middle,
        Family::Mono,
        30.,
        400,
        "SHARED PROPERTIES",
    );
    shared.letter_spacing = 4.;
    specs.push(shared);
    specs
}

/// The laid-out labels, split by role.
pub(crate) struct Labels {
    /// The property label texts they were laid out for (renames lay them out again).
    pub texts: Vec<String>,
    pub far: Vec<Label>,
    pub types: Vec<Label>,
    pub props: Vec<Label>,
    /// "N properties in use", "any type can hold any of them", "N", "SHARED PROPERTIES".
    pub center: Vec<Label>,
}

impl Labels {
    pub fn layout(g: &SchemaGraph, texts: Vec<String>) -> Self {
        let mut all: Vec<Label> = label_specs(g, &texts)
            .into_iter()
            .map(Label::layout)
            .collect();
        let (types, props) = (g.types.len(), g.props.len());
        let center = all.split_off(2 * types + props);
        let props_v = all.split_off(2 * types);
        let types_v = all.split_off(types);
        Self {
            texts,
            far: all,
            types: types_v,
            props: props_v,
            center,
        }
    }
}

/// Static geometry of a graph at one level of detail (world units).
pub(crate) struct Geo {
    pub lod: i32,
    /// Per type: its object→property curves.
    edges: Vec<Vec<Poly>>,
    /// Per type: its object dots' centres (discs of [`DOT_R`]).
    dots: Vec<Vec<[f32; 2]>>,
    /// Per type (lite): its faint "not loaded" placeholder dots' centres.
    shadow_dots: Vec<Vec<[f32; 2]>>,
    /// Per type: the cluster's circle.
    clusters: Vec<Poly>,
    /// Per type: the far-level spoke from the property ring to the cluster.
    spokes: Vec<Poly>,
    ring: Poly,
    ring_dashes: Vec<Poly>,
    props: Vec<Poly>,
    props_idle: Vec<Vec<Poly>>,
    /// Object-format properties limited to types here: the ring around their dot.
    prop_rings: Vec<Option<Poly>>,
    /// (property, its dashed curve) for each limited-to type.
    targets: Vec<(usize, Vec<Poly>)>,
}

impl Geo {
    pub fn build(b: &Built, lod: i32) -> Self {
        let g = &b.g;
        let tol = tolerance(lod);
        let mut edges = Vec::new();
        let mut dots = Vec::new();
        let mut shadow_dots = Vec::new();
        let mut clusters = Vec::new();
        let mut spokes = Vec::new();
        for t in &g.types {
            let mut curves = Vec::new();
            for o in &t.objects {
                for key in &o.held {
                    if let Some(p) = b.prop_index(key).map(|i| &g.props[i]) {
                        curves.push(edge(xy(o.x, o.y), xy(p.x, p.y), tol));
                    }
                }
            }
            edges.push(curves);
            dots.push(t.objects.iter().map(|o| [o.x as f32, o.y as f32]).collect());
            // Lite: placeholders continue the cluster's phyllotaxis past its objects.
            shadow_dots.push(
                (t.objects.len()..t.objects.len() + t.shadow_dots())
                    .map(|j| {
                        let rr = DOT_SPACING * (j as f64 + 0.5).sqrt();
                        let th = j as f64 * 2.39996;
                        [(t.x + rr * th.cos()) as f32, (t.y + rr * th.sin()) as f32]
                    })
                    .collect(),
            );
            clusters.push(circle(xy(t.x, t.y), t.rad, tol));
            spokes.push(vec![
                [
                    (t.a.cos() * (g.rp + 16.)) as f32,
                    (t.a.sin() * (g.rp + 16.)) as f32,
                ],
                [
                    (t.x - t.a.cos() * (t.rad + 8.)) as f32,
                    (t.y - t.a.sin() * (t.rad + 8.)) as f32,
                ],
            ]);
        }
        let ring = circle(xy(0., 0.), g.rp, tol);
        let ring_dashes = dash(&ring, true, &[4., 14.]);
        let mut props = Vec::new();
        let mut props_idle = Vec::new();
        let mut prop_rings = Vec::new();
        let mut targets = Vec::new();
        for (i, p) in g.props.iter().enumerate() {
            let r = prop_radius(p);
            let c = circle(xy(p.x, p.y), r, tol);
            props_idle.push(dash(&c, true, &[3., 3.]));
            props.push(c);
            prop_rings.push((!p.targets.is_empty()).then(|| circle(xy(p.x, p.y), r + 5., tol)));
            for &tt in &p.targets {
                let t = &g.types[tt];
                let ex = t.x - t.a.cos() * t.rad;
                let ey = t.y - t.a.sin() * t.rad;
                let curve = quad(
                    xy(p.x, p.y),
                    xy((p.x + ex) * 0.15, (p.y + ey) * 0.15),
                    xy(ex, ey),
                    tol,
                );
                targets.push((i, dash(&curve, false, &[2., 7.])));
            }
        }
        Self {
            lod,
            edges,
            dots,
            shadow_dots,
            clusters,
            spokes,
            ring,
            ring_dashes,
            props,
            props_idle,
            prop_rings,
            targets,
        }
    }
}

/// Highlight geometry of one selection (world units).
pub(crate) struct SelGeo {
    /// The highlighted lines: the selected object's, a type's, or every type's to the
    /// selected property.
    lit: Vec<(Color, Vec<Poly>)>,
    /// A selected type's dashed curves to the properties none of its objects hold.
    unused: Option<(Color, Vec<Poly>)>,
    /// The selected object's ring (fill colour, outline).
    marker: Option<(Color, Poly)>,
    /// Connection curves (colour, dashes), then the end dots (fill colour, outline).
    links: Vec<(Color, Vec<Poly>)>,
    link_dots: Vec<(Color, Poly)>,
}

impl SelGeo {
    pub fn build(b: &Built, d: &Derived, lod: i32) -> Self {
        let g = &b.g;
        let tol = tolerance(lod);
        let mut lit = Vec::new();
        let mut unused = None;
        let mut marker = None;
        let mut links = Vec::new();
        let mut link_dots = Vec::new();
        let held_edges = |o: &GraphObject, out: &mut Vec<Poly>| {
            for key in &o.held {
                if let Some(p) = b.prop_index(key).map(|i| &g.props[i]) {
                    out.push(edge(xy(o.x, o.y), xy(p.x, p.y), tol));
                }
            }
        };
        if d.any {
            if let (Some(o), Some(t)) = (d.sel_obj_ref(b), d.sel_type) {
                let mut curves = Vec::new();
                held_edges(o, &mut curves);
                lit.push((g.types[t].color, curves));
            } else if let Some(t) = d.sel_type {
                let ty = &g.types[t];
                let mut curves = Vec::new();
                for o in &ty.objects {
                    held_edges(o, &mut curves);
                }
                lit.push((ty.color, curves));
            } else if let Some(pi) = d.sel_prop {
                let p = &g.props[pi];
                for ty in &g.types {
                    let curves: Vec<Poly> = ty
                        .objects
                        .iter()
                        .filter(|o| o.held.contains(&p.key))
                        .map(|o| edge(xy(o.x, o.y), xy(p.x, p.y), tol))
                        .collect();
                    if !curves.is_empty() {
                        lit.push((ty.color, curves));
                    }
                }
            }
        }
        if let (Some(Sel::Type(_)), Some(t)) = (&d.sel, d.sel_type) {
            let ty = &g.types[t];
            let curves = g
                .props
                .iter()
                .filter(|p| !ty.uses(&p.key))
                .flat_map(|p| dash(&edge(xy(ty.x, ty.y), xy(p.x, p.y), tol), false, &[3., 9.]))
                .collect();
            unused = Some((fade(ty.color, 0.28), curves));
        }
        if let (Some(o), Some(t)) = (d.sel_obj_ref(b), d.sel_type) {
            marker = Some((g.types[t].color, circle(xy(o.x, o.y), 6.5, tol)));
            let sel_color = fade(g.types[t].color, 0.85);
            for &(lt, lj) in &d.out {
                let l = b.object(lt, lj);
                let curve = quad(
                    xy(o.x, o.y),
                    xy((o.x + l.x) * 0.35, (o.y + l.y) * 0.35),
                    xy(l.x, l.y),
                    tol,
                );
                links.push((sel_color, vec![curve]));
                link_dots.push((g.types[lt].color, circle(xy(l.x, l.y), 6., tol)));
            }
            for &(lt, lj) in &d.inc {
                let l = b.object(lt, lj);
                let curve = quad(
                    xy(l.x, l.y),
                    xy((o.x + l.x) * 0.35, (o.y + l.y) * 0.35),
                    xy(o.x, o.y),
                    tol,
                );
                links.push((
                    fade(g.types[lt].color, 0.85),
                    dash(&curve, false, &[5., 4.]),
                ));
                link_dots.push((g.types[lt].color, circle(xy(l.x, l.y), 6., tol)));
            }
        }
        Self {
            lit,
            unused,
            marker,
            links,
            link_dots,
        }
    }
}

/// Layer opacities for a scale (`applyView`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layers {
    pub far: f64,
    pub schema: f64,
    pub obj: f64,
    pub schema_labels: f64,
}

impl Layers {
    pub fn at(s: f64) -> Self {
        let ramp = |a: f64, b: f64| ((s - a) / (b - a)).clamp(0., 1.);
        let schema = ramp(0.36, 0.52);
        let obj = ramp(1.4, 1.9);
        Self {
            far: 1. - schema,
            schema,
            obj,
            schema_labels: schema * (1. - obj * 0.35),
        }
    }
}

/// What the world layers draw from: the graph, its labels and geometry, and the selection.
pub(crate) struct Scene<'a> {
    pub b: &'a Built,
    pub labels: &'a Labels,
    pub geo: &'a Geo,
    pub sel_geo: &'a SelGeo,
    pub d: &'a Derived,
}

/// Elements drawn in order, one after another.
fn stack(parts: impl IntoIterator<Item = El>) -> El {
    parts.into_iter().fold(el(), El::child)
}

impl Scene<'_> {
    /// The schema layer's object→property lines (stroke 0.7) at rest or dimmed.
    pub fn edges(&self) -> El {
        let mut pen = Pen::new();
        for t in &self.b.g.types {
            let color = fade(t.color, if self.d.any { 0.025 } else { 0.12 });
            for curve in &self.geo.edges[t.index] {
                pen.stroke(curve, 0.7, LineCap::Butt, color);
            }
        }
        pen.build()
    }

    /// The schema layer's highlights: lit lines, a type's unused properties, property→type
    /// curves, then the "N properties in use" caption.
    pub fn schema(&self) -> El {
        let g = &self.b.g;
        let d = self.d;
        let mut lit = Pen::new();
        for (color, curves) in &self.sel_geo.lit {
            for c in curves {
                lit.stroke(c, 1.4, LineCap::Butt, fade(*color, 0.6));
            }
        }
        let mut unused = Pen::new();
        if let Some((color, dashes)) = &self.sel_geo.unused {
            for c in dashes {
                unused.stroke(c, 1.2, LineCap::Butt, *color);
            }
        }
        let mut targets = Pen::new();
        let sel_obj = d.sel_obj_ref(self.b);
        for (pi, dashes) in &self.geo.targets {
            let p = &g.props[*pi];
            let on = d.any
                && (d.sel_prop == Some(*pi)
                    || (d.sel_obj.is_none()
                        && d.sel_type.is_some_and(|t| g.types[t].uses(&p.key)))
                    || sel_obj.is_some_and(|o| o.held.contains(&p.key)));
            let alpha = if on {
                0.55
            } else if d.any {
                0.03
            } else {
                0.1
            };
            for c in dashes {
                targets.stroke(
                    c,
                    if on { 2. } else { 1.2 },
                    LineCap::Round,
                    hex(0xe7e9ee, alpha),
                );
            }
        }
        let captions = self
            .labels
            .center
            .iter()
            .zip([0xc9cfdc, 0x6f7686])
            .map(|(label, color)| label.el(&[hex(color, 1.)]));
        stack(
            [lit.build(), unused.build(), targets.build()]
                .into_iter()
                .chain(captions),
        )
    }

    /// The far layer: the property ring, its count, the spokes and big type labels.
    pub fn far(&self) -> El {
        let g = &self.b.g;
        let mut ring = Pen::new();
        ring.fill(&self.geo.ring, hex(WHITE, 0.02));
        let mut dashes = Pen::new();
        for c in &self.geo.ring_dashes {
            dashes.stroke(c, 3., LineCap::Butt, hex(WHITE, 0.18));
        }
        let mut spokes = Pen::new();
        for t in &g.types {
            let on = self.d.type_on(self.b, t.index);
            spokes.stroke(
                &self.geo.spokes[t.index],
                (2. + t.usage.len() as f64 * 2.2) as f32,
                LineCap::Round,
                fade(t.color, if on { 0.45 } else { 0.08 }),
            );
        }
        let count = self
            .labels
            .center
            .iter()
            .skip(2)
            .zip([0xe7e9ee, 0x7d8496])
            .map(|(label, color)| label.el(&[hex(color, 1.)]));
        let names = g.types.iter().filter_map(|t| {
            let colors = if self.d.type_on(self.b, t.index) {
                [hex(0xe7e9ee, 1.), t.color]
            } else {
                [hex(0x4a4f5c, 1.), hex(0x3a3f4a, 1.)]
            };
            Some(self.labels.far.get(t.index)?.el(&colors))
        });
        stack(
            [ring.build(), dashes.build()]
                .into_iter()
                .chain(count)
                .chain([spokes.build()])
                .chain(names),
        )
    }

    /// The type clusters (circle + dots), the selected object's ring and its connections.
    pub fn clusters(&self) -> El {
        let g = &self.b.g;
        let (mut fills, mut outlines, mut shadows, mut dots) =
            (Pen::new(), Pen::new(), Pen::new(), Pen::new());
        let (mut marker, mut marker_ring) = (Pen::new(), Pen::new());
        for t in &g.types {
            let is_sel = self.d.sel_type == Some(t.index);
            let k = if self.d.type_on(self.b, t.index) {
                1.
            } else {
                0.18
            };
            let outline = &self.geo.clusters[t.index];
            fills.fill(outline, fade(t.color, k * if is_sel { 0.14 } else { 0.06 }));
            outlines.ring(
                outline,
                if is_sel { 2.5 } else { 1.5 },
                fade(t.color, k * if is_sel { 0.7 } else { 0.22 }),
            );
            for &dot in &self.geo.shadow_dots[t.index] {
                shadows.disc(dot, DOT_R as f32, fade(t.color, k * 0.22));
            }
            for &dot in &self.geo.dots[t.index] {
                dots.disc(dot, DOT_R as f32, fade(t.color, k));
            }
            if is_sel && let Some((color, ring)) = &self.sel_geo.marker {
                marker.fill(ring, fade(*color, k));
                marker_ring.ring(ring, 1.5, hex(WHITE, k as f32));
            }
        }
        let mut links = Pen::new();
        for (color, dashes) in &self.sel_geo.links {
            for c in dashes {
                links.stroke(c, 1.6, LineCap::Butt, *color);
            }
        }
        let (mut ends, mut end_rings) = (Pen::new(), Pen::new());
        for (color, ring) in &self.sel_geo.link_dots {
            ends.fill(ring, *color);
            end_rings.ring(ring, 1.2, hex(WHITE, 1.));
        }
        stack(
            [
                fills,
                outlines,
                shadows,
                dots,
                marker,
                marker_ring,
                links,
                ends,
                end_rings,
            ]
            .map(Pen::build),
        )
    }

    /// Property dots and labels, small type labels.
    pub fn schema_labels(&self) -> El {
        let b = self.b;
        let g = &b.g;
        let d = self.d;
        let bright = hex(0xe9ebf0, 1.);
        let dim = hex(0x3a3f4a, 1.);
        let (mut fills, mut outlines) = (Pen::new(), Pen::new());
        for i in 0..g.props.len() {
            let on = d.prop_on(b, i);
            let idle = d.prop_idle(b, i);
            let outline = &self.geo.props[i];
            fills.fill(
                outline,
                if on && !idle {
                    bright
                } else {
                    hex(0x0c0e13, 1.)
                },
            );
            if idle {
                for c in &self.geo.props_idle[i] {
                    outlines.stroke(c, 2., LineCap::Butt, hex(WHITE, 0.35));
                }
            } else {
                outlines.ring(outline, 2., if on { bright } else { dim });
            }
            if let Some(ring) = &self.geo.prop_rings[i] {
                outlines.ring(ring, 1., fade(if on { bright } else { dim }, 0.5));
            }
        }
        let props = self.labels.props.iter().enumerate().map(|(i, label)| {
            let color = if d.prop_idle(b, i) {
                hex(0x6f7686, 1.)
            } else if d.prop_on(b, i) {
                hex(0xdfe2ea, 1.)
            } else {
                hex(0x3f4450, 1.)
            };
            label.el(&[color])
        });
        let types = g.types.iter().filter_map(|t| {
            let colors = if d.type_on(b, t.index) {
                [hex(0xe7e9ee, 1.), t.color]
            } else {
                [hex(0x4f5461, 1.), hex(0x3d424d, 1.)]
            };
            Some(self.labels.types.get(t.index)?.el(&colors))
        });
        stack(
            [fills.build(), outlines.build()]
                .into_iter()
                .chain(props)
                .chain(types),
        )
    }

    /// Object names right of their dots (only those in view, zoomed in).
    pub fn object_labels(&self, objs: &[(usize, usize)]) -> El {
        const SIZE: f32 = 4.2;
        let sel = self.d.sel_obj_ref(self.b).map(|o| o.id.as_str());
        let style = rune::TextStyle::new(SIZE, 400.);
        let mut layer = el();
        for &(t, j) in objs {
            let o = self.b.object(t, j);
            let color = if sel == Some(o.id.as_str()) {
                hex(WHITE, 1.)
            } else {
                hex(0xc6cbd6, 1.)
            };
            layer = layer.child(text(o.name.clone()).font(style).color(color).pin(
                Some((o.x + 6.5) as f32),
                Some(o.y as f32 - style.line_px() / 2.),
                None,
                None,
            ));
        }
        layer
    }
}

/// `radial-gradient(ellipse at 50% 45%, #141823 0%, #0c0e13 70%)` over a stage `w`×`h`.
pub(crate) fn background(stage: El, w: f32, h: f32) -> El {
    let outer = hex(0x0c0e13, 1.);
    let stage = stage.fill(outer);
    if w <= 0. || h <= 0. {
        return stage;
    }
    // GPUI's farthest-corner ellipse.
    let (rx, ry) = (
        w * 0.5 * std::f32::consts::SQRT_2,
        h * 0.55 * std::f32::consts::SQRT_2,
    );
    stage.radial_gradient(
        [0.5, 0.45],
        [rx, ry],
        [(0., hex(0x141823, 1.)), (0.7, outer), (1., outer)],
    )
}

/// This computer, live: lines to its agents (lit while they work), a pulse from each working
/// agent to the object its turn is on, and a card with its engine and harness. Drawn in stage
/// points (its strokes and card keep their size at any zoom); `project` maps world points.
pub(crate) fn hub(
    hub: &Hub,
    project: impl Fn(f64, f64) -> (f64, f64),
    s: f64,
    stage: [f32; 2],
) -> El {
    let green = hex(0x4ade80, 1.);
    let p = |(x, y): (f64, f64)| {
        let (a, b) = project(x, y);
        [a as f32, b as f32]
    };
    let at = p(hub.at);
    let dot_r = (DOT_R * s) as f32;
    let ring = |c: [f32; 2], r: f32| -> Poly {
        (0..48)
            .map(|i| {
                let th = i as f32 / 48. * std::f32::consts::TAU;
                [c[0] + r * th.cos(), c[1] + r * th.sin()]
            })
            .collect()
    };
    let mut shape = rune::Shape::new();
    for &(agent, working) in &hub.agents {
        let q = p(agent);
        shape = shape.line(
            at,
            q,
            1.5,
            LineCap::Butt,
            if working {
                fade(green, 0.75)
            } else {
                hex(WHITE, 0.16)
            },
        );
        if working {
            shape = shape.stroke_with(ring(q, dot_r.max(3.) + 4.), true, 2., LineCap::Butt, green);
        }
    }
    for &(a, o) in &hub.pulses {
        let (a, o) = (p(a), p(o));
        shape = shape.line(a, o, 1.5, LineCap::Butt, fade(green, 0.35));
        let t = hub.phase as f32;
        let q = [a[0] + (o[0] - a[0]) * t, a[1] + (o[1] - a[1]) * t];
        shape = shape.fill(ring(q, 4.), green);
    }
    let color = if hub.answering {
        green
    } else {
        hex(0x8a90a0, 1.)
    };
    let r = dot_r.max(5.) + 6.;
    if !hub.pulses.is_empty() || hub.agents.iter().any(|(_, w)| *w) {
        let k = hub.phase as f32;
        shape = shape.stroke_with(
            ring(at, r + 10. * k),
            true,
            2.,
            LineCap::Butt,
            fade(color, f64::from(1. - k)),
        );
    }
    shape = shape.stroke_with(ring(at, r), true, 2.5, LineCap::Butt, color);

    // The card, right of the computer.
    let line_h = 17f32;
    let rows = [
        (hub.name.as_str(), hex(WHITE, 1.)),
        (hub.lines[0].as_str(), hex(0xc6cbd6, 1.)),
        (hub.lines[1].as_str(), color),
    ];
    let style = rune::TextStyle::new(12., 400.).lh(line_h);
    let card = rows.iter().fold(
        rune::col()
            .pin(
                Some(at[0] + r + 12.),
                Some(at[1] - line_h * 1.5 - 8.),
                None,
                None,
            )
            .px(10.)
            .py(8.)
            .radius(8.)
            .fill(hex(0x0c0e13, 0.88)),
        |card, (line, color)| card.child(text(line.to_string()).font(style).color(*color)),
    );
    el().pin(Some(0.), Some(0.), None, None)
        .size(stage[0], stage[1])
        .child(
            rune::canvas(shape)
                .pin(Some(0.), Some(0.), None, None)
                .size(stage[0], stage[1]),
        )
        .child(card)
}

//! What the graph page's SVG draws, layer by layer (`+page.svelte` markup):
//! geometry built once per graph and level of detail, the selection's
//! highlight geometry, and the per-frame paint of each layer.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;

use gpui_kit::{
    App, Bounds, Hsla, PathBuilder, Pixels, SharedString, TextAlign, TextRun, Window, font, point,
    px,
};
use roostr_core::theme;

use crate::model::{DOT_SPACING, GraphObject, GraphProperty, GraphType, SchemaGraph};
use crate::paint::{Mesh, Screen, Shape, View, fade, hex, tolerance};
use crate::text::{Anchor, Family, Label, LabelSpec, SpanSpec};

/// Type→property curve bend: 0 straight, toward 1 pulled to the centre.
const CURVATURE: f64 = 0.55;
/// Object dot radius (world units).
const DOT_R: f64 = 4.3;

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
fn edge(shape: &mut Shape, s: (f64, f64), p: (f64, f64)) {
    let c = (
        ((s.0 + p.0) / 2.) * (1. - CURVATURE),
        ((s.1 + p.1) / 2.) * (1. - CURVATURE),
    );
    shape.quad(s, c, p);
}

pub(crate) fn prop_radius(p: &GraphProperty) -> f64 {
    4. + (p.users.len() as f64).sqrt() * 2.4
}

/// A label along the radius at angle `a`; flipped on the left half so it
/// never reads upside down. `out`: text runs away from the centre.
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

/// Every world-space label, in [`Labels`] order: far type labels, schema type
/// labels, property labels, then the four centre texts.
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
    pub far: Vec<Label>,
    pub types: Vec<Label>,
    pub props: Vec<Label>,
    /// "N properties in use", "any type can hold any of them", "N", "SHARED PROPERTIES".
    pub center: Vec<Label>,
}

impl Labels {
    pub fn split(mut all: Vec<Label>, types: usize, props: usize) -> Self {
        let center = all.split_off(2 * types + props);
        let props_v = all.split_off(2 * types);
        let types_v = all.split_off(types);
        Self {
            far: all,
            types: types_v,
            props: props_v,
            center,
        }
    }
}

/// Tessellated labels for one level of detail.
pub(crate) struct LabelGeo {
    pub lod: i32,
    far: Vec<Vec<Mesh>>,
    types: Vec<Vec<Mesh>>,
    props: Vec<Vec<Mesh>>,
    center: Vec<Vec<Mesh>>,
}

impl LabelGeo {
    pub fn build(labels: &Labels, lod: i32) -> Self {
        let tol = tolerance(lod);
        let all = |v: &[Label]| v.iter().map(|l| l.meshes(tol)).collect();
        Self {
            lod,
            far: all(&labels.far),
            types: all(&labels.types),
            props: all(&labels.props),
            center: all(&labels.center),
        }
    }
}

/// Static geometry of a graph at one level of detail.
pub(crate) struct Geo {
    pub lod: i32,
    /// Per type: its object→property lines (stroke 0.7, round caps).
    edges: Vec<Mesh>,
    /// Per type: its object dots.
    dots: Vec<Mesh>,
    /// Per type (lite): its faint "not loaded" placeholder dots.
    shadow_dots: Vec<Mesh>,
    cluster_fill: Vec<Mesh>,
    /// Per type: cluster outline at 1.5 and 2.5.
    cluster_stroke: Vec<[Mesh; 2]>,
    /// Per type: the far-level spoke from the property ring to the cluster.
    spokes: Vec<Mesh>,
    ring_fill: Mesh,
    ring_stroke: Mesh,
    prop_fill: Vec<Mesh>,
    prop_stroke: Vec<Mesh>,
    prop_stroke_idle: Vec<Mesh>,
    prop_ring: Vec<Mesh>,
    /// (property, curve at width 1.2, curve at width 2) for each limited-to type.
    targets: Vec<(usize, Mesh, Mesh)>,
}

impl Geo {
    pub fn build(b: &Built, lod: i32) -> Self {
        let g = &b.g;
        let tol = tolerance(lod);
        let mut edges = Vec::new();
        let mut dots = Vec::new();
        let mut shadow_dots = Vec::new();
        let mut cluster_fill = Vec::new();
        let mut cluster_stroke = Vec::new();
        let mut spokes = Vec::new();
        for t in &g.types {
            let mut shape = Shape::new();
            for o in &t.objects {
                for key in &o.held {
                    if let Some(p) = b.prop_index(key).map(|i| &g.props[i]) {
                        edge(&mut shape, xy(o.x, o.y), xy(p.x, p.y));
                    }
                }
            }
            let mut mesh = Mesh::default();
            mesh.stroke(&shape.build(), 0.7, true, None, tol);
            edges.push(mesh);

            let mut shape = Shape::new();
            for o in &t.objects {
                shape.circle(xy(o.x, o.y), DOT_R);
            }
            let mut mesh = Mesh::default();
            mesh.fill(&shape.build(), tol);
            dots.push(mesh);

            // Lite: placeholders continue the cluster's phyllotaxis past its objects.
            let mut shape = Shape::new();
            for j in t.objects.len()..t.objects.len() + t.shadow_dots() {
                let rr = DOT_SPACING * (j as f64 + 0.5).sqrt();
                let th = j as f64 * 2.39996;
                shape.circle(xy(t.x + rr * th.cos(), t.y + rr * th.sin()), DOT_R);
            }
            let mut mesh = Mesh::default();
            mesh.fill(&shape.build(), tol);
            shadow_dots.push(mesh);

            let mut circle = Shape::new();
            circle.circle(xy(t.x, t.y), t.rad);
            let circle = circle.build();
            let mut fill = Mesh::default();
            fill.fill(&circle, tol);
            cluster_fill.push(fill);
            let mut thin = Mesh::default();
            thin.stroke(&circle, 1.5, false, None, tol);
            let mut thick = Mesh::default();
            thick.stroke(&circle, 2.5, false, None, tol);
            cluster_stroke.push([thin, thick]);

            let mut spoke = Shape::new();
            spoke.line(
                xy(t.a.cos() * (g.rp + 16.), t.a.sin() * (g.rp + 16.)),
                xy(
                    t.x - t.a.cos() * (t.rad + 8.),
                    t.y - t.a.sin() * (t.rad + 8.),
                ),
            );
            let mut mesh = Mesh::default();
            mesh.stroke(
                &spoke.build(),
                (2. + t.usage.len() as f64 * 2.2) as f32,
                true,
                None,
                tol,
            );
            spokes.push(mesh);
        }

        let mut ring = Shape::new();
        ring.circle(xy(0., 0.), g.rp);
        let ring = ring.build();
        let mut ring_fill = Mesh::default();
        ring_fill.fill(&ring, tol);
        let mut ring_stroke = Mesh::default();
        ring_stroke.stroke(&ring, 3., false, Some(&[4., 14.]), tol);

        let mut prop_fill = Vec::new();
        let mut prop_stroke = Vec::new();
        let mut prop_stroke_idle = Vec::new();
        let mut prop_ring = Vec::new();
        let mut targets = Vec::new();
        for (i, p) in g.props.iter().enumerate() {
            let r = prop_radius(p);
            let mut c = Shape::new();
            c.circle(xy(p.x, p.y), r);
            let c = c.build();
            let mut fill = Mesh::default();
            fill.fill(&c, tol);
            prop_fill.push(fill);
            let mut stroke = Mesh::default();
            stroke.stroke(&c, 2., false, None, tol);
            prop_stroke.push(stroke);
            let mut idle = Mesh::default();
            idle.stroke(&c, 2., false, Some(&[3., 3.]), tol);
            prop_stroke_idle.push(idle);
            let mut ring = Mesh::default();
            if !p.targets.is_empty() {
                let mut rr = Shape::new();
                rr.circle(xy(p.x, p.y), r + 5.);
                ring.stroke(&rr.build(), 1., false, None, tol);
            }
            prop_ring.push(ring);
            for &tt in &p.targets {
                let t = &g.types[tt];
                let ex = t.x - t.a.cos() * t.rad;
                let ey = t.y - t.a.sin() * t.rad;
                let mut curve = Shape::new();
                curve.quad(
                    xy(p.x, p.y),
                    xy((p.x + ex) * 0.15, (p.y + ey) * 0.15),
                    xy(ex, ey),
                );
                let curve = curve.build();
                let mut thin = Mesh::default();
                thin.stroke(&curve, 1.2, true, Some(&[2., 7.]), tol);
                let mut thick = Mesh::default();
                thick.stroke(&curve, 2., true, Some(&[2., 7.]), tol);
                targets.push((i, thin, thick));
            }
        }
        Self {
            lod,
            edges,
            dots,
            shadow_dots,
            cluster_fill,
            cluster_stroke,
            spokes,
            ring_fill,
            ring_stroke,
            prop_fill,
            prop_stroke,
            prop_stroke_idle,
            prop_ring,
            targets,
        }
    }
}

/// Highlight geometry of one selection at one level of detail.
pub(crate) struct SelGeo {
    pub key: (Option<Sel>, i32),
    /// The highlighted lines: the selected object's, a type's, or every type's to the selected property.
    lit: Vec<(Hsla, Mesh)>,
    /// A selected type's dashed curves to the properties none of its objects hold.
    unused: Option<(Hsla, Mesh)>,
    /// The selected object's ring (fill, white stroke).
    marker: Option<(Hsla, Mesh, Mesh)>,
    /// Connection curves (color, mesh), then the end dots (fill color, fill, white stroke).
    links: Vec<(Hsla, Mesh)>,
    link_dots: Vec<(Hsla, Mesh, Mesh)>,
}

impl SelGeo {
    pub fn build(b: &Built, d: &Derived, lod: i32) -> Self {
        let g = &b.g;
        let tol = tolerance(lod);
        let stroke = |shape: Shape, w: f32, round: bool, dash: Option<&[f32]>| {
            let mut m = Mesh::default();
            m.stroke(&shape.build(), w, round, dash, tol);
            m
        };
        let dot = |x: f64, y: f64, r: f64, w: f32| {
            let mut c = Shape::new();
            c.circle(xy(x, y), r);
            let c = c.build();
            let mut fill = Mesh::default();
            fill.fill(&c, tol);
            let mut ring = Mesh::default();
            ring.stroke(&c, w, false, None, tol);
            (fill, ring)
        };
        let mut lit = Vec::new();
        let mut unused = None;
        let mut marker = None;
        let mut links = Vec::new();
        let mut link_dots = Vec::new();
        if d.any {
            if let (Some(o), Some(t)) = (d.sel_obj_ref(b), d.sel_type) {
                let mut shape = Shape::new();
                for key in &o.held {
                    if let Some(p) = b.prop_index(key).map(|i| &g.props[i]) {
                        edge(&mut shape, xy(o.x, o.y), xy(p.x, p.y));
                    }
                }
                lit.push((g.types[t].color, stroke(shape, 1.4, true, None)));
            } else if let Some(t) = d.sel_type {
                let ty = &g.types[t];
                let mut shape = Shape::new();
                for o in &ty.objects {
                    for key in &o.held {
                        if let Some(p) = b.prop_index(key).map(|i| &g.props[i]) {
                            edge(&mut shape, xy(o.x, o.y), xy(p.x, p.y));
                        }
                    }
                }
                lit.push((ty.color, stroke(shape, 1.4, true, None)));
            } else if let Some(pi) = d.sel_prop {
                let p = &g.props[pi];
                for ty in &g.types {
                    let mut shape = Shape::new();
                    let mut any = false;
                    for o in ty.objects.iter().filter(|o| o.held.contains(&p.key)) {
                        edge(&mut shape, xy(o.x, o.y), xy(p.x, p.y));
                        any = true;
                    }
                    if any {
                        lit.push((ty.color, stroke(shape, 1.4, true, None)));
                    }
                }
            }
        }
        if let (Some(Sel::Type(_)), Some(t)) = (&d.sel, d.sel_type) {
            let ty = &g.types[t];
            let mut shape = Shape::new();
            for p in g.props.iter().filter(|p| !ty.uses(&p.key)) {
                edge(&mut shape, xy(ty.x, ty.y), xy(p.x, p.y));
            }
            unused = Some((
                fade(ty.color, 0.28),
                stroke(shape, 1.2, false, Some(&[3., 9.])),
            ));
        }
        if let (Some(o), Some(t)) = (d.sel_obj_ref(b), d.sel_type) {
            let (fill, ring) = dot(o.x, o.y, 6.5, 1.5);
            marker = Some((g.types[t].color, fill, ring));
            let sel_color = fade(g.types[t].color, 0.85);
            for &(lt, lj) in &d.out {
                let l = b.object(lt, lj);
                let mut shape = Shape::new();
                shape.quad(
                    xy(o.x, o.y),
                    xy((o.x + l.x) * 0.35, (o.y + l.y) * 0.35),
                    xy(l.x, l.y),
                );
                links.push((sel_color, stroke(shape, 1.6, false, None)));
                let (fill, ring) = dot(l.x, l.y, 6., 1.2);
                link_dots.push((g.types[lt].color, fill, ring));
            }
            for &(lt, lj) in &d.inc {
                let l = b.object(lt, lj);
                let mut shape = Shape::new();
                shape.quad(
                    xy(l.x, l.y),
                    xy((o.x + l.x) * 0.35, (o.y + l.y) * 0.35),
                    xy(o.x, o.y),
                );
                links.push((
                    fade(g.types[lt].color, 0.85),
                    stroke(shape, 1.6, false, Some(&[5., 4.])),
                ));
                let (fill, ring) = dot(l.x, l.y, 6., 1.2);
                link_dots.push((g.types[lt].color, fill, ring));
            }
        }
        Self {
            key: (d.sel.clone(), lod),
            lit,
            unused,
            marker,
            links,
            link_dots,
        }
    }
}

/// Layer opacities for a scale (`applyView`).
#[derive(Clone, Copy)]
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

/// Everything one frame paints, shared by the layer canvases.
pub(crate) struct Frame {
    pub b: Rc<Built>,
    pub labels: Option<Rc<Labels>>,
    pub geo: Rc<Geo>,
    pub label_geo: Option<Rc<LabelGeo>>,
    pub sel_geo: Rc<SelGeo>,
    pub d: Derived,
    pub view: View,
    /// Objects whose labels show (type, index).
    pub label_objs: Vec<(usize, usize)>,
}

const WHITE: u32 = 0xffffff;

impl Frame {
    fn screen(&self, bounds: Bounds<Pixels>) -> Screen {
        Screen {
            view: self.view,
            bounds,
        }
    }

    fn paint_label(
        &self,
        meshes: &[Mesh],
        label: Option<&Label>,
        colors: &[Hsla],
        screen: &Screen,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (mesh, color) in meshes.iter().zip(colors) {
            mesh.paint(screen, *color, window);
        }
        if let Some(label) = label {
            label.paint_emoji(screen, window, cx);
        }
    }

    /// `radial-gradient(ellipse at 50% 45%, #141823 0%, #0c0e13 70%)` in steps.
    pub fn paint_background(bounds: Bounds<Pixels>, window: &mut Window) {
        const STEPS: usize = 28;
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (cx, cy) = (
            f32::from(bounds.origin.x) + w * 0.5,
            f32::from(bounds.origin.y) + h * 0.45,
        );
        // farthest-corner ellipse
        let (rx, ry) = (
            w * 0.5 * std::f32::consts::SQRT_2,
            h * 0.55 * std::f32::consts::SQRT_2,
        );
        let inner = gpui_kit::Rgba::from(hex(0x141823, 1.));
        let outer = gpui_kit::Rgba::from(hex(0x0c0e13, 1.));
        for k in (1..=STEPS).rev() {
            let f = k as f32 / STEPS as f32;
            let at = (f - 0.5 / STEPS as f32).max(0.);
            let mix = |a: f32, b: f32| a + (b - a) * at;
            let color = gpui_kit::Rgba {
                r: mix(inner.r, outer.r),
                g: mix(inner.g, outer.g),
                b: mix(inner.b, outer.b),
                a: 1.,
            };
            let (ex, ey) = (rx * 0.7 * f, ry * 0.7 * f);
            let mut path = PathBuilder::fill();
            let n = 96;
            for i in 0..=n {
                let th = i as f32 / n as f32 * std::f32::consts::TAU;
                let p = point(px(cx + ex * th.cos()), px(cy + ey * th.sin()));
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
    }

    /// The stage's 12px rounded corners, masked in the page color over the drawing.
    pub fn paint_corners(bounds: Bounds<Pixels>, window: &mut Window) {
        let r = 12f32;
        let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let (x1, y1) = (
            x0 + f32::from(bounds.size.width),
            y0 + f32::from(bounds.size.height),
        );
        for (cx, cy, sx, sy) in [
            (x0 + r, y0 + r, -1., -1.),
            (x1 - r, y0 + r, 1., -1.),
            (x0 + r, y1 - r, -1., 1.),
            (x1 - r, y1 - r, 1., 1.),
        ] {
            let mut path = PathBuilder::fill();
            path.move_to(point(px(cx + sx * r), px(cy + sy * r)));
            path.line_to(point(px(cx + sx * r), px(cy)));
            for i in 0..=12 {
                let th = i as f32 / 12. * PI as f32 / 2.;
                path.line_to(point(
                    px(cx + sx * r * th.cos()),
                    px(cy + sy * r * th.sin()),
                ));
            }
            path.line_to(point(px(cx), px(cy + sy * r)));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, theme::bg());
            }
        }
    }

    /// The schema layer: object→property lines, highlights, type→unused and
    /// property→type curves, the "N properties in use" caption.
    pub fn paint_schema(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let screen = self.screen(bounds);
        let g = &self.b.g;
        let d = &self.d;
        for t in &g.types {
            self.geo.edges[t.index].paint(
                &screen,
                fade(t.color, if d.any { 0.025 } else { 0.12 }),
                window,
            );
        }
        for (color, mesh) in &self.sel_geo.lit {
            mesh.paint(&screen, fade(*color, 0.6), window);
        }
        if let Some((color, mesh)) = &self.sel_geo.unused {
            mesh.paint(&screen, *color, window);
        }
        let sel_obj = d.sel_obj_ref(&self.b);
        for (pi, thin, thick) in &self.geo.targets {
            let p = &g.props[*pi];
            let lit = d.any
                && (d.sel_prop == Some(*pi)
                    || (d.sel_obj.is_none()
                        && d.sel_type.is_some_and(|t| g.types[t].uses(&p.key)))
                    || sel_obj.is_some_and(|o| o.held.contains(&p.key)));
            let alpha = if lit {
                0.55
            } else if d.any {
                0.03
            } else {
                0.1
            };
            (if lit { thick } else { thin }).paint(&screen, hex(0xe7e9ee, alpha), window);
        }
        if let (Some(labels), Some(geo)) = (&self.labels, &self.label_geo) {
            self.paint_label(
                &geo.center[0],
                labels.center.first(),
                &[hex(0xc9cfdc, 1.)],
                &screen,
                window,
                cx,
            );
            self.paint_label(
                &geo.center[1],
                labels.center.get(1),
                &[hex(0x6f7686, 1.)],
                &screen,
                window,
                cx,
            );
        }
    }

    /// The far layer: the property ring, its count, the spokes and big type labels.
    pub fn paint_far(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let screen = self.screen(bounds);
        let g = &self.b.g;
        self.geo.ring_fill.paint(&screen, hex(WHITE, 0.02), window);
        self.geo
            .ring_stroke
            .paint(&screen, hex(WHITE, 0.18), window);
        let geo = self.label_geo.as_ref();
        let labels = self.labels.as_ref();
        if let (Some(labels), Some(geo)) = (labels, geo) {
            self.paint_label(
                &geo.center[2],
                labels.center.get(2),
                &[hex(0xe7e9ee, 1.)],
                &screen,
                window,
                cx,
            );
            self.paint_label(
                &geo.center[3],
                labels.center.get(3),
                &[hex(0x7d8496, 1.)],
                &screen,
                window,
                cx,
            );
        }
        for t in &g.types {
            let on = self.d.type_on(&self.b, t.index);
            self.geo.spokes[t.index].paint(
                &screen,
                fade(t.color, if on { 0.45 } else { 0.08 }),
                window,
            );
            if let (Some(labels), Some(geo)) = (labels, geo) {
                let colors = if on {
                    [hex(0xe7e9ee, 1.), t.color]
                } else {
                    [hex(0x4a4f5c, 1.), hex(0x3a3f4a, 1.)]
                };
                self.paint_label(
                    &geo.far[t.index],
                    labels.far.get(t.index),
                    &colors,
                    &screen,
                    window,
                    cx,
                );
            }
        }
    }

    /// The type clusters (circle + dots), the selected object's ring and its connections.
    pub fn paint_clusters(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let screen = self.screen(bounds);
        let g = &self.b.g;
        for t in &g.types {
            let is_sel = self.d.sel_type == Some(t.index);
            let k = if self.d.type_on(&self.b, t.index) {
                1.
            } else {
                0.18
            };
            self.geo.cluster_fill[t.index].paint(
                &screen,
                fade(t.color, k * if is_sel { 0.14 } else { 0.06 }),
                window,
            );
            self.geo.cluster_stroke[t.index][usize::from(is_sel)].paint(
                &screen,
                fade(t.color, k * if is_sel { 0.7 } else { 0.22 }),
                window,
            );
            self.geo.shadow_dots[t.index].paint(&screen, fade(t.color, k * 0.22), window);
            self.geo.dots[t.index].paint(&screen, fade(t.color, k), window);
            if is_sel && let Some((color, fill, ring)) = &self.sel_geo.marker {
                fill.paint(&screen, fade(*color, k), window);
                ring.paint(&screen, hex(WHITE, k as f32), window);
            }
        }
        for (color, mesh) in &self.sel_geo.links {
            mesh.paint(&screen, *color, window);
        }
        for (color, fill, ring) in &self.sel_geo.link_dots {
            fill.paint(&screen, *color, window);
            ring.paint(&screen, hex(WHITE, 1.), window);
        }
    }

    /// Property dots and labels, small type labels.
    pub fn paint_schema_labels(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let screen = self.screen(bounds);
        let b = &self.b;
        let g = &b.g;
        let d = &self.d;
        for (i, _) in g.props.iter().enumerate() {
            let on = d.prop_on(b, i);
            let idle = d.prop_idle(b, i);
            let bright = hex(0xe9ebf0, 1.);
            let dim = hex(0x3a3f4a, 1.);
            self.geo.prop_fill[i].paint(
                &screen,
                if on && !idle {
                    bright
                } else {
                    hex(0x0c0e13, 1.)
                },
                window,
            );
            if idle {
                self.geo.prop_stroke_idle[i].paint(&screen, hex(WHITE, 0.35), window);
            } else {
                self.geo.prop_stroke[i].paint(&screen, if on { bright } else { dim }, window);
            }
            self.geo.prop_ring[i].paint(&screen, fade(if on { bright } else { dim }, 0.5), window);
            if let (Some(labels), Some(geo)) = (&self.labels, &self.label_geo) {
                let color = if idle {
                    hex(0x6f7686, 1.)
                } else if on {
                    hex(0xdfe2ea, 1.)
                } else {
                    hex(0x3f4450, 1.)
                };
                if let Some(meshes) = geo.props.get(i) {
                    self.paint_label(meshes, labels.props.get(i), &[color], &screen, window, cx);
                }
            }
        }
        if let (Some(labels), Some(geo)) = (&self.labels, &self.label_geo) {
            for t in &g.types {
                let on = d.type_on(b, t.index);
                let colors = if on {
                    [hex(0xe7e9ee, 1.), t.color]
                } else {
                    [hex(0x4f5461, 1.), hex(0x3d424d, 1.)]
                };
                self.paint_label(
                    &geo.types[t.index],
                    labels.types.get(t.index),
                    &colors,
                    &screen,
                    window,
                    cx,
                );
            }
        }
    }

    /// Object names right of their dots (only those in view, zoomed in).
    pub fn paint_object_labels(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let screen = self.screen(bounds);
        let size = px((4.2 * self.view.s) as f32);
        let sel = self.d.sel_obj_ref(&self.b).map(|o| o.id.as_str());
        for &(t, j) in &self.label_objs {
            let o = self.b.object(t, j);
            let color = if sel == Some(o.id.as_str()) {
                hex(WHITE, 1.)
            } else {
                hex(0xc6cbd6, 1.)
            };
            let text = SharedString::from(o.name.clone());
            let run = TextRun {
                len: text.len(),
                font: font(theme::fonts::UI),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(text, size, &[run], None);
            let height = line.ascent + line.descent;
            let at = screen.project(o.x + 6.5, o.y);
            let origin = point(at.x, at.y - height / 2.);
            if origin.x > bounds.origin.x + bounds.size.width
                || origin.x + line.width < bounds.origin.x
            {
                continue;
            }
            let _ = line.paint(origin, height, TextAlign::Left, None, window, cx);
        }
    }
}

//! The space's schema graph (`src/lib/graph.ts`): types on an outer ring (each
//! a cluster of its objects, sized by count), the properties those objects
//! use on an inner ring, and a curve from each type to each property its
//! objects hold, weighted by how many of them do. Object-format properties
//! limited to a type also point at that type.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::f64::consts::PI;

use gpui_kit::Hsla;
use roostr_core::objects::kind_name;
use roostr_core::{Fields, QueryRow, RelationDef, TypeDef, Value};

use crate::paint::oklch;

/// Spacing between neighbouring object dots inside a cluster (world units).
pub(crate) const DOT_SPACING: f64 = 13.;
/// Lite: at most this many faint placeholder dots per type; the label says how many.
pub(crate) const SHADOW_DOTS: usize = 400;

pub(crate) struct GraphObject {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// Keys of the properties this object holds a value for (field order).
    pub held: BTreeSet<String>,
    pub fields: Fields,
    /// Objects in this graph it points at: link properties, links/mentions/files in its text, a collection's members.
    pub links: Vec<String>,
}

pub(crate) struct GraphType {
    pub index: usize,
    pub key: String,
    pub name: String,
    pub icon: String,
    pub color: Hsla,
    /// Cluster radius (world units).
    pub rad: f64,
    /// Angle on the outer ring; x/y is the cluster centre.
    pub a: f64,
    pub x: f64,
    pub y: f64,
    pub objects: Vec<GraphObject>,
    /// Lite: objects of this type the space's directory counts but this
    /// device has not loaded (faint "N not loaded" placeholders after the dots).
    pub shadow: u64,
    /// Property key → share of this type's objects holding it (0..1], in first-seen order.
    pub usage: Vec<(String, f64)>,
    usage_ix: HashMap<String, usize>,
}

impl GraphType {
    /// Placeholder dots drawn (capped at [`SHADOW_DOTS`]).
    pub fn shadow_dots(&self) -> usize {
        (self.shadow as usize).min(SHADOW_DOTS)
    }

    pub fn uses(&self, key: &str) -> bool {
        self.usage_ix.contains_key(key)
    }

    pub fn usage_of(&self, key: &str) -> Option<f64> {
        self.usage_ix.get(key).map(|&i| self.usage[i].1)
    }
}

pub(crate) struct GraphProperty {
    pub key: String,
    pub name: String,
    pub format: String,
    pub icon: String,
    pub a: f64,
    pub x: f64,
    pub y: f64,
    /// Indices of the types whose objects use it.
    pub users: Vec<usize>,
    /// Object-format property limited to types present here (type indices).
    pub targets: Vec<usize>,
}

pub(crate) struct SchemaGraph {
    pub types: Vec<GraphType>,
    pub props: Vec<GraphProperty>,
    /// Space properties no object here uses (the type panel's "available, unused").
    pub idle: Vec<RelationDef>,
    pub object_count: usize,
    /// Lite: objects in the space not loaded on this device (all types).
    pub shadow_count: u64,
    /// Outer (types) and inner (properties) ring radii.
    pub rt: f64,
    pub rp: f64,
    /// Radius that contains everything, labels included: what "fit" frames.
    pub extent: f64,
}

fn has_value(v: Option<&Value>) -> bool {
    let Some(v) = v else { return false };
    if let Some(s) = &v.string_value {
        return !s.is_empty();
    }
    if let Some(b) = v.bool_value {
        return b;
    }
    if v.int_value.is_some() || v.float_value.is_some() || v.link_value.is_some() {
        return true;
    }
    if let Some(values) = &v.values_value {
        return values.items.iter().any(|it| has_value(Some(it)));
    }
    if let Some(list) = &v.list_value {
        return !list.values.is_empty();
    }
    if let Some(map) = &v.map_value {
        return !map.entries.is_empty();
    }
    false
}

/// Every object id `fields` points at: link values anywhere, object-format property ids, `collectionIds`.
fn outgoing_ids(fields: &Fields, object_keys: &HashSet<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    };
    for (k, v) in fields {
        let single = std::slice::from_ref(v);
        let items = v
            .values_value
            .as_ref()
            .map_or(single, |vv| vv.items.as_slice());
        for it in items {
            if let Some(link) = it.link_value.as_ref().filter(|l| !l.target_id.is_empty()) {
                push(&link.target_id);
            } else if let Some(s) = it.string_value.as_deref().filter(|s| !s.is_empty())
                && (object_keys.contains(k.as_str()) || k == "collectionIds")
            {
                push(s);
            }
        }
    }
    out
}

/// `String.prototype.localeCompare` for names: case-insensitive first, then
/// lower case ahead of upper case (ICU's tertiary order).
pub(crate) fn locale_cmp(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<String>();
    fold(a).cmp(&fold(b)).then_with(|| {
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y {
                return match (x.is_lowercase(), y.is_lowercase()) {
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    _ => x.cmp(&y),
                };
            }
        }
        a.len().cmp(&b.len())
    })
}

fn field_string<'a>(fields: &'a Fields, key: &str) -> Option<&'a str> {
    fields.get(key).and_then(|v| v.string_value.as_deref())
}

/// Builds the schema graph of one space from its objects (`rows`: every
/// object of the vault, `fetchAllQuery({})`) - every object, its types,
/// properties, agents, skills and templates included. `relations` are the
/// space's own definitions (`spaceRelations`), `types` the store's. `counts`
/// (lite only, else empty) are the directory's objects per type key: what
/// exceeds the loaded objects becomes each type's shadow placeholders.
pub(crate) fn build_schema_graph(
    rows: Vec<QueryRow>,
    channel_id: &str,
    is_default_channel: bool,
    relations: &[RelationDef],
    types: &[TypeDef],
    counts: &HashMap<String, u64>,
) -> SchemaGraph {
    let rows: Vec<QueryRow> = rows
        .into_iter()
        .filter(|r| {
            let ch = field_string(&r.fields, "channel").unwrap_or("");
            ch == channel_id || (ch.is_empty() && is_default_channel)
        })
        .collect();
    let object_count = rows.len();
    // The space's properties, minus hidden plumbing; only these count as "held".
    let rel_by_key: Vec<&RelationDef> = relations.iter().filter(|r| !r.hidden).collect();
    let rel_keys: HashSet<&str> = rel_by_key.iter().map(|r| r.key.as_str()).collect();
    let type_def = |key: &str| -> Option<&TypeDef> {
        types
            .iter()
            .find(|t| t.key == key && t.space == channel_id)
            .or_else(|| types.iter().find(|t| t.key == key))
    };

    let object_keys: HashSet<&str> = relations
        .iter()
        .filter(|r| r.format == "object")
        .map(|r| r.key.as_str())
        .collect();
    let in_graph: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut by_kind: Vec<(String, Vec<GraphObject>)> = Vec::new();
    for r in rows {
        let held: BTreeSet<String> = r
            .fields
            .iter()
            .filter(|(k, v)| rel_keys.contains(k.as_str()) && has_value(Some(v)))
            .map(|(k, _)| k.clone())
            .collect();
        let name_part = field_string(&r.fields, "name")
            .filter(|s| !s.is_empty())
            .unwrap_or("Untitled");
        let name = match field_string(&r.fields, "iconEmoji").filter(|s| !s.is_empty()) {
            Some(icon) => format!("{icon} {name_part}"),
            None => name_part.to_string(),
        };
        let links = outgoing_ids(&r.fields, &object_keys)
            .into_iter()
            .filter(|id| *id != r.id && in_graph.contains(id))
            .collect();
        let obj = GraphObject {
            id: r.id,
            name,
            x: 0.,
            y: 0.,
            held,
            fields: r.fields,
            links,
        };
        match by_kind.iter_mut().find(|(k, _)| *k == r.type_key) {
            Some((_, list)) => list.push(obj),
            None => by_kind.push((r.type_key, vec![obj])),
        }
    }
    let loaded: HashMap<String, u64> = by_kind
        .iter()
        .map(|(k, list)| (k.clone(), list.len() as u64))
        .collect();
    let shadows: HashMap<String, u64> = roostr_core::lite_ui::shadow_counts(counts, &loaded)
        .into_iter()
        .collect();
    for key in shadows.keys() {
        if !by_kind.iter().any(|(k, _)| k == key) {
            by_kind.push((key.clone(), Vec::new()));
        }
    }
    let shadow_count = shadows.values().sum();

    // ── Types on the outer ring, each an arc as wide as its cluster ──
    let sort_name = |key: &str| type_def(key).map_or_else(|| key.to_string(), |d| d.name.clone());
    by_kind.sort_by(|a, b| locale_cmp(&sort_name(&a.0), &sort_name(&b.0)));
    let n = by_kind.len();
    let mut gtypes: Vec<GraphType> = by_kind
        .into_iter()
        .enumerate()
        .map(|(index, (key, objects))| {
            let mut usage: Vec<(String, f64)> = Vec::new();
            let mut usage_ix: HashMap<String, usize> = HashMap::new();
            for o in &objects {
                for k in &o.held {
                    match usage_ix.get(k) {
                        Some(&i) => usage[i].1 += 1.,
                        None => {
                            usage_ix.insert(k.clone(), usage.len());
                            usage.push((k.clone(), 1.));
                        }
                    }
                }
            }
            let count = objects.len() as f64;
            for u in &mut usage {
                u.1 /= count;
            }
            let shadow = shadows.get(&key).copied().unwrap_or(0);
            let dots = count + (shadow as usize).min(SHADOW_DOTS) as f64;
            let hue = ((index as f64 / n.max(1) as f64) * 330. + 15.) % 360.;
            let def = type_def(&key);
            let name = match def.map(|d| d.name.as_str()).filter(|s| !s.is_empty()) {
                Some(name) => name.to_string(),
                None if !key.is_empty() => kind_name(&key),
                None => "Untyped".to_string(),
            };
            GraphType {
                index,
                icon: def.map(|d| d.icon.clone()).unwrap_or_default(),
                key,
                name,
                color: oklch(0.76, 0.13, hue),
                rad: dots.sqrt() * DOT_SPACING + 18.,
                a: 0.,
                x: 0.,
                y: 0.,
                objects,
                shadow,
                usage,
                usage_ix,
            }
        })
        .collect();
    let sum_d: f64 = gtypes.iter().map(|t| 2. * t.rad).sum();
    // Room for every cluster plus a gap; never tighter than the design's ring.
    let rt = f64::max(1150., (sum_d * 1.35) / (2. * PI));
    let gap = (2. * PI * rt - sum_d) / n.max(1) as f64;
    let mut acc = -gtypes.first().map_or(0., |t| t.rad);
    for t in &mut gtypes {
        let s = acc + t.rad;
        acc += 2. * t.rad + gap;
        t.a = -PI / 2. + s / rt;
        t.x = t.a.cos() * rt;
        t.y = t.a.sin() * rt;
        // Phyllotaxis: evenly packed, densest at the centre.
        for (j, o) in t.objects.iter_mut().enumerate() {
            let rr = DOT_SPACING * (j as f64 + 0.5).sqrt();
            let th = j as f64 * 2.39996;
            o.x = t.x + rr * th.cos();
            o.y = t.y + rr * th.sin();
        }
    }

    // ── Properties in use, on the inner ring near the types that use them ──
    let mut type_by_id: HashMap<&str, usize> = HashMap::new();
    for t in &gtypes {
        if let Some(def) = type_def(&t.key) {
            type_by_id.insert(def.id.as_str(), t.index);
        }
        for d in types.iter().filter(|d| d.key == t.key) {
            type_by_id.insert(d.id.as_str(), t.index);
        }
    }
    let mut props: Vec<(GraphProperty, f64)> = Vec::new();
    for rel in &rel_by_key {
        let key = rel.key.as_str();
        let users: Vec<usize> = gtypes
            .iter()
            .filter(|t| t.uses(key))
            .map(|t| t.index)
            .collect();
        if users.is_empty() {
            continue;
        }
        let (mut cx, mut cy) = (0., 0.);
        for &i in &users {
            let t = &gtypes[i];
            let w = t.usage_of(key).unwrap_or(0.) * (t.objects.len() as f64).sqrt();
            cx += t.a.cos() * w;
            cy += t.a.sin() * w;
        }
        // A property nearly every type uses sits at the top, not at a meaningless mean.
        let mean = if users.len() as f64 >= n as f64 * 0.9 {
            -PI / 2. + 0.001
        } else {
            f64::atan2(cy, cx)
        };
        let mut targets: Vec<usize> = Vec::new();
        if rel.format == "object" {
            for id in rel.object_types.iter().flatten() {
                if let Some(&t) = type_by_id.get(id.as_str())
                    && !targets.contains(&t)
                {
                    targets.push(t);
                }
            }
        }
        let name = if rel.name.is_empty() {
            key.to_string()
        } else {
            rel.name.clone()
        };
        props.push((
            GraphProperty {
                key: key.to_string(),
                name,
                format: rel.format.clone(),
                icon: rel.icon_emoji.clone().unwrap_or_default(),
                a: 0.,
                x: 0.,
                y: 0.,
                users,
                targets,
            },
            mean,
        ));
    }
    props.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Label room: ~26 world units of arc per property.
    let rp = f64::max(440., (props.len() as f64 * 26.) / (2. * PI));
    let count = props.len() as f64;
    let props: Vec<GraphProperty> = props
        .into_iter()
        .enumerate()
        .map(|(k, (mut p, _))| {
            p.a = -PI / 2. + ((k as f64 + 0.5) * 2. * PI) / count;
            p.x = p.a.cos() * rp;
            p.y = p.a.sin() * rp;
            p
        })
        .collect();
    let used: HashSet<&str> = props.iter().map(|p| p.key.as_str()).collect();
    let idle = rel_by_key
        .iter()
        .filter(|r| !used.contains(r.key.as_str()))
        .map(|r| (*r).clone())
        .collect();
    let max_rad = gtypes.iter().fold(0., |m: f64, t| m.max(t.rad));
    SchemaGraph {
        types: gtypes,
        props,
        idle,
        object_count,
        shadow_count,
        rt,
        rp,
        extent: rt + max_rad + 420.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, type_key: &str) -> QueryRow {
        QueryRow {
            id: id.into(),
            type_key: type_key.into(),
            ..Default::default()
        }
    }

    #[test]
    fn lite_shadows_per_type() {
        let rows = vec![row("a", "page"), row("b", "page"), row("c", "task")];
        let counts = HashMap::from([
            ("page".to_string(), 5),
            ("task".to_string(), 1),
            ("note".to_string(), 3),
        ]);
        let g = build_schema_graph(rows, "s", true, &[], &[], &counts);
        let shadow = |key: &str| {
            g.types
                .iter()
                .find(|t| t.key == key)
                .map(|t| (t.objects.len(), t.shadow))
        };
        assert_eq!(shadow("page"), Some((2, 3)));
        assert_eq!(shadow("task"), Some((1, 0)));
        // A type with nothing loaded is a ghost cluster of its own.
        assert_eq!(shadow("note"), Some((0, 3)));
        assert_eq!(g.shadow_count, 6);
        assert_eq!(g.object_count, 3);
    }

    #[test]
    fn full_space_has_no_shadows() {
        let g = build_schema_graph(vec![row("a", "page")], "s", true, &[], &[], &HashMap::new());
        assert_eq!(g.shadow_count, 0);
        assert!(g.types.iter().all(|t| t.shadow == 0));
    }
}

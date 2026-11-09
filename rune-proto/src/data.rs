//! What the prototype shows: the local glon daemon's spaces, pins, pages and
//! queries, read-only, or a small sample when no daemon answers.

use serde_json::{Value, json};

/// A space and the objects pinned in its sidebar.
pub struct Space {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub pins: Vec<Pin>,
}

pub struct Pin {
    pub id: String,
    pub name: String,
    pub icon: String,
}

/// A block of a page body, in the engine's text styles (roostr-core `style`).
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Paragraph,
    H1,
    H2,
    H3,
    Quote,
    Code,
    Bullet,
    Numbered,
    Check(bool),
    Divider,
}

pub struct Block {
    pub kind: Kind,
    pub text: String,
}

/// An object opened in the main pane.
pub struct Object {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub type_name: String,
    /// (property name, shown value) for the properties under the title.
    pub props: Vec<(String, String)>,
    pub body: Vec<Block>,
    /// A query or collection: its columns and rows.
    pub set: Option<Set>,
    /// A query's source types, by name.
    pub sources: Vec<String>,
}

/// A query or collection as its table shows it.
pub struct Set {
    /// The Name column's width.
    pub name_width: f32,
    /// The other columns, in the view's order.
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

pub struct Column {
    pub key: String,
    pub name: String,
    pub icon: String,
    pub width: f32,
}

pub struct Row {
    pub id: String,
    pub icon: String,
    pub name: String,
    /// One per column.
    pub cells: Vec<Cell>,
}

/// A cell's value as Roostr draws it.
#[derive(Clone)]
pub enum Cell {
    Empty,
    Text(String),
    /// Linked objects, each a pill with its icon.
    Objects(Vec<Pill>),
    /// Tag or status options: (text, colour name).
    Tags(Vec<(String, String)>),
    Check(bool),
}

#[derive(Clone)]
pub struct Pill {
    pub icon: String,
    pub name: String,
}

impl Cell {
    /// The words it shows, for sorting.
    pub fn sort_text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(t) => t.clone(),
            Cell::Objects(p) => p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
            Cell::Tags(t) => t.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", "),
            Cell::Check(b) => if *b { "1".into() } else { String::new() },
        }
    }
}

/// A relation as the table reads it.
struct Rel {
    name: String,
    icon: String,
    format: String,
    /// Option text -> colour name.
    colors: std::collections::HashMap<String, String>,
    /// The types an object property links to, by type object id.
    object_types: Vec<String>,
}

/// Width of a column the view does not size.
const DEFAULT_WIDTH: f32 = 150.0;
const MIN_WIDTH: f32 = 60.0;
const NAME_MIN_WIDTH: f32 = 180.0;
/// Types every unsourced query leaves out (roostr-core SYSTEM_TYPE_KEYS, the visible ones).
const SYSTEM_TYPES: &[&str] = &["relation", "type", "template", "channel", "skill", "tool", "credential", "system_prompt", "capability", "machine", "program", "typescript", "json", "proto", "pinned_fact", "milestone", "user_prefs", "device"];

/// The local daemon, when one answers.
pub struct Daemon {
    base: String,
    token: String,
    /// relation key -> name, for property labels and column headers.
    names: std::collections::HashMap<String, String>,
    /// relation key -> format ("date", "object", …), for how a value reads.
    formats: std::collections::HashMap<String, String>,
    /// (space, relation key) -> how the table draws it: each space has its own copy.
    rels: std::collections::HashMap<(String, String), Rel>,
    /// type key -> its icon, for objects without their own.
    type_icons: std::collections::HashMap<String, String>,
    /// type object id -> its icon, for a property limited to that type.
    type_icons_by_id: std::collections::HashMap<String, String>,
    /// object id -> (name, icon), for links.
    titles: std::cell::RefCell<std::collections::HashMap<String, (String, String)>>,
}

impl Daemon {
    pub fn connect() -> Option<Daemon> {
        let home = std::env::var("HOME").ok()?;
        let token = std::fs::read_to_string(format!("{home}/.glon/api-token")).ok()?.trim().to_string();
        let mut d = Daemon { base: "http://127.0.0.1:7333".into(), token, names: Default::default(), formats: Default::default(), rels: Default::default(), type_icons: Default::default(), type_icons_by_id: Default::default(), titles: Default::default() };
        let rels = d.get("/api/relations")?;
        for r in rels.as_array()? {
            if let (Some(k), Some(n)) = (r["key"].as_str(), r["name"].as_str()) {
                d.names.entry(k.to_string()).or_insert_with(|| n.to_string());
                d.formats.entry(k.to_string()).or_insert_with(|| r["format"].as_str().unwrap_or_default().to_string());
                d.rels.entry((str_of(r, "space"), k.to_string())).or_insert_with(|| Rel {
                    name: n.to_string(),
                    icon: r["iconEmoji"].as_str().unwrap_or_default().to_string(),
                    format: r["format"].as_str().unwrap_or_default().to_string(),
                    colors: r["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| Some((o["text"].as_str()?.to_string(), o["color"].as_str().unwrap_or_default().to_string())))
                        .collect(),
                    object_types: r["objectTypes"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect(),
                });
            }
        }
        if let Some(types) = d.post("/api/query", json!({"type": "type", "limit": 1000})) {
            for t in types["records"].as_array().into_iter().flatten() {
                let key = t["fields"]["key"]["stringValue"].as_str().unwrap_or_default();
                let icon = t["fields"]["iconEmoji"]["stringValue"].as_str().unwrap_or_default();
                if !key.is_empty() && !icon.is_empty() {
                    d.type_icons.entry(key.to_string()).or_insert_with(|| icon.to_string());
                    d.type_icons_by_id.insert(str_of(t, "id"), icon.to_string());
                }
            }
        }
        Some(d)
    }

    fn get(&self, path: &str) -> Option<Value> {
        ureq::get(format!("{}{path}", self.base))
            .header("Authorization", format!("Bearer {}", self.token))
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()
    }

    fn post(&self, path: &str, body: Value) -> Option<Value> {
        ureq::post(format!("{}{path}", self.base))
            .header("Authorization", format!("Bearer {}", self.token))
            .send_json(body)
            .ok()?
            .body_mut()
            .read_json()
            .ok()
    }

    pub fn spaces(&self) -> Vec<Space> {
        let Some(list) = self.get("/api/channels") else { return Vec::new() };
        list.as_array()
            .into_iter()
            .flatten()
            .map(|c| Space {
                id: str_of(c, "id"),
                name: str_of(c, "name"),
                icon: str_of(c, "icon"),
                pins: c["pinnedIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str())
                    .filter_map(|id| {
                        let o = self.get(&format!("/api/objects/{id}"))?;
                        Some(Pin { id: id.to_string(), name: name_of(&o), icon: field_str(&o, "iconEmoji") })
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn object(&self, id: &str) -> Option<Object> {
        let o = self.get(&format!("/api/objects/{id}"))?;
        let type_key = str_of(&o, "typeKey");
        let set = matches!(type_key.as_str(), "query" | "collection").then(|| self.set(&o)).flatten();
        let skip = ["name", "iconEmoji", "channel", "agent", "viewRelations", "viewFilters", "viewSorts", "setOf", "cover", "cover_y", "pin_order", "pin_collapsed", "links", "last_run", "repeat", "repeat_instructions", "assignee", "error", "run_active", "run_now", "collectionIds"];
        let props = o["fields"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !skip.contains(&k.as_str()) && !k.starts_with("key_") && !k.starts_with("view") && k.as_str() != "collectionIds")
            .map(|(k, v)| (self.names.get(k).cloned().unwrap_or_else(|| k.clone()), self.read(k, v)))
            .filter(|(_, v)| !v.is_empty())
            .take(8)
            .collect();
        let sources = o["fields"]["setOf"]["valuesValue"]["items"].as_array().into_iter().flatten().filter_map(|i| i["stringValue"].as_str()).map(|t| format!("{} {}", self.type_icons.get(t).cloned().unwrap_or_default(), type_name(t)).trim().to_string()).collect();
        Some(Object { id: id.to_string(), name: name_of(&o), icon: field_str(&o, "iconEmoji"), type_name: type_key, props, body: blocks(&o), set, sources })
    }

    /// A query's rows (its source types, filters and sorts, in its space) or a collection's
    /// members, with the view's columns and widths.
    fn set(&self, o: &Value) -> Option<Set> {
        let fields = &o["fields"];
        let items = |k: &str| fields[k]["valuesValue"]["items"].as_array().cloned().unwrap_or_default();
        let entry = |v: &Value, k: &str| v["mapValue"]["entries"][k].clone();
        let mut name_width = 200.0f32;
        let mut columns = Vec::new();
        for c in items("viewRelations") {
            let key = entry(&c, "key")["stringValue"].as_str().unwrap_or_default().to_string();
            let width = entry(&c, "width")["intValue"].as_i64().map(|w| w as f32);
            if key == "name" {
                name_width = width.unwrap_or(name_width).max(NAME_MIN_WIDTH);
                continue;
            }
            if key.is_empty() || entry(&c, "isHidden")["boolValue"] == true {
                continue;
            }
            let (name, icon) = match key.as_str() {
                "type" | "typeKey" => ("Type".to_string(), "🧩".to_string()),
                "updatedAt" => ("Modified date".to_string(), "🗓️".to_string()),
                "createdAt" => ("Created date".to_string(), "📅".to_string()),
                k => self.rel(&field_str(o, "channel"), k).map_or((k.to_string(), String::new()), |r| (r.name.clone(), self.rel_icon(r))),
            };
            columns.push(Column { key, name, icon, width: width.unwrap_or(DEFAULT_WIDTH).max(MIN_WIDTH) });
        }
        let space = json!({"key": "channel", "condition": "equal", "value": field_str(o, "channel")});
        let filters = if str_of(o, "typeKey") == "collection" {
            let members: Vec<String> = items("collectionIds").iter().filter_map(|i| i["stringValue"].as_str().or(i["linkValue"]["targetId"].as_str()).map(str::to_string)).collect();
            vec![json!({"key": "id", "condition": "in", "value": members})]
        } else {
            let types: Vec<String> = items("setOf").iter().filter_map(|i| i["stringValue"].as_str().map(str::to_string)).collect();
            let mut f = self.engine_filters(&items("viewFilters"));
            f.push(if types.is_empty() { json!({"key": "typeKey", "condition": "notIn", "value": SYSTEM_TYPES}) } else { json!({"key": "typeKey", "condition": "in", "value": types}) });
            f.push(space);
            f
        };
        let sorts: Vec<Value> = items("viewSorts")
            .iter()
            .filter_map(|r| {
                let key = entry(r, "key")["stringValue"].as_str()?.to_string();
                let dir = entry(r, "type")["stringValue"].as_str().unwrap_or("asc").to_string();
                let empty = if entry(r, "empty")["stringValue"] == "start" { "start" } else { "end" };
                Some(json!({"key": key, "type": dir, "emptyPlacement": empty}))
            })
            .collect();
        // No sort of its own: newest change first, as Roostr's table does.
        let sorts = if sorts.is_empty() { vec![json!({"key": "updatedAt", "type": "desc", "emptyPlacement": "end"})] } else { sorts };
        let body = json!({"filters": filters, "sorts": sorts, "limit": 500});
        let res = self.post("/api/query", body)?;
        let rows = res["records"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| Row {
                id: str_of(r, "id"),
                icon: self.icon_of(r),
                name: name_of(r),
                cells: columns.iter().map(|c| self.cell(r, &c.key, &field_str(o, "channel"))).collect(),
            })
            .collect();
        Some(Set { name_width, columns, rows })
    }

    /// A view's stored filters as engine filters (roostr-core `engine_filters_of`).
    fn engine_filters(&self, rules: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        for r in rules {
            let e = &r["mapValue"]["entries"];
            let key = e["key"]["stringValue"].as_str().unwrap_or_default();
            if key.is_empty() || key == "serving" {
                continue;
            }
            let condition = e["condition"]["stringValue"].as_str().unwrap_or("equal");
            let values: Vec<&str> = e["value"]["valuesValue"]["items"].as_array().into_iter().flatten().filter_map(|i| i["stringValue"].as_str()).collect();
            let format = match key {
                "createdAt" | "updatedAt" => "number",
                k => self.formats.get(k).map_or("shorttext", String::as_str),
            };
            let value = if format == "checkbox" {
                Some(json!(true))
            } else if matches!(condition, "in" | "notIn" | "allIn" | "exactIn") {
                Some(json!(values))
            } else if format == "number" || format == "date" {
                values.first().map(|v| v.trim().parse::<f64>().map_or(Value::Null, |n| json!(n)))
            } else {
                values.first().map(|v| json!(v))
            };
            let incomplete = match &value {
                None => true,
                Some(Value::String(s)) => s.is_empty(),
                Some(Value::Array(a)) => a.is_empty(),
                _ => false,
            };
            if incomplete && !matches!(condition, "empty" | "notEmpty" | "exists") && format != "checkbox" {
                continue;
            }
            let mut rule = json!({"key": key, "condition": condition});
            if let Some(v) = value {
                rule["value"] = v;
            }
            out.push(rule);
        }
        out
    }

    /// A row's value for column `key`, as Roostr's table draws it.
    fn cell(&self, r: &Value, key: &str, space: &str) -> Cell {
        match key {
            "type" | "typeKey" => {
                let t = str_of(r, "typeKey");
                return if t.is_empty() { Cell::Empty } else { Cell::Objects(vec![Pill { icon: self.type_icons.get(&t).cloned().unwrap_or_default(), name: type_name(&t) }]) };
            }
            "updatedAt" | "createdAt" => return r[key].as_i64().map_or(Cell::Empty, |ms| Cell::Text(date(ms))),
            _ => {}
        }
        let Some(v) = r["fields"].get(key) else { return Cell::Empty };
        let rel = self.rel(space, key);
        match rel.map_or("", |r| r.format.as_str()) {
            "checkbox" => Cell::Check(v["boolValue"].as_bool().unwrap_or(false)),
            "date" => v["intValue"].as_i64().map_or(Cell::Empty, |ms| Cell::Text(date(ms))),
            "tag" | "status" => {
                let colors = &rel.expect("a tag relation").colors;
                let tags: Vec<(String, String)> = shown(v).split(", ").filter(|t| !t.is_empty()).map(|t| (t.to_string(), colors.get(t).cloned().unwrap_or_default())).collect();
                if tags.is_empty() { Cell::Empty } else { Cell::Tags(tags) }
            }
            "object" => {
                let pills = self.link_ids(v).iter().map(|id| self.pill(id)).collect::<Vec<_>>();
                if pills.is_empty() { Cell::Empty } else { Cell::Objects(pills) }
            }
            _ => {
                let t = self.read(key, v);
                if t.is_empty() { Cell::Empty } else { Cell::Text(t) }
            }
        }
    }

    fn link_ids(&self, v: &Value) -> Vec<String> {
        match v["valuesValue"]["items"].as_array() {
            Some(items) => items.iter().filter_map(|i| i["linkValue"]["targetId"].as_str().or(i["stringValue"].as_str())).map(str::to_string).collect(),
            None => v["linkValue"]["targetId"].as_str().or(v["stringValue"].as_str()).into_iter().map(str::to_string).collect(),
        }
    }

    /// An object's icon: its own emoji, else its type's.
    fn icon_of(&self, o: &Value) -> String {
        let own = field_str(o, "iconEmoji");
        if !own.is_empty() && own.chars().count() <= 8 {
            return own;
        }
        self.type_icons.get(&str_of(o, "typeKey")).cloned().unwrap_or_default()
    }

    /// A property's icon (roostr-core `relation_emoji`): its own, else for an object property
    /// limited to a type, that type's.
    /// Relation `key` as space `space` has it, else any space's copy.
    fn rel(&self, space: &str, key: &str) -> Option<&Rel> {
        self.rels.get(&(space.to_string(), key.to_string())).or_else(|| self.rels.iter().find(|((_, k), _)| k == key).map(|(_, r)| r))
    }

    fn rel_icon(&self, r: &Rel) -> String {
        if !r.icon.is_empty() {
            return r.icon.clone();
        }
        if r.format != "object" {
            return String::new();
        }
        r.object_types.iter().find_map(|id| self.type_icons_by_id.get(id)).cloned().unwrap_or_default()
    }

    fn pill(&self, id: &str) -> Pill {
        let (name, icon) = self.titled(id);
        Pill { icon, name }
    }
}

impl Daemon {
    /// A value as its property's format reads it: dates as dates, links as their objects' names.
    fn read(&self, key: &str, v: &Value) -> String {
        match self.formats.get(key).map(String::as_str) {
            Some("date") => v["intValue"].as_i64().map(date).unwrap_or_else(|| shown(v)),
            Some("object") | None if is_link(v) => self.link_names(v),
            _ => {
                let text = shown(v);
                // Object properties sometimes hold bare ids: show whose they are.
                let ids = text.split(", ").all(|t| t.len() == 36 && t.matches('-').count() == 4);
                if !text.is_empty() && ids { text.split(", ").map(|id| self.title(id)).collect::<Vec<_>>().join(", ") } else { text }
            }
        }
    }

    fn link_names(&self, v: &Value) -> String {
        let ids: Vec<&str> = match v["valuesValue"]["items"].as_array() {
            Some(items) => items.iter().filter_map(|i| i["linkValue"]["targetId"].as_str().or(i["stringValue"].as_str())).collect(),
            None => v["linkValue"]["targetId"].as_str().or(v["stringValue"].as_str()).into_iter().collect(),
        };
        ids.iter().map(|id| self.title(id)).collect::<Vec<_>>().join(", ")
    }

    fn title(&self, id: &str) -> String {
        self.titled(id).0
    }

    /// An object's name and icon, fetched once.
    fn titled(&self, id: &str) -> (String, String) {
        if let Some(t) = self.titles.borrow().get(id) {
            return t.clone();
        }
        let t = self.get(&format!("/api/objects/{id}")).map(|o| (name_of(&o), self.icon_of(&o))).unwrap_or_else(|| (id.chars().take(8).collect(), String::new()));
        self.titles.borrow_mut().insert(id.to_string(), t.clone());
        t
    }
}

fn is_link(v: &Value) -> bool {
    v.get("linkValue").is_some() || v["valuesValue"]["items"].as_array().is_some_and(|i| i.iter().any(|x| x.get("linkValue").is_some()))
}

/// Epoch ms as "Nov 9, 2026".
fn date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

/// "game_publisher" -> "Game publisher".
fn type_name(key: &str) -> String {
    let mut c = key.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect::<String>().replace('_', " ")).unwrap_or_default()
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn field_str(o: &Value, k: &str) -> String {
    o["fields"][k]["stringValue"].as_str().unwrap_or_default().to_string()
}

fn name_of(o: &Value) -> String {
    let n = field_str(o, "name");
    if n.is_empty() { "Untitled".into() } else { n }
}

/// A property value in words: text, numbers, checked, list items joined.
fn shown(v: &Value) -> String {
    if let Some(s) = v["stringValue"].as_str() {
        return s.to_string();
    }
    if let Some(b) = v["boolValue"].as_bool() {
        return if b { "✓".into() } else { String::new() };
    }
    if let Some(n) = v["intValue"].as_i64() {
        return n.to_string();
    }
    if let Some(f) = v["floatValue"].as_f64() {
        return format!("{f}");
    }
    if let Some(items) = v["valuesValue"]["items"].as_array() {
        return items.iter().map(shown).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    }
    if let Some(t) = v["linkValue"]["targetId"].as_str() {
        return t.chars().take(8).collect();
    }
    String::new()
}

fn blocks(o: &Value) -> Vec<Block> {
    let mut out = Vec::new();
    for b in o["blocks"].as_array().into_iter().flatten() {
        let c = &b["content"];
        if c["custom"]["contentType"] == "divider" {
            out.push(Block { kind: Kind::Divider, text: String::new() });
            continue;
        }
        let Some(t) = c.get("text") else { continue };
        let kind = match t["style"].as_u64().unwrap_or(0) {
            1 => Kind::H1,
            2 => Kind::H2,
            3 => Kind::H3,
            4 | 11 => Kind::Quote,
            5 => Kind::Code,
            6 => Kind::Bullet,
            7 => Kind::Numbered,
            8 => Kind::Check(t["checked"].as_bool().unwrap_or(false)),
            9 | 12 => continue,
            _ => Kind::Paragraph,
        };
        out.push(Block { kind, text: t["text"].as_str().unwrap_or_default().to_string() });
    }
    out
}

/// What shows without a daemon.
pub fn sample() -> (Vec<Space>, Vec<Object>) {
    let b = |kind, text: &str| Block { kind, text: text.into() };
    let page = Object {
        id: "p1".into(),
        name: "Launch plan".into(),
        icon: "🚀".into(),
        type_name: "page".into(),
        props: vec![("Status".into(), "In progress".into()), ("Owner".into(), "Grant".into())],
        sources: vec![],
        body: vec![
            b(Kind::H1, "Goals"),
            b(Kind::Paragraph, "Ship the Roostr MVP to ten testers and learn what they open first."),
            b(Kind::Check(true), "Pick the UI framework"),
            b(Kind::Check(false), "Build the page editor"),
            b(Kind::Check(false), "Queries as tables"),
            b(Kind::H2, "Notes"),
            b(Kind::Bullet, "Pages are blocks: headings, lists, checkboxes, quotes"),
            b(Kind::Bullet, "Every object can have agents"),
            b(Kind::Quote, "Make it feel like a notebook, not a database."),
        ],
        set: None,
    };
    let tasks = Object {
        id: "q1".into(),
        name: "All tasks".into(),
        icon: "✅".into(),
        type_name: "query".into(),
        props: vec![],
        sources: vec!["✅ Task".into()],
        body: vec![],
        set: Some(Set {
            name_width: 240.0,
            columns: vec![
                Column { key: "status".into(), name: "Status".into(), icon: "🚦".into(), width: 140.0 },
                Column { key: "owner".into(), name: "Owner".into(), icon: "👤".into(), width: 170.0 },
                Column { key: "done".into(), name: "Done".into(), icon: "✅".into(), width: 90.0 },
                Column { key: "due".into(), name: "Due date".into(), icon: "⏰".into(), width: 140.0 },
            ],
            rows: [("Pick the UI framework", "Done", "lime", true, "Nov 8, 2026"), ("Build the page editor", "Doing", "blue", false, "Nov 12, 2026"), ("Queries as tables", "Todo", "grey", false, "Nov 14, 2026")]
                .iter()
                .enumerate()
                .map(|(i, (n, st, color, done, due))| Row {
                    id: format!("t{i}"),
                    icon: "✅".into(),
                    name: n.to_string(),
                    cells: vec![
                        Cell::Tags(vec![(st.to_string(), color.to_string())]),
                        Cell::Objects(vec![Pill { icon: "👤".into(), name: "Grant".into() }]),
                        Cell::Check(*done),
                        Cell::Text(due.to_string()),
                    ],
                })
                .collect(),
        }),
    };
    let space = Space {
        id: "s1".into(),
        name: "Personal".into(),
        icon: "🏠".into(),
        pins: vec![Pin { id: "p1".into(), name: page.name.clone(), icon: page.icon.clone() }, Pin { id: "q1".into(), name: tasks.name.clone(), icon: tasks.icon.clone() }],
    };
    (vec![space], vec![page, tasks])
}

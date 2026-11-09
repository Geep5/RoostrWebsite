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
    /// A query or collection: its columns (key, name) and rows.
    pub set: Option<Set>,
}

pub struct Set {
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Row>,
}

pub struct Row {
    pub id: String,
    pub cells: Vec<String>,
}

/// The local daemon, when one answers.
pub struct Daemon {
    base: String,
    token: String,
    /// relation key -> name, for property labels and column headers.
    names: std::collections::HashMap<String, String>,
    /// relation key -> format ("date", "object", …), for how a value reads.
    formats: std::collections::HashMap<String, String>,
    /// object id -> name, for links.
    titles: std::cell::RefCell<std::collections::HashMap<String, String>>,
}

impl Daemon {
    pub fn connect() -> Option<Daemon> {
        let home = std::env::var("HOME").ok()?;
        let token = std::fs::read_to_string(format!("{home}/.glon/api-token")).ok()?.trim().to_string();
        let mut d = Daemon { base: "http://127.0.0.1:7333".into(), token, names: Default::default(), formats: Default::default(), titles: Default::default() };
        let rels = d.get("/api/relations")?;
        for r in rels.as_array()? {
            if let (Some(k), Some(n)) = (r["key"].as_str(), r["name"].as_str()) {
                d.names.entry(k.to_string()).or_insert_with(|| n.to_string());
                d.formats.entry(k.to_string()).or_insert_with(|| r["format"].as_str().unwrap_or_default().to_string());
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
        let skip = ["name", "iconEmoji", "channel", "agent", "viewRelations", "viewFilters", "viewSorts", "setOf", "cover", "cover_y", "pin_order", "pin_collapsed", "links", "last_run", "repeat", "repeat_instructions", "assignee", "error", "run_active", "run_now"];
        let props = o["fields"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !skip.contains(&k.as_str()) && !k.starts_with("key_") && !k.starts_with("view") && k.as_str() != "collectionIds")
            .map(|(k, v)| (self.names.get(k).cloned().unwrap_or_else(|| k.clone()), self.read(k, v)))
            .filter(|(_, v)| !v.is_empty())
            .take(8)
            .collect();
        Some(Object { id: id.to_string(), name: name_of(&o), icon: field_str(&o, "iconEmoji"), type_name: type_key, props, body: blocks(&o), set })
    }

    /// A query's rows: the objects of its source types in its space (its filters are left out).
    fn set(&self, o: &Value) -> Option<Set> {
        let types: Vec<String> = o["fields"]["setOf"]["valuesValue"]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["stringValue"].as_str().map(str::to_string))
            .collect();
        let mut columns = vec![("name".to_string(), "Name".to_string())];
        for c in o["fields"]["viewRelations"]["valuesValue"]["items"].as_array().into_iter().flatten() {
            if let Some(k) = c["mapValue"]["entries"]["key"]["stringValue"].as_str().filter(|k| *k != "name") {
                let name = match k {
                    "type" | "typeKey" => "Type".to_string(),
                    "updatedAt" => "Modified".to_string(),
                    "createdAt" => "Created".to_string(),
                    k => self.names.get(k).cloned().unwrap_or_else(|| k.to_string()),
                };
                columns.push((k.to_string(), name));
            }
        }
        let mut filters = vec![json!({"key": "channel", "condition": "equal", "value": field_str(o, "channel")})];
        // A collection holds its members; a query, the objects of its source types.
        if str_of(o, "typeKey") == "collection" {
            let members: Vec<String> = o["fields"]["collectionIds"]["valuesValue"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i["stringValue"].as_str().or(i["linkValue"]["targetId"].as_str()).map(str::to_string))
                .collect();
            filters = vec![json!({"key": "id", "condition": "in", "value": members})];
        } else if !types.is_empty() {
            filters.push(json!({"key": "typeKey", "condition": "in", "value": types}));
        }
        let res = self.post("/api/query", json!({"filters": filters, "limit": 500}))?;
        let rows = res["records"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| Row {
                id: str_of(r, "id"),
                cells: columns
                    .iter()
                    .map(|(k, _)| match k.as_str() {
                        "name" => name_of(r),
                        "type" | "typeKey" => str_of(r, "typeKey").replace('_', " "),
                        "updatedAt" | "createdAt" => r[k.as_str()].as_i64().map(date).unwrap_or_default(),
                        k => r["fields"].get(k).map(|v| self.read(k, v)).unwrap_or_default(),
                    })
                    .collect(),
            })
            .collect();
        Some(Set { columns, rows })
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
        if let Some(t) = self.titles.borrow().get(id) {
            return t.clone();
        }
        let t = self.get(&format!("/api/objects/{id}")).map(|o| name_of(&o)).unwrap_or_else(|| id.chars().take(8).collect());
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
        body: vec![],
        set: Some(Set {
            columns: vec![("name".into(), "Name".into()), ("status".into(), "Status".into()), ("due".into(), "Due".into())],
            rows: [("Pick the UI framework", "Done", "Nov 8"), ("Build the page editor", "Doing", "Nov 12"), ("Queries as tables", "Todo", "Nov 14")]
                .iter()
                .enumerate()
                .map(|(i, (n, s, d))| Row { id: format!("t{i}"), cells: vec![n.to_string(), s.to_string(), d.to_string()] })
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

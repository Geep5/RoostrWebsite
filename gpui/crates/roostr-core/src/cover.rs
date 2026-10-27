//! Page covers: the fields every page with the object header (all but chat
//! objects) can carry, and the generated-cover session the space's OpenAI
//! credential's computer answers. Everything here is an ordinary synced
//! field on the page, visible as its properties:
//!
//! - `cover`: a link to the cover's File object; `cover_y`: its vertical
//!   focus, 0 (top edge shown) ..= 1 (bottom), 0.5 when absent.
//! - `cover_request` (written here, any device): `{at, action, prompt}`; the
//!   computer serving the space's OpenAI credential deletes it as it takes it.
//! - `cover_generation` (written by that computer): the session's status,
//!   prompt(s) and painted tries. The app deletes it, and vanishes the tries
//!   nobody used, when the person closes the generate panel.

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global, Task};
use serde_json::json;

use crate::api;
use crate::types::{Fields, Object, QueryRow, Value, field_str};

pub const COVER: &str = "cover";
pub const COVER_Y: &str = "cover_y";
pub const REQUEST: &str = "cover_request";
pub const GENERATION: &str = "cover_generation";

/// Where a cover is focused until someone repositions it.
pub const DEFAULT_Y: f32 = 0.5;

/// Shown (and nothing asked) while the space's OpenAI credential has no key;
/// the serving computer answers with the same words when it finds none.
pub const NEEDS_KEY: &str =
    "Add an OpenAI key to this space's OpenAI credential to generate covers";

/// The generate panel's cost note.
pub const COST_NOTE: &str = "High quality · about $0.20 per image on this space's OpenAI key";

/// Every page with the object header has a cover slot; a chat object's page
/// is its discussion.
pub fn can_have_cover(object: &Object) -> bool {
    object.type_key != "chat"
}

/// The cover's File object id: a link, a plain id, or a one-item list of either.
pub fn cover_file(fields: &Fields) -> Option<String> {
    fields.get(COVER)?.link_ids().into_iter().next()
}

/// The cover's vertical focus, 0..=1.
pub fn cover_y(fields: &Fields) -> f32 {
    fields
        .get(COVER_Y)
        .and_then(Value::as_number)
        .map_or(DEFAULT_Y, |y| (y as f32).clamp(0., 1.))
}

/// The space a page belongs to (a space's own page: that space).
pub fn space_of(object: &Object) -> String {
    if object.type_key == "channel" {
        object.id.clone()
    } else {
        object.channel().to_string()
    }
}

/// `cover_generation.status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The art director is writing the prompt.
    Writing,
    /// The image is being painted.
    Painting,
    Done,
    Failed,
}

/// One painted image of the session.
#[derive(Clone, PartialEq, Debug)]
pub struct Try {
    pub file: String,
    pub prompt: String,
    pub at: i64,
}

/// `cover_generation`, as the serving computer last wrote it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Generation {
    /// The `cover_request.at` it answers.
    pub at: i64,
    pub status: Option<Status>,
    /// The OpenAI credential used (or needed).
    pub credential: String,
    pub prompt: String,
    pub tries: Vec<Try>,
    pub error: String,
    pub needs_key: bool,
}

impl Generation {
    /// Writing or painting.
    pub fn running(&self) -> bool {
        matches!(self.status, Some(Status::Writing | Status::Painting))
    }
}

fn entry_str(entries: &Fields, key: &str) -> String {
    field_str(entries, key).to_string()
}

fn entry_int(entries: &Fields, key: &str) -> i64 {
    entries.get(key).and_then(Value::as_int).unwrap_or_default()
}

/// The page's generate session, if one is open.
pub fn generation(fields: &Fields) -> Option<Generation> {
    let entries = fields.get(GENERATION)?.entries()?;
    let status = match field_str(entries, "status") {
        "writing" => Some(Status::Writing),
        "painting" => Some(Status::Painting),
        "done" => Some(Status::Done),
        "failed" => Some(Status::Failed),
        _ => None,
    };
    let tries = entries
        .get("tries")
        .map(Value::items)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::entries)
        .map(|t| Try {
            file: entry_str(t, "file"),
            prompt: entry_str(t, "prompt"),
            at: entry_int(t, "at"),
        })
        .filter(|t| !t.file.is_empty())
        .collect();
    Some(Generation {
        at: entry_int(entries, "at"),
        status,
        credential: entry_str(entries, "credential"),
        prompt: entry_str(entries, "prompt"),
        tries,
        error: entry_str(entries, "error"),
        needs_key: entries.get("needs_key").and_then(|v| v.bool_value) == Some(true),
    })
}

/// The `at` of the page's pending `cover_request`, if any.
pub fn pending_request(fields: &Fields) -> Option<i64> {
    let entries = fields.get(REQUEST)?.entries()?;
    Some(entry_int(entries, "at"))
}

/// `cover_request.action`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Write the idea, then paint it - or paint the person's own prompt.
    Generate,
    /// Paint the same prompt again.
    Reroll,
    /// Write a different idea, then paint it.
    NewIdea,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Reroll => "reroll",
            Self::NewIdea => "new_idea",
        }
    }
}

/// Ask the space's OpenAI computer for a cover (`cover_request`). `prompt`:
/// Generate - the person's own prompt, "" to have the idea written; Reroll -
/// the prompt to paint again; New idea - ignored.
pub fn request(object_id: &str, action: Action, prompt: &str, cx: &mut App) -> Task<Result<()>> {
    let mut entries = Fields::from([
        (
            "at".to_string(),
            Value::int(chrono::Utc::now().timestamp_millis()),
        ),
        ("action".to_string(), Value::string(action.as_str())),
    ]);
    if action != Action::NewIdea {
        entries.insert("prompt".into(), Value::string(prompt));
    }
    let set = api::note::set_field(object_id, REQUEST, Value::map(entries), cx);
    cx.background_spawn(async move { set.await.map(|_| ()) })
}

/// Make File `file_id` the page's cover, focused in the middle.
pub fn set_cover(object_id: &str, file_id: &str, cx: &mut App) -> Task<Result<()>> {
    let cover = api::note::set_field(object_id, COVER, Value::link(file_id, COVER), cx);
    let y = api::note::set_field(object_id, COVER_Y, Value::float(DEFAULT_Y as f64), cx);
    cx.background_spawn(async move {
        cover.await?;
        y.await?;
        Ok(())
    })
}

/// Save the cover's vertical focus.
pub fn set_y(object_id: &str, y: f32, cx: &mut App) -> Task<Result<()>> {
    let set = api::note::set_field(object_id, COVER_Y, Value::float(y.clamp(0., 1.) as f64), cx);
    cx.background_spawn(async move { set.await.map(|_| ()) })
}

/// Take the cover off the page (its File stays, like any file).
pub fn remove(object_id: &str, cx: &mut App) -> Task<Result<()>> {
    let cover = api::note::delete_field(object_id, COVER, cx);
    let y = api::note::delete_field(object_id, COVER_Y, cx);
    cx.background_spawn(async move {
        cover.await?;
        y.await?;
        Ok(())
    })
}

/// Close the page's generate session: vanish every painted try but `kept`
/// and the page's cover, then delete `cover_generation` and any pending
/// `cover_request`.
pub fn end_session(object: &Object, kept: Option<&str>, cx: &mut App) -> Task<Result<()>> {
    let cover = cover_file(&object.fields);
    let unused: Vec<String> = generation(&object.fields)
        .map(|g| g.tries)
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.file)
        .filter(|f| Some(f.as_str()) != kept && Some(f) != cover.as_ref())
        .collect();
    let vanish = (!unused.is_empty()).then(|| api::note::vanish(&unused, cx));
    let generation = object
        .fields
        .contains_key(GENERATION)
        .then(|| api::note::delete_field(&object.id, GENERATION, cx));
    let request = object
        .fields
        .contains_key(REQUEST)
        .then(|| api::note::delete_field(&object.id, REQUEST, cx));
    cx.background_spawn(async move {
        for task in [vanish, generation, request].into_iter().flatten() {
            task.await?;
        }
        Ok(())
    })
}

/// The space's OpenAI credential (the harness seeds one, blank, per space).
#[derive(Clone, PartialEq, Debug)]
pub struct OpenAiCredential {
    pub id: String,
    /// Whether its `key_api_key` holds anything.
    pub has_key: bool,
}

/// Of a space's credentials, the OpenAI one the serving computer uses: an
/// active one first, else the oldest.
pub fn pick_openai_credential(mut rows: Vec<QueryRow>) -> Option<OpenAiCredential> {
    rows.retain(|r| field_str(&r.fields, "service") == "openai");
    rows.sort_by(|a, b| {
        let active = |r: &QueryRow| field_str(&r.fields, "status") == "active";
        active(b)
            .cmp(&active(a))
            .then(a.created_at.cmp(&b.created_at))
    });
    rows.into_iter().next().map(|r| OpenAiCredential {
        has_key: !field_str(&r.fields, "key_api_key").trim().is_empty(),
        id: r.id,
    })
}

/// Look up `space_id`'s OpenAI credential.
pub fn openai_credential(space_id: &str, cx: &App) -> Task<Result<Option<OpenAiCredential>>> {
    let rows = api::query_all(
        json!({
            "type": "credential",
            "filters": [{ "key": "channel", "condition": "equal", "value": space_id }],
        }),
        cx,
    );
    cx.background_spawn(async move { Ok(pick_openai_credential(rows.await?)) })
}

/// What the phone's ⋯ menu asks of the open page's cover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverAction {
    /// Add or change: choose, paste or generate an image.
    Choose,
    Reposition,
    Remove,
    Generate,
}

/// A [`CoverAction`] for the page showing `object_id`.
#[derive(Clone, PartialEq, Debug)]
pub struct CoverAsk {
    pub object_id: String,
    pub action: CoverAction,
}

/// Carries [`CoverAsk`]s from the shell's menus to the object page.
pub struct CoverAsks;

impl EventEmitter<CoverAsk> for CoverAsks {}

struct GlobalCoverAsks(Entity<CoverAsks>);

impl Global for GlobalCoverAsks {}

impl CoverAsks {
    pub fn init(cx: &mut App) {
        let asks = cx.new(|_| Self);
        cx.set_global(GlobalCoverAsks(asks));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCoverAsks>().0.clone()
    }
}

/// Ask the page showing `object_id` to do `action` with its cover.
pub fn ask(object_id: &str, action: CoverAction, cx: &mut App) {
    let object_id = object_id.to_string();
    CoverAsks::global(cx).update(cx, |_, cx| cx.emit(CoverAsk { object_id, action }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(json: serde_json::Value) -> Fields {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn cover_fields_read_every_accepted_form() {
        let link = fields(
            json!({ "cover": { "linkValue": { "targetId": "f1", "relationKey": "cover" } } }),
        );
        assert_eq!(cover_file(&link).as_deref(), Some("f1"));
        let plain = fields(json!({ "cover": { "stringValue": "f2" } }));
        assert_eq!(cover_file(&plain).as_deref(), Some("f2"));
        let list =
            fields(json!({ "cover": { "valuesValue": { "items": [{ "stringValue": "f3" }] } } }));
        assert_eq!(cover_file(&list).as_deref(), Some("f3"));
        assert_eq!(cover_file(&Fields::new()), None);
        assert_eq!(cover_y(&Fields::new()), 0.5);
        assert_eq!(
            cover_y(&fields(json!({ "cover_y": { "intValue": 1 } }))),
            1.
        );
        assert_eq!(
            cover_y(&fields(json!({ "cover_y": { "floatValue": 0.25 } }))),
            0.25
        );
        assert_eq!(
            cover_y(&fields(json!({ "cover_y": { "floatValue": 3.0 } }))),
            1.
        );
    }

    #[test]
    fn a_generation_reads_its_status_prompt_and_tries() {
        let f = fields(json!({ "cover_generation": { "mapValue": { "entries": {
            "at": { "intValue": 7 },
            "status": { "stringValue": "failed" },
            "credential": { "stringValue": "openai-sp" },
            "prompt": { "stringValue": "A fox" },
            "tries": { "valuesValue": { "items": [
                { "mapValue": { "entries": { "file": { "stringValue": "f1" }, "prompt": { "stringValue": "A fox" }, "at": { "intValue": 5 } } } },
            ] } },
            "error": { "stringValue": NEEDS_KEY },
            "needs_key": { "boolValue": true },
        } } } }));
        let g = generation(&f).unwrap();
        assert_eq!(g.at, 7);
        assert_eq!(g.status, Some(Status::Failed));
        assert!(!g.running());
        assert_eq!(g.credential, "openai-sp");
        assert_eq!(
            g.tries,
            vec![Try {
                file: "f1".into(),
                prompt: "A fox".into(),
                at: 5
            }]
        );
        assert!(g.needs_key);
        assert_eq!(g.error, NEEDS_KEY);
    }

    #[test]
    fn the_openai_credential_prefers_active_then_oldest() {
        let row = |id: &str, service: &str, status: &str, key: &str, created_at: i64| QueryRow {
            id: id.into(),
            type_key: "credential".into(),
            created_at,
            fields: fields(json!({
                "service": { "stringValue": service },
                "status": { "stringValue": status },
                "key_api_key": { "stringValue": key },
            })),
            ..QueryRow::default()
        };
        assert_eq!(
            pick_openai_credential(vec![row("a", "anthropic", "active", "k", 1)]),
            None
        );
        let picked = pick_openai_credential(vec![
            row("old", "openai", "", "", 1),
            row("new", "openai", "", "sk", 2),
        ]);
        assert_eq!(
            picked,
            Some(OpenAiCredential {
                id: "old".into(),
                has_key: false
            })
        );
        let picked = pick_openai_credential(vec![
            row("old", "openai", "", "", 1),
            row("live", "openai", "active", "sk", 2),
        ]);
        assert_eq!(
            picked,
            Some(OpenAiCredential {
                id: "live".into(),
                has_key: true
            })
        );
    }
}

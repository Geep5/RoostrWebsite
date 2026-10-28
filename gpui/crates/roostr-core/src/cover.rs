//! Page covers: the fields every page with the object header (all but chat
//! objects) can carry, and the generated-cover session the space's OpenAI
//! credential's computer answers. Everything here is an ordinary synced
//! field on the page, visible as its properties:
//!
//! - `cover`: a link to the cover's File object; `cover_y`: its vertical
//!   focus, 0 (top edge shown) ..= 1 (bottom), 0.5 when absent.
//! - `cover_request` (written here, any device): `{at, prompt}` - the
//!   person's own words, painted as written; an empty prompt ("Choose for
//!   me") has the art director write an idea from the page first. The
//!   computer serving the space's OpenAI credential deletes it as it takes
//!   it.
//! - `cover_generation` (written by that computer): the run in progress
//!   (`writing`, `painting`), shown as a spinner. A finished run makes its
//!   image the page's `cover` and deletes this field; a failed one stays
//!   (`failed`, with OpenAI's message) until the next Generate or the person
//!   dismisses it.

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
    /// "Choose for me": the art director is writing the idea.
    Writing,
    /// The image is being painted.
    Painting,
    Failed,
}

/// `cover_generation`, as the serving computer last wrote it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Generation {
    /// The `cover_request.at` it answers.
    pub at: i64,
    pub status: Option<Status>,
    /// The OpenAI credential used (or needed).
    pub credential: String,
    pub error: String,
    pub needs_key: bool,
}

impl Generation {
    /// Writing or painting.
    pub fn running(&self) -> bool {
        matches!(self.status, Some(Status::Writing | Status::Painting))
    }
}

/// Whether a cover is being made for the page: asked and not yet answered,
/// or its run is writing or painting.
pub fn generating(fields: &Fields) -> bool {
    let run = generation(fields);
    pending_request(fields).is_some_and(|at| run.as_ref().is_none_or(|g| g.at != at))
        || run.is_some_and(|g| g.running())
}

fn entry_str(entries: &Fields, key: &str) -> String {
    field_str(entries, key).to_string()
}

/// The page's generate run, if one is on it.
pub fn generation(fields: &Fields) -> Option<Generation> {
    let entries = fields.get(GENERATION)?.entries()?;
    let status = match field_str(entries, "status") {
        "writing" => Some(Status::Writing),
        "painting" => Some(Status::Painting),
        "failed" => Some(Status::Failed),
        _ => None,
    };
    Some(Generation {
        at: entries.get("at").and_then(Value::as_int).unwrap_or_default(),
        status,
        credential: entry_str(entries, "credential"),
        error: entry_str(entries, "error"),
        needs_key: entries.get("needs_key").and_then(|v| v.bool_value) == Some(true),
    })
}

/// The `at` of the page's pending `cover_request`, if any.
pub fn pending_request(fields: &Fields) -> Option<i64> {
    let entries = fields.get(REQUEST)?.entries()?;
    Some(entries.get("at").and_then(Value::as_int).unwrap_or_default())
}

/// Ask the space's OpenAI computer for the page's new cover
/// (`cover_request`): `prompt` painted as written, or "" to have the idea
/// chosen from the page. The failure of an earlier try goes with it.
pub fn request(object: &Object, prompt: &str, cx: &mut App) -> Task<Result<()>> {
    let entries = Fields::from([
        (
            "at".to_string(),
            Value::int(chrono::Utc::now().timestamp_millis()),
        ),
        ("prompt".to_string(), Value::string(prompt.trim())),
    ]);
    let set = api::note::set_field(&object.id, REQUEST, Value::map(entries), cx);
    let failed = object
        .fields
        .contains_key(GENERATION)
        .then(|| api::note::delete_field(&object.id, GENERATION, cx));
    cx.background_spawn(async move {
        if let Some(failed) = failed {
            failed.await?;
        }
        set.await.map(|_| ())
    })
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

/// Dismiss the page's failed run (or stop waiting for one): its fields go.
pub fn dismiss(object: &Object, cx: &mut App) -> Task<Result<()>> {
    let generation = object
        .fields
        .contains_key(GENERATION)
        .then(|| api::note::delete_field(&object.id, GENERATION, cx));
    let request = object
        .fields
        .contains_key(REQUEST)
        .then(|| api::note::delete_field(&object.id, REQUEST, cx));
    cx.background_spawn(async move {
        for task in [generation, request].into_iter().flatten() {
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
    fn a_generation_reads_its_status_and_error_and_says_when_a_cover_is_coming() {
        let failed = fields(json!({ "cover_generation": { "mapValue": { "entries": {
            "at": { "intValue": 7 },
            "status": { "stringValue": "failed" },
            "credential": { "stringValue": "openai-sp" },
            "error": { "stringValue": NEEDS_KEY },
            "needs_key": { "boolValue": true },
        } } } }));
        let g = generation(&failed).unwrap();
        assert_eq!(g.at, 7);
        assert_eq!(g.status, Some(Status::Failed));
        assert!(!g.running());
        assert_eq!(g.credential, "openai-sp");
        assert!(g.needs_key);
        assert_eq!(g.error, NEEDS_KEY);
        assert!(!generating(&failed));
        // Asked, unanswered: coming. Answered by a run that paints: coming.
        let asked = fields(json!({ "cover_request": { "mapValue": { "entries": { "at": { "intValue": 8 } } } } }));
        assert!(generating(&asked));
        let painting = fields(json!({ "cover_generation": { "mapValue": { "entries": {
            "at": { "intValue": 8 }, "status": { "stringValue": "painting" },
        } } } }));
        assert!(generating(&painting));
        assert!(!generating(&Fields::new()));
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

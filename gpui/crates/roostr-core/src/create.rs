//! Object creation (`src/lib/create.ts`), space-aware, shared by the home
//! page, the sidebar New menu, search's "Create object" and collections.
//! Flows that open the new object navigate the active tab to it.

use std::collections::HashSet;

use anyhow::{Result, anyhow};
#[cfg(not(target_family = "wasm"))]
use gpui_kit::PathPromptOptions;
use gpui_kit::{App, AsyncApp, Task};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;

use crate::api;
use crate::harness::{self, Harness};
use crate::io;
use crate::nav::{Route, navigate};
use crate::objects::type_glyph;
use crate::relations::current_space_id;
use crate::store::Store;
use crate::types::{Block, Fields, Object, Value, field_str, guest_agents, is_builtin_tool};

/// Legacy fallback until the type objects have loaded.
pub const CREATABLE_TYPES: &[&str] = &["page", "note", "task", "person", "project", "bookmark"];

/// Types a space can't default to: each needs a choice first.
pub const NOT_DEFAULTABLE: &[&str] = &["file", "machine", "agent", "credential"];

/// The built-in Tools (by name) a new agent gets when its template sets none.
pub const DEFAULT_TOOLS: &[&str] = &["shell_exec", "web_fetch"];
/// The Model a new agent starts with when its template sets none.
pub const DEFAULT_AGENT_MODEL: &str = "claude-sonnet-4-5";

/// A template's own identity and bookkeeping, never a default for what it creates.
const TEMPLATE_OWN: &[&str] = &[
    "name",
    "target_type",
    "channel",
    "error",
    "createdDate",
    "modifiedDate",
    "type_key",
    "repeat",
    "seed_key",
    "seed_hash",
];

const NOT_RUNNING_TO_ADD: &str = "Roostr isn't running on this computer - start it to add files.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatableType {
    pub key: String,
    pub name: String,
    pub icon: String,
}

fn channel_field(channel_id: &str) -> Fields {
    let mut f = Fields::new();
    if !channel_id.is_empty() {
        f.insert("channel".into(), Value::string(channel_id));
    }
    f
}

fn capitalize(k: &str) -> String {
    let mut c = k.chars();
    c.next()
        .map(|h| h.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The New menu's types: the active space's own types (spaces are
/// self-contained), else the legacy list.
pub fn creatable_types(cx: &App) -> Vec<CreatableType> {
    let sid = current_space_id(cx);
    let store = Store::global(cx).read(cx);
    let legacy = || {
        CREATABLE_TYPES
            .iter()
            .map(|k| CreatableType {
                key: k.to_string(),
                name: capitalize(k),
                icon: type_glyph(store, k, &sid),
            })
            .collect()
    };
    let own: Vec<_> = store.types.iter().filter(|t| t.space == sid).collect();
    if own.is_empty() {
        return legacy();
    }
    own.into_iter()
        .map(|t| CreatableType {
            key: t.key.clone(),
            name: if t.name.is_empty() {
                t.key.clone()
            } else {
                t.name.clone()
            },
            icon: if t.icon.is_empty() {
                type_glyph(store, &t.key, &sid)
            } else {
                t.icon.clone()
            },
        })
        .collect()
}

/// The space's Default type (`default_type` on the space object), "note"
/// when unset, not defaultable, or gone from the space.
pub fn default_type_key(space_id: &str, cx: &App) -> Task<String> {
    if space_id.is_empty() {
        return Task::ready("note".into());
    }
    let fetch = Store::global(cx).read(cx).fetch_object(space_id, cx);
    let space_id = space_id.to_string();
    cx.spawn(async move |cx| {
        let key = fetch
            .await
            .map(|s| s.field_str("default_type").to_string())
            .unwrap_or_default();
        let known = cx.update(|cx| {
            Store::global(cx)
                .read(cx)
                .types
                .iter()
                .any(|t| t.space == space_id && t.key == key)
        });
        if !key.is_empty() && !NOT_DEFAULTABLE.contains(&key.as_str()) && known {
            key
        } else {
            "note".into()
        }
    })
}

/// The type's default template in a space (each space has its own copy of a bundled type).
pub fn default_template_of(store: &Store, key: &str, channel_id: &str) -> String {
    let mut first = None;
    for t in store.types.iter().filter(|t| t.key == key) {
        if t.space == channel_id {
            return t.default_template_id.clone();
        }
        first.get_or_insert(t);
    }
    first
        .map(|t| t.default_template_id.clone())
        .unwrap_or_default()
}

/// The `agent` guest-list value: links to agent ids.
pub fn agent_links_value(ids: &[String]) -> Value {
    Value::values(ids.iter().map(|id| Value::link(id, "agent")).collect())
}

/// Whether a field holds something: blank text and empty lists are unset.
fn has_value(v: Option<&Value>) -> bool {
    let Some(v) = v else { return false };
    if let Some(s) = &v.string_value {
        return !s.is_empty();
    }
    if let Some(vals) = &v.values_value {
        return !vals.items.is_empty();
    }
    if let Some(list) = &v.list_value {
        return !list.values.is_empty();
    }
    true
}

fn fetch(id: &str, cx: &AsyncApp) -> Task<Result<Object>> {
    cx.update(|cx| Store::global(cx).read(cx).fetch_object(id, cx))
}

/// Copy a template into an object: its content blocks (ids remapped, the
/// discussion subtree left behind) and its properties as DEFAULTS - a field
/// the object already holds wins. The guest list is normalized to links.
pub fn apply_template(object_id: &str, template_id: &str, cx: &mut App) -> Task<Result<()>> {
    let object_id = object_id.to_string();
    let template_id = template_id.to_string();
    cx.spawn(async move |cx| {
        let (tpl, target) = (fetch(&template_id, cx), fetch(&object_id, cx));
        let (tpl, target) = (tpl.await?, target.await?);
        // Everything reachable from __discussion__ is conversation, not content.
        let mut skip = HashSet::new();
        let mut stack = vec!["__discussion__".to_string()];
        while let Some(id) = stack.pop() {
            if skip.insert(id.clone())
                && let Some(b) = tpl.block(&id)
            {
                stack.extend(b.children_ids.iter().cloned());
            }
        }
        let id_map: std::collections::HashMap<&str, String> = tpl
            .blocks
            .iter()
            .filter(|b| !skip.contains(&b.id))
            .map(|b| (b.id.as_str(), uuid::Uuid::new_v4().to_string()))
            .collect();
        for b in tpl.blocks.iter().filter(|b| !skip.contains(&b.id)) {
            let block = Block {
                id: id_map[b.id.as_str()].clone(),
                children_ids: b
                    .children_ids
                    .iter()
                    .filter_map(|c| id_map.get(c.as_str()).cloned())
                    .collect(),
                content: b.content.clone(),
                ..Block::default()
            };
            cx.update(|cx| api::note::block_add(&object_id, &block, "", 0, cx))
                .await?;
        }
        for (key, value) in &tpl.fields {
            if TEMPLATE_OWN.contains(&key.as_str()) || has_value(target.fields.get(key)) {
                continue;
            }
            let value = if key == "agent" {
                agent_links_value(&guest_agents(&tpl.fields))
            } else {
                value.clone()
            };
            cx.update(|cx| api::note::set_field(&object_id, key, value, cx))
                .await?;
        }
        Ok(())
    })
}

/// This computer's machine id from its harness (`GET /machine`), "" when unknown.
pub fn this_machine_id(cx: &App) -> Task<String> {
    let Some(harness) = harness::harness(cx) else {
        return Task::ready(String::new());
    };
    io::spawn(cx, async move {
        harness
            .get_json("/machine")
            .await
            .ok()
            .and_then(|j| j.get("id")?.as_str().map(str::to_string))
            .unwrap_or_default()
    })
}

fn open(id: &str, cx: &AsyncApp) {
    let id = id.to_string();
    cx.update(|cx| navigate(Route::Object(id), cx));
}

fn query_all(body: serde_json::Value, cx: &AsyncApp) -> Task<Result<Vec<crate::types::QueryRow>>> {
    cx.update(|cx| api::query_all(body, cx))
}

/// A new agent in the space, from the agent type's default template there
/// (model, prompt, skills, tools, credentials, computer), else linked to the
/// space's "Assistant" prompt, its built-in shell/web tools and its Anthropic
/// credential. Stays where you are. `seeds` are starting fields (a view's
/// filter values); the template fills only what they leave empty.
pub fn new_agent(
    channel_id: &str,
    name: &str,
    seeds: Fields,
    cx: &mut App,
) -> Task<Result<String>> {
    let channel_id = channel_id.to_string();
    let name = name.trim().to_string();
    cx.spawn(async move |cx| {
        let tpl_id = cx.update(|cx| default_template_of(Store::global(cx).read(cx), "agent", &channel_id));
        let tpl = if tpl_id.is_empty() { None } else { fetch(&tpl_id, cx).await.ok() };
        let has = |k: &str| tpl.as_ref().is_some_and(|t| t.fields.contains_key(k));
        let in_space = |kind: &str| json!({ "type": kind, "filters": [{ "key": "channel", "condition": "equal", "value": channel_id }] });
        let assistant = if has("prompt") {
            None
        } else {
            query_all(in_space("system_prompt"), cx).await?.into_iter().find(|r| field_str(&r.fields, "name") == "Assistant")
        };
        let tools = if has("tools") {
            vec![]
        } else {
            query_all(in_space("tool"), cx)
                .await?
                .into_iter()
                .filter(|r| is_builtin_tool(&r.fields) && DEFAULT_TOOLS.contains(&field_str(&r.fields, "name")))
                .collect()
        };
        let model_cred = if has("credentials") {
            None
        } else {
            let mut creds: Vec<_> = query_all(in_space("credential"), cx)
                .await?
                .into_iter()
                .filter(|r| field_str(&r.fields, "service") == "anthropic")
                .collect();
            creds.sort_by(|a, b| {
                let active = |r: &crate::types::QueryRow| field_str(&r.fields, "status") == "active";
                active(b).cmp(&active(a)).then(a.created_at.cmp(&b.created_at))
            });
            creds.into_iter().next()
        };
        // A view's Agent filter seeds its records' guest list; an agent has none of its own.
        let mut fields: Fields = seeds.into_iter().filter(|(k, _)| k != "agent").collect();
        fields.extend(channel_field(&channel_id));
        if let Some(a) = &assistant {
            fields.insert("prompt".into(), Value::link(&a.id, "prompt"));
        }
        if !tools.is_empty() {
            fields.insert("tools".into(), Value::values(tools.iter().map(|t| Value::link(&t.id, "tools")).collect()));
        }
        if let Some(c) = &model_cred {
            fields.insert("credentials".into(), Value::values(vec![Value::link(&c.id, "credentials")]));
        }
        if !has("model") {
            fields.insert("model".into(), Value::values(vec![Value::string(DEFAULT_AGENT_MODEL)]));
        }
        let name = if name.is_empty() { "New agent".to_string() } else { name };
        let agent_id = cx.update(|cx| api::note::create(&name, Some("agent"), Some(fields), cx)).await?;
        if let Some(tpl) = &tpl {
            cx.update(|cx| apply_template(&agent_id, &tpl.id, cx)).await?;
        }
        Ok(agent_id)
    })
}

/// `createTyped`: make an object of `type_key` in `channel_id` and open it.
/// Resolves to its id, or "" when nothing was created (no machine object for
/// this computer yet; a cancelled file pick). Errors carry the sentence to
/// show (e.g. a failed file add).
pub fn create_typed(
    type_key: &str,
    channel_id: &str,
    name: &str,
    cx: &mut App,
) -> Task<Result<String>> {
    let key = type_key.trim().to_lowercase();
    let channel_id = channel_id.to_string();
    let name = name.to_string();
    match key.as_str() {
        "agent" => {
            let task = new_agent(&channel_id, &name, Fields::new(), cx);
            cx.spawn(async move |cx| {
                let id = task.await?;
                open(&id, cx);
                Ok(id)
            })
        }
        "machine" => create_machine(cx),
        "credential" => create_credential(channel_id, name, cx),
        "file" => create_file(channel_id, cx),
        _ => cx.spawn(async move |cx| {
            let id = cx
                .update(|cx| {
                    api::note::create(&name, Some(&key), Some(channel_field(&channel_id)), cx)
                })
                .await?;
            let tpl_id =
                cx.update(|cx| default_template_of(Store::global(cx).read(cx), &key, &channel_id));
            if !tpl_id.is_empty() {
                // A deleted default template is a no-op.
                let _ = cx.update(|cx| apply_template(&id, &tpl_id, cx)).await;
            }
            open(&id, cx);
            Ok(id)
        }),
    }
}

/// A computer object is a machine's self-publication: adopt this device's
/// row when its harness already published it, else create nothing ("").
fn create_machine(cx: &mut App) -> Task<Result<String>> {
    let machine_id = this_machine_id(cx);
    cx.spawn(async move |cx| {
        let id = machine_id.await;
        if id.is_empty() {
            return Ok(String::new());
        }
        let rows = query_all(json!({ "type": "machine" }), cx).await?;
        match rows
            .into_iter()
            .find(|r| field_str(&r.fields, "machine_id") == id)
        {
            Some(mine) => {
                open(&mine.id, cx);
                Ok(mine.id)
            }
            None => Ok(String::new()),
        }
    })
}

/// A new credential starts served by this computer, when known; the type's
/// default template still applies.
fn create_credential(channel_id: String, name: String, cx: &mut App) -> Task<Result<String>> {
    let machine_id = this_machine_id(cx);
    cx.spawn(async move |cx| {
        let machine_id = machine_id.await;
        let mut fields = channel_field(&channel_id);
        if !machine_id.is_empty() {
            fields.insert("served_by".into(), Value::string(machine_id));
        }
        let name = if name.is_empty() {
            "New credential".to_string()
        } else {
            name
        };
        let id = cx
            .update(|cx| api::note::create(&name, Some("credential"), Some(fields), cx))
            .await?;
        let tpl_id = cx.update(|cx| {
            default_template_of(Store::global(cx).read(cx), "credential", &channel_id)
        });
        if !tpl_id.is_empty() {
            let _ = cx.update(|cx| apply_template(&id, &tpl_id, cx)).await;
        }
        open(&id, cx);
        Ok(id)
    })
}

/// The MIME type the harness records for a picked file.
#[cfg(not(target_family = "wasm"))]
fn mime_of(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "",
    }
}

/// Upload `bytes` through this computer's harness, which creates the File
/// object in `space_id`; resolves to its id.
pub async fn upload_file(
    harness: &Harness,
    file_name: &str,
    mime: &str,
    bytes: &[u8],
    space_id: &str,
) -> Result<String> {
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    let path = format!(
        "/files?name={}&mime={}&space={}",
        enc(file_name),
        enc(mime),
        enc(space_id)
    );
    let answer = harness
        .post_bytes(&path, bytes, "application/octet-stream")
        .await?;
    Ok(answer
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string())
}

/// Why `name` could not be added, as the person should read it (`addFailureText`).
pub fn add_failure_text(error: &anyhow::Error, name: &str) -> String {
    if harness::is_unreachable(error) {
        NOT_RUNNING_TO_ADD.to_string()
    } else {
        format!("Could not add {name}: {error}")
    }
}

/// A file is its bytes: pick first, then upload through the harness (which
/// creates the File object) and open it. Cancelled pick → "".
#[cfg(target_family = "wasm")]
fn create_file(_channel_id: String, _cx: &mut App) -> Task<Result<String>> {
    Task::ready(Err(anyhow!(io::NO_FILE_PICKER)))
}

/// How a picked file becomes a File object (files.ts `uploadFile`): the
/// editor installs it (`roostr_editor::files::upload_file`, harness or
/// Blossom); resolves to the new object's id.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct AddFile(
    pub std::rc::Rc<dyn Fn(String, String, Vec<u8>, String, &mut App) -> Task<Result<String>>>,
);

impl gpui_kit::Global for AddFile {}

/// A file is its bytes: pick first, then add it (through the harness, or
/// Blossom on this device's own replica) and open it. Cancelled pick → "".
#[cfg(not(target_family = "wasm"))]
fn create_file(channel_id: String, cx: &mut App) -> Task<Result<String>> {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    let harness = harness::harness(cx);
    let add = cx.try_global::<AddFile>().cloned();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return Ok(String::new());
        };
        let Some(path) = paths.into_iter().next() else {
            return Ok(String::new());
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mime = mime_of(&path);
        if let Some(AddFile(add)) = add {
            let bytes =
                std::fs::read(&path).map_err(|e| anyhow!(add_failure_text(&e.into(), &name)))?;
            let task = cx.update(|cx| add(name.clone(), mime.to_string(), bytes, channel_id, cx));
            let id = task
                .await
                .map_err(|e| anyhow!(add_failure_text(&e, &name)))?;
            open(&id, cx);
            return Ok(id);
        }
        let Some(harness) = harness else {
            return Err(anyhow!(NOT_RUNNING_TO_ADD));
        };
        let upload = io::spawn_async(cx, {
            let name = name.clone();
            async move {
                let bytes = std::fs::read(&path)?;
                upload_file(&harness, &name, mime, &bytes, &channel_id).await
            }
        });
        let id = upload
            .await
            .map_err(|e| anyhow!(add_failure_text(&e, &name)))?;
        open(&id, cx);
        Ok(id)
    })
}

/// A new, empty collection in the space, opened.
pub fn create_collection(channel_id: &str, cx: &mut App) -> Task<Result<String>> {
    let mut fields = channel_field(channel_id);
    fields.insert("collectionIds".into(), Value::values(vec![]));
    let create = api::note::create("New collection", Some("collection"), Some(fields), cx);
    cx.spawn(async move |cx| {
        let id = create.await?;
        open(&id, cx);
        Ok(id)
    })
}

/// A sourceless query, opened (the query page then asks for its source).
pub fn create_query(channel_id: &str, cx: &mut App) -> Task<Result<String>> {
    let mut fields = channel_field(channel_id);
    fields.insert("setOf".into(), Value::values(vec![]));
    let create = api::note::create("New query", Some("query"), Some(fields), cx);
    cx.spawn(async move |cx| {
        let id = create.await?;
        open(&id, cx);
        Ok(id)
    })
}

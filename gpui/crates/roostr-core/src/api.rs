//! Typed wrappers over `/api/mutate` and `/api/query` (`src/lib/api.ts`).
//!
//! Every call goes through the global [`Store`], which adds `now_ms` and
//! `tz_offset_min` and refetches the mutated object. All return a `Task`;
//! fire-and-forget callers use `.detach_and_log_err(cx)`, callers that show
//! errors await it inside `cx.spawn`.

use anyhow::Result;
use gpui_kit::AppContext as _;
use gpui_kit::{App, Task};
use serde::Serialize;
use serde_json::{Value as Json, json};

use crate::backend::JsonMap;
use crate::color::{DEFAULT_CHAT_COLOR, normalize_hex};
use crate::store::{Store, USER_PREFS_ID};
use crate::types::{
    AgentMessage, Block, BlockContent, Fields, QueryResult, QueryRow, RepeatRule, Value,
};

fn to_json<T: Serialize>(value: &T) -> Json {
    serde_json::to_value(value).expect("serializable")
}

fn params(value: Json) -> JsonMap {
    match value {
        Json::Object(map) => map,
        _ => JsonMap::new(),
    }
}

/// `mutate(action, params)` with the planner clock.
pub fn mutate(action: &str, params: JsonMap, cx: &mut App) -> Task<Result<JsonMap>> {
    Store::global(cx).update(cx, |store, cx| store.mutate(action, params, cx))
}

/// `fetchQuery`: one page as the body asks.
pub fn query(body: Json, cx: &App) -> Task<Result<QueryResult>> {
    Store::global(cx).read(cx).query(body, cx)
}

/// `fetchAllQuery`: every match.
pub fn query_all(body: Json, cx: &App) -> Task<Result<Vec<QueryRow>>> {
    Store::global(cx).read(cx).query_all(body, cx)
}

fn result_id(result: Result<JsonMap>) -> Result<String> {
    let map = result?;
    Ok(map
        .get("id")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string())
}

/// `note.*`: objects, blocks and fields.
pub mod note {
    use super::*;

    /// Create an object; resolves to its id.
    pub fn create(
        name: &str,
        type_key: Option<&str>,
        fields: Option<Fields>,
        cx: &mut App,
    ) -> Task<Result<String>> {
        let mut p = JsonMap::new();
        p.insert("name".into(), json!(name));
        if let Some(type_key) = type_key {
            p.insert("type_key".into(), json!(type_key));
        }
        if let Some(fields) = fields {
            p.insert("fields".into(), to_json(&fields));
        }
        let task = mutate("create", p, cx);
        cx.background_spawn(async move { result_id(task.await) })
    }

    /// Add a block (`id` may be empty: the engine mints one). `target_id` ""
    /// is the root; `position` is a [`crate::types::pos`] value.
    pub fn block_add(
        object_id: &str,
        block: &Block,
        target_id: &str,
        position: u32,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        let mut block_json = to_json(block);
        if block.id.is_empty()
            && let Some(map) = block_json.as_object_mut()
        {
            map.remove("id");
        }
        mutate(
            "block_add",
            params(
                json!({ "object_id": object_id, "block": block_json, "target_id": target_id, "position": position }),
            ),
            cx,
        )
    }

    pub fn block_update(
        object_id: &str,
        block_id: &str,
        content: &BlockContent,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "block_update",
            params(
                json!({ "object_id": object_id, "block_id": block_id, "content": to_json(content) }),
            ),
            cx,
        )
    }

    pub fn block_move(
        object_id: &str,
        block_id: &str,
        target_id: &str,
        position: u32,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "block_move",
            params(
                json!({ "object_id": object_id, "block_id": block_id, "target_id": target_id, "position": position }),
            ),
            cx,
        )
    }

    /// `align` and/or `background_color` (clear = `Some(0)`, `Some("")`).
    pub fn block_set_attrs(
        object_id: &str,
        block_id: &str,
        align: Option<u32>,
        background_color: Option<&str>,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        let mut p = params(json!({ "object_id": object_id, "block_id": block_id }));
        if let Some(align) = align {
            p.insert("align".into(), json!(align));
        }
        if let Some(color) = background_color {
            p.insert("background_color".into(), json!(color));
        }
        mutate("block_set_attrs", p, cx)
    }

    pub fn block_remove(object_id: &str, block_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "block_remove",
            params(json!({ "object_id": object_id, "block_id": block_id })),
            cx,
        )
    }

    /// Resolves to `next` when it advanced a recurring object.
    pub fn set_field(
        object_id: &str,
        key: &str,
        value: Value,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "set_field",
            params(json!({ "object_id": object_id, "key": key, "value": to_json(&value) })),
            cx,
        )
    }

    pub fn delete_field(object_id: &str, key: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "delete_field",
            params(json!({ "object_id": object_id, "key": key })),
            cx,
        )
    }

    /// Move to the bin.
    pub fn delete(object_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate("delete", params(json!({ "object_id": object_id })), cx)
    }

    /// Restore from the bin under its own type.
    pub fn restore(object_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate("restore", params(json!({ "object_id": object_id })), cx)
    }

    /// Retype in place: history, blocks and fields survive.
    pub fn set_type(object_id: &str, type_key: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "set_type",
            params(json!({ "object_id": object_id, "type_key": type_key })),
            cx,
        )
    }

    /// Erase objects for good (resolves to `vanished` count in the map).
    pub fn vanish(object_ids: &[String], cx: &mut App) -> Task<Result<JsonMap>> {
        mutate("vanish", params(json!({ "object_ids": object_ids })), cx)
    }
}

/// The user's own preferences (`userPrefs`): a personal object under a fixed id.
pub mod user_prefs {
    use std::collections::HashMap;

    use super::*;

    fn ensure(cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "create",
            params(json!({
                "id": USER_PREFS_ID,
                "name": "Preferences",
                "type_key": "user_prefs",
                "fields": { "channel": { "stringValue": "" } },
            })),
            cx,
        )
    }

    /// Save a hex chat color (the default stores as empty). Errors on invalid hex.
    pub fn set_chat_color(raw: &str, cx: &mut App) -> Task<Result<String>> {
        let Some(hex) = normalize_hex(raw) else {
            return Task::ready(Err(anyhow::anyhow!("Use a hex color like #EDCF7F")));
        };
        let ensure = ensure(cx);
        let stored = if hex == DEFAULT_CHAT_COLOR {
            String::new()
        } else {
            hex.clone()
        };
        let set = note::set_field(USER_PREFS_ID, "chat_color", Value::string(stored), cx);
        cx.background_spawn(async move {
            ensure.await?;
            set.await?;
            Ok(hex)
        })
    }

    /// Save this person's space order (space id -> position).
    pub fn set_space_order(order: &HashMap<String, f64>, cx: &mut App) -> Task<Result<()>> {
        let entries: Fields = order
            .iter()
            .map(|(id, n)| (id.clone(), Value::float(*n)))
            .collect();
        let ensure = ensure(cx);
        let set = note::set_field(USER_PREFS_ID, "space_order", Value::map(entries), cx);
        cx.background_spawn(async move {
            ensure.await?;
            set.await?;
            Ok(())
        })
    }
}

/// Recurring objects: the engine owns the rule and the occurrence math.
pub mod repeat {
    use super::*;

    /// Resolves to the result map (`next`: ms of the first occurrence).
    pub fn set(object_id: &str, rule: &RepeatRule, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "repeat_set",
            params(json!({ "object_id": object_id, "rule": to_json(rule) })),
            cx,
        )
    }

    pub fn clear(object_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "repeat_clear",
            params(json!({ "object_id": object_id })),
            cx,
        )
    }
}

/// Inline table blocks.
pub mod table {
    use super::*;

    /// Resolves to the new table block id.
    pub fn create(
        object_id: &str,
        target_id: &str,
        position: u32,
        rows: u32,
        cols: u32,
        cx: &mut App,
    ) -> Task<Result<String>> {
        let task = mutate(
            "table_create",
            params(
                json!({ "object_id": object_id, "target_id": target_id, "position": position, "rows": rows, "cols": cols }),
            ),
            cx,
        );
        cx.background_spawn(async move { result_id(task.await) })
    }

    pub fn row_add(object_id: &str, table_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "table_row_add",
            params(json!({ "object_id": object_id, "table_id": table_id })),
            cx,
        )
    }

    pub fn col_add(object_id: &str, table_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "table_col_add",
            params(json!({ "object_id": object_id, "table_id": table_id })),
            cx,
        )
    }

    pub fn col_remove(
        object_id: &str,
        table_id: &str,
        column_id: &str,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "table_col_remove",
            params(json!({ "object_id": object_id, "table_id": table_id, "column_id": column_id })),
            cx,
        )
    }
}

/// Spaces (`channel` objects).
pub mod space {
    use super::*;

    /// Resolves to the new space id.
    pub fn create(name: &str, icon: Option<&str>, cx: &mut App) -> Task<Result<String>> {
        let mut p = params(json!({ "name": name }));
        if let Some(icon) = icon {
            p.insert("icon".into(), json!(icon));
        }
        let task = mutate("channel_create", p, cx);
        cx.background_spawn(async move { result_id(task.await) })
    }

    pub fn member_add(
        channel_id: &str,
        npub: &str,
        role: Option<&str>,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        let mut p = params(json!({ "channel_id": channel_id, "npub": npub }));
        if let Some(role) = role {
            p.insert("role".into(), json!(role));
        }
        mutate("channel_member_add", p, cx)
    }

    pub fn member_remove(channel_id: &str, npub: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "channel_member_remove",
            params(json!({ "channel_id": channel_id, "npub": npub })),
            cx,
        )
    }

    pub fn key_rotate(channel_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "channel_key_rotate",
            params(json!({ "channel_id": channel_id })),
            cx,
        )
    }

    /// A member's delete: only this identity's copies go.
    pub fn leave(channel_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "space_leave",
            params(json!({ "channel_id": channel_id })),
            cx,
        )
    }
}

/// Identity and relays (daemon platform actions: no planner clock).
pub mod settings {
    use super::*;

    /// Replace the machine-wide identity (nsec1… or 64 hex). Everything reloads after.
    pub fn import_key(key: &str, cx: &mut App) -> Task<Result<JsonMap>> {
        Store::global(cx).update(cx, |store, cx| {
            store.mutate_platform("nostr_key_import", params(json!({ "key": key.trim() })), cx)
        })
    }

    pub fn set_relays(relays: &[String], cx: &mut App) -> Task<Result<JsonMap>> {
        Store::global(cx).update(cx, |store, cx| {
            store.mutate_platform("nostr_relays_set", params(json!({ "relays": relays })), cx)
        })
    }
}

/// Human chat on an object.
pub mod chat {
    use super::*;

    /// `thread_id` "" = the human thread. Resolves to the result map (`id`, `threadId`).
    pub fn post(
        object_id: &str,
        text: &str,
        reply_to: &str,
        thread_id: &str,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "chat_post",
            params(
                json!({ "object_id": object_id, "text": text, "reply_to": reply_to, "thread_id": thread_id }),
            ),
            cx,
        )
    }

    /// Toggle this author's `emoji` reaction on a message.
    pub fn react(
        object_id: &str,
        message_id: &str,
        emoji: &str,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        mutate(
            "chat_react",
            params(json!({ "object_id": object_id, "message_id": message_id, "emoji": emoji })),
            cx,
        )
    }
}

/// Agent mailbox exchanges.
pub mod mailbox {
    use super::*;

    /// Resolves to the result map (`id`, `exchangeId`, `threadId`).
    pub fn send(message: &AgentMessage, cx: &mut App) -> Task<Result<JsonMap>> {
        mutate(
            "message_send",
            params(json!({ "object_id": message.sender.object_id, "message": to_json(message) })),
            cx,
        )
    }

    /// `stage`: "delivery" | "processing".
    pub fn retry(
        object_id: &str,
        message_id: &str,
        stage: &str,
        recipient_object_id: Option<&str>,
        cx: &mut App,
    ) -> Task<Result<JsonMap>> {
        let mut p =
            params(json!({ "object_id": object_id, "message_id": message_id, "stage": stage }));
        if let Some(recipient) = recipient_object_id {
            p.insert("recipient_object_id".into(), json!(recipient));
        }
        mutate("message_retry", p, cx)
    }
}

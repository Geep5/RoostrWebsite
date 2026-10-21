//! The account's devices (glonOdin `core/device.odin`): every device keeps
//! one personal record, `__device__:<deviceId>` of type `device`, with its
//! name, kind, whether it holds the vault whole ("full") or in part
//! ("lite"), about how much of it, and when it last ran. Settings → "Your
//! devices" lists them; the records are bookkeeping, hidden everywhere else
//! ([`crate::types::SYSTEM_TYPE_KEYS`]).

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Task};
use serde_json::json;

use crate::api;
use crate::backend::JsonMap;
use crate::types::{QueryRow, field_str};

pub const DEVICE_TYPE: &str = "device";
pub const DEVICE_ID_PREFIX: &str = "__device__:";
/// Devices not seen for this long collapse under "Older devices".
pub const OLDER_AFTER_MS: i64 = 30 * DAY_MS;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// One device record as Settings shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceRecord {
    /// The record's object id (`__device__:<deviceId>`).
    pub id: String,
    pub name: String,
    /// A person named it; the device's own name never replaces it.
    pub name_custom: bool,
    /// "engine", "browser", "iphone" or "desktop".
    pub kind: String,
    /// The device holds part of the vault and fetches the rest on demand.
    pub lite: bool,
    /// About how much of the vault it holds, 0-100.
    pub percent: u8,
    /// Epoch ms of its last write; 0 when unknown.
    pub last_seen: i64,
}

impl DeviceRecord {
    pub fn from_row(row: &QueryRow) -> Self {
        let int = |key: &str| row.fields.get(key).and_then(|v| v.as_int()).unwrap_or(0);
        let name = field_str(&row.fields, "name").trim();
        Self {
            id: row.id.clone(),
            name: if name.is_empty() {
                "Unnamed device".into()
            } else {
                name.to_string()
            },
            name_custom: row
                .fields
                .get("name_custom")
                .and_then(|v| v.bool_value)
                .unwrap_or(false),
            kind: field_str(&row.fields, "kind").to_string(),
            lite: field_str(&row.fields, "holding") == "lite",
            percent: int("percent").clamp(0, 100) as u8,
            last_seen: int("last_seen"),
        }
    }

    /// The device id the record is kept under.
    pub fn device_id(&self) -> &str {
        self.id.strip_prefix(DEVICE_ID_PREFIX).unwrap_or(&self.id)
    }

    /// Not seen for [`OLDER_AFTER_MS`].
    pub fn older(&self, now_ms: i64) -> bool {
        now_ms - self.last_seen > OLDER_AFTER_MS
    }
}

/// The record id of `device_id`.
pub fn record_id(device_id: &str) -> String {
    format!("{DEVICE_ID_PREFIX}{device_id}")
}

/// "now", "6 min ago", "2 h ago", "3 days ago" ("1 day ago"); "never" when unknown.
pub fn last_seen_text(now_ms: i64, last_seen: i64) -> String {
    if last_seen <= 0 {
        return "never".into();
    }
    let ago = (now_ms - last_seen).max(0);
    if ago < MINUTE_MS {
        "now".into()
    } else if ago < HOUR_MS {
        format!("{} min ago", ago / MINUTE_MS)
    } else if ago < DAY_MS {
        format!("{} h ago", ago / HOUR_MS)
    } else {
        let days = ago / DAY_MS;
        format!("{days} day{} ago", if days == 1 { "" } else { "s" })
    }
}

/// The list's order and split: this device first, then the most recently
/// seen; those not seen for [`OLDER_AFTER_MS`] apart (this device never is).
pub fn arrange(
    mut records: Vec<DeviceRecord>,
    this_device: &str,
    now_ms: i64,
) -> (Vec<DeviceRecord>, Vec<DeviceRecord>) {
    records.sort_by(|a, b| {
        (b.device_id() == this_device)
            .cmp(&(a.device_id() == this_device))
            .then(b.last_seen.cmp(&a.last_seen))
            .then(a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });
    records
        .into_iter()
        .partition(|r| r.device_id() == this_device || !r.older(now_ms))
}

/// Every device record of the account.
pub fn list(cx: &App) -> Task<Result<Vec<DeviceRecord>>> {
    let rows = api::query_all(json!({ "type": DEVICE_TYPE }), cx);
    cx.background_spawn(async move { Ok(rows.await?.iter().map(DeviceRecord::from_row).collect()) })
}

/// A person's name for a device (`name_custom`: the device keeps it).
pub fn rename(record_id: &str, name: &str, cx: &mut App) -> Task<Result<JsonMap>> {
    let mut params = JsonMap::new();
    params.insert("object_id".into(), json!(record_id));
    params.insert("name".into(), json!(name));
    api::mutate("device_rename", params, cx)
}

/// Remove a device's record; the device writes it again the next time it runs.
pub fn remove(record_id: &str, cx: &mut App) -> Task<Result<JsonMap>> {
    api::note::delete(record_id, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Fields, Value};

    fn row(id: &str, holding: &str, percent: i64, last_seen: i64) -> QueryRow {
        let mut fields = Fields::new();
        fields.insert("name".into(), Value::string(id.to_string()));
        fields.insert("holding".into(), Value::string(holding.to_string()));
        fields.insert("kind".into(), Value::string("browser".to_string()));
        fields.insert("percent".into(), Value::int(percent));
        fields.insert("last_seen".into(), Value::int(last_seen));
        QueryRow {
            id: record_id(id),
            type_key: DEVICE_TYPE.into(),
            fields,
            ..QueryRow::default()
        }
    }

    #[test]
    fn last_seen_words() {
        let now = 100 * DAY_MS;
        assert_eq!(last_seen_text(now, now - 5_000), "now");
        assert_eq!(last_seen_text(now, now + 5_000), "now");
        assert_eq!(last_seen_text(now, now - 6 * MINUTE_MS - 1), "6 min ago");
        assert_eq!(last_seen_text(now, now - 2 * HOUR_MS - 5), "2 h ago");
        assert_eq!(last_seen_text(now, now - DAY_MS), "1 day ago");
        assert_eq!(
            last_seen_text(now, now - 3 * DAY_MS - HOUR_MS),
            "3 days ago"
        );
        assert_eq!(last_seen_text(now, 0), "never");
    }

    #[test]
    fn records_from_rows() {
        let r = DeviceRecord::from_row(&row("phone", "lite", 140, 7));
        assert_eq!(r.device_id(), "phone");
        assert!(r.lite);
        assert_eq!(r.percent, 100);
        assert_eq!(r.last_seen, 7);
        assert!(!r.name_custom);
        assert_eq!(r.kind, "browser");
        let full = DeviceRecord::from_row(&row("mac", "full", 100, 7));
        assert!(!full.lite);
    }

    #[test]
    fn this_device_first_and_older_apart() {
        let now = 100 * DAY_MS;
        let records = vec![
            DeviceRecord::from_row(&row("old", "full", 100, now - 40 * DAY_MS)),
            DeviceRecord::from_row(&row("recent", "lite", 20, now - HOUR_MS)),
            DeviceRecord::from_row(&row("me", "lite", 10, now - 50 * DAY_MS)),
            DeviceRecord::from_row(&row("newest", "full", 100, now - MINUTE_MS)),
        ];
        let (shown, older) = arrange(records, "me", now);
        let ids: Vec<&str> = shown.iter().map(DeviceRecord::device_id).collect();
        assert_eq!(ids, ["me", "newest", "recent"]);
        let older: Vec<&str> = older.iter().map(DeviceRecord::device_id).collect();
        assert_eq!(older, ["old"]);
    }
}

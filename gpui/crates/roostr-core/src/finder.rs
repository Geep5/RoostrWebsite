//! Finder's brain, run by the app the person types in (roostr-app `finder.rs` drives it): the
//! space's pages ranked for a request, the agent a hand-off goes to, and one turn of the
//! Finder agent's model (Claude, with the agent's own Anthropic login).
//!
//! With no model to ask - no internet, no login, a model this app can't call - the app falls
//! back to [`rank_pages`] alone: the pages the words name, as cards.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value as Json, json};

use crate::http;
use crate::types::{Fields, QueryRow, field_str};

pub const SESSION_TYPE: &str = "finder_session";
/// Tool calls one answer may make before it must reply.
pub const MAX_STEPS: usize = 6;
/// The model a Finder runs on when its agent names none.
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5";

/// Types that are setup, not pages a request belongs on.
const NOT_PAGES: &[&str] = &[
    "relation",
    "type",
    "agent",
    "credential",
    "system_prompt",
    "tool",
    "skill",
    "template",
    "machine",
    "capability",
    "channel",
    "file",
    "reaction",
    SESSION_TYPE,
];

/// Now, in epoch ms.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The space's Finder agent.
pub fn finder_id(space: &str) -> String {
    format!("finder-agent-{space}")
}

pub fn is_page(type_key: &str) -> bool {
    !NOT_PAGES.contains(&type_key)
}

/// One page as Finder sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PageHit {
    pub id: String,
    pub name: String,
    pub type_key: String,
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Pages of `space` matching `query`: every word that matches a name scores (a whole word
/// twice), a request naming every word of a page's name scores more, recent edits break ties.
pub fn rank_pages(rows: &[QueryRow], space: &str, query: &str, limit: usize) -> Vec<PageHit> {
    let asked: Vec<String> = words(query)
        .into_iter()
        .filter(|w| w.chars().count() > 1)
        .collect();
    let mut scored: Vec<(usize, i64, PageHit)> = rows
        .iter()
        .filter(|r| field_str(&r.fields, "channel") == space && is_page(&r.type_key))
        .filter_map(|r| {
            let name = field_str(&r.fields, "name").trim();
            if name.is_empty() {
                return None;
            }
            let lower = name.to_lowercase();
            let name_words = words(name);
            let matched: usize = asked
                .iter()
                .map(|w| {
                    if !lower.contains(w.as_str()) {
                        0
                    } else if name_words.contains(w) {
                        2
                    } else {
                        1
                    }
                })
                .sum();
            let named = matched > 0 && name_words.iter().all(|w| asked.contains(w));
            let score = matched + if named { 3 } else { 0 };
            (score > 0).then(|| {
                (
                    score,
                    r.updated_at,
                    PageHit {
                        id: r.id.clone(),
                        name: name.to_string(),
                        type_key: r.type_key.clone(),
                    },
                )
            })
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    scored.into_iter().take(limit).map(|(_, _, h)| h).collect()
}

/// The space's agents a request can go to: every agent but its Finder.
pub fn agents(rows: &[QueryRow]) -> impl Iterator<Item = &QueryRow> {
    rows.iter()
        .filter(|r| r.type_key == "agent" && !r.id.starts_with("finder-agent-"))
}

/// The agent a hand-off goes to: the one asked for, else the page's own, else the space agent.
/// `fallback`: not the one asked for (the person confirms it first).
pub fn hand_off_agent(
    rows: &[QueryRow],
    page: &QueryRow,
    space: &str,
    space_name: &str,
    asked: &str,
) -> Option<(String, bool)> {
    let known = |id: &str| agents(rows).any(|a| a.id == id);
    if !asked.is_empty() && known(asked) {
        return Some((asked.to_string(), false));
    }
    if let Some(own) = page
        .fields
        .get("agent")
        .map(|v| v.link_ids())
        .unwrap_or_default()
        .into_iter()
        .find(|id| known(id))
    {
        return Some((own, true));
    }
    let space_agent = format!("space-agent-{space}");
    let named = format!("{space_name} Space");
    agents(rows)
        .find(|a| a.id == space_agent)
        .or_else(|| agents(rows).find(|a| field_str(&a.fields, "name") == named))
        .map(|a| (a.id.clone(), true))
}

/// The space's agents with their job properties, for the prompt.
pub fn roster(rows: &[QueryRow]) -> String {
    agents(rows)
        .map(|a| {
            let line = |k: &str| {
                field_str(&a.fields, k)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let role = line("agent_role");
            let mut out = format!(
                "- {} (id {}): {}",
                field_str(&a.fields, "name"),
                a.id,
                if role.is_empty() {
                    "no Role yet"
                } else {
                    &role
                }
            );
            for (key, label) in [("good_for", "Good for"), ("not_for", "Not for")] {
                let v = line(key);
                if !v.is_empty() {
                    out.push_str(&format!(" | {label}: {v}"));
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A page as chat shows it: a link the app draws as the page's pill.
pub fn page_link(id: &str, name: &str) -> String {
    format!("[{}](/app/object/{id})", name.replace(['[', ']'], ""))
}

/// The model an agent names in its Model property.
pub fn model_of(agent: &Fields) -> String {
    agent
        .get("model")
        .and_then(|v| {
            v.items()
                .first()
                .and_then(|i| i.as_str())
                .or_else(|| v.as_str())
        })
        .filter(|m| !m.is_empty())
        .unwrap_or(DEFAULT_MODEL)
        .to_string()
}

/// Margin before expiry at which a plan token no longer counts as good (glonOdin anthropic-plan.ts).
const PLAN_GOOD_FOR_MS: i64 = 60_000;

/// What to call Claude with from an Anthropic credential's fields: the Claude plan while its
/// token is good, else the pasted API key, else None.
pub fn anthropic_key(fields: &Fields, now_ms: i64) -> Option<String> {
    let token = field_str(fields, "plan_token");
    let expires = fields
        .get("plan_expires")
        .and_then(|v| v.as_int())
        .unwrap_or(0);
    if !token.is_empty()
        && field_str(fields, "plan_error").is_empty()
        && expires > now_ms + PLAN_GOOD_FOR_MS
    {
        return Some(token.to_string());
    }
    let key = field_str(fields, "key_api_key").trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// One tool call the model made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Json,
}

/// One model turn: what it wrote and the tools it called.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub tool_uses: Vec<ToolUse>,
    /// The turn as the API returned it, to send back with the tool results.
    pub content: Json,
}

/// Finder's tools (glonOdin's harness Finder had the same).
pub fn tools() -> Json {
    json!([
        {
            "name": "find_pages",
            "description": "Search this space's pages (objects, queries, collections) by name. Returns up to 8: id, name, type.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string", "description": "words from the request" } }, "required": ["query"] }
        },
        {
            "name": "show_pages",
            "description": "Show pages as cards the person taps to choose (a '+ New page' card is added). Use this whenever you have candidate pages - never ask about a page in words. Ends your turn.",
            "input_schema": { "type": "object", "properties": { "prompt": { "type": "string", "description": "one short line above the cards" }, "page_ids": { "type": "array", "items": { "type": "string" } } }, "required": ["prompt", "page_ids"] }
        },
        {
            "name": "new_page",
            "description": "Make a new page in this space; only after the person tapped '+ New page'. Returns its id for hand_off.",
            "input_schema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
        },
        {
            "name": "open_page",
            "description": "Open a page for the person, and nothing else: no agent is asked. Use it when they want to find, see, open, read or check something. Ends your turn.",
            "input_schema": { "type": "object", "properties": { "page_id": { "type": "string" } }, "required": ["page_id"] }
        },
        {
            "name": "hand_off",
            "description": "Give the request to an agent on a page - only when an agent has work to do there (add, write, change, send, sort out…); to find or see something use open_page. Leave agent_id out when no agent's Role / Good for clearly fits: the page's agent, else the space agent, takes it. sure=false shows the person [Do it] first. Ends your turn.",
            "input_schema": { "type": "object", "properties": {
                "page_id": { "type": "string" },
                "agent_id": { "type": "string" },
                "message": { "type": "string", "description": "the person's request, briefly, as theirs (\"Grant asks: …\")" },
                "sure": { "type": "boolean", "description": "the page and agent are clearly right" }
            }, "required": ["page_id", "message", "sure"] }
        }
    ])
}

/// A plan login must look like Claude Code (glonOdin harness llm.ts).
const CLAUDE_CODE_VERSION: &str = "2.1.302";
const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_CODE_TOOL_PREFIX: &str = "proxy_";
const CLAUDE_CODE_BETAS: &str = "claude-code-20250219,oauth-2025-04-20";

/// One turn of a Claude model: `messages` in the Messages API's shape, `tools` as [`tools`].
pub async fn call_claude(
    key: &str,
    model: &str,
    system: &str,
    messages: &[Json],
    tools: &Json,
) -> Result<Reply> {
    if !model.starts_with("claude") {
        bail!("Finder in the app runs on Claude models; {model} isn't one");
    }
    let plan = key.starts_with("sk-ant-oat");
    let tools: Vec<Json> = tools
        .as_array()
        .map(|list| {
            list.iter()
                .map(|t| {
                    let mut t = t.clone();
                    if plan && let Some(name) = t["name"].as_str() {
                        t["name"] = json!(format!("{CLAUDE_CODE_TOOL_PREFIX}{name}"));
                    }
                    t
                })
                .collect()
        })
        .unwrap_or_default();
    let system = if plan {
        json!([{ "type": "text", "text": CLAUDE_CODE_SYSTEM }, { "type": "text", "text": system }])
    } else {
        json!(system)
    };
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "system": system,
        "messages": messages,
        "tools": tools,
    });
    let bearer = format!("Bearer {key}");
    let agent = format!("claude-cli/{CLAUDE_CODE_VERSION} (external, cli)");
    let mut headers: Vec<(&str, &str)> = vec![
        ("content-type", "application/json"),
        ("anthropic-version", "2023-06-01"),
    ];
    if plan {
        headers.extend([
            ("authorization", bearer.as_str()),
            ("anthropic-beta", CLAUDE_CODE_BETAS),
            ("user-agent", agent.as_str()),
            ("x-app", "cli"),
        ]);
    } else {
        headers.push(("x-api-key", key));
    }
    // A browser may call Anthropic only when the request says it means to.
    if cfg!(target_family = "wasm") {
        headers.push(("anthropic-dangerous-direct-browser-access", "true"));
    }
    let res = http::post(
        "https://api.anthropic.com/v1/messages",
        &headers,
        &serde_json::to_vec(&body)?,
        Some(Duration::from_secs(60)),
        4 << 20,
    )
    .await?;
    let reply: Json = serde_json::from_slice(&res.body).unwrap_or(Json::Null);
    if !res.is_success() {
        let why = reply["error"]["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                String::from_utf8_lossy(&res.body)
                    .chars()
                    .take(300)
                    .collect()
            });
        return Err(anyhow!("anthropic {}: {why}", res.status));
    }
    Ok(parse_reply(reply, plan))
}

/// A Messages API answer as a [`Reply`]; `plan`: tool names carry the Claude Code prefix.
pub fn parse_reply(reply: Json, plan: bool) -> Reply {
    let mut out = Reply::default();
    let mut content = reply["content"].as_array().cloned().unwrap_or_default();
    for part in &mut content {
        match part["type"].as_str() {
            Some("text") => {
                if !out.text.is_empty() {
                    out.text.push('\n');
                }
                out.text.push_str(part["text"].as_str().unwrap_or_default());
            }
            Some("tool_use") => {
                let raw = part["name"].as_str().unwrap_or_default();
                let name = if plan {
                    raw.strip_prefix(CLAUDE_CODE_TOOL_PREFIX).unwrap_or(raw)
                } else {
                    raw
                }
                .to_string();
                out.tool_uses.push(ToolUse {
                    id: part["id"].as_str().unwrap_or_default().to_string(),
                    name,
                    input: part["input"].clone(),
                });
            }
            _ => {}
        }
    }
    out.content = Json::Array(content);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;

    fn row(
        id: &str,
        type_key: &str,
        name: &str,
        updated_at: i64,
        extra: &[(&str, Value)],
    ) -> QueryRow {
        let mut fields: Fields = [
            ("name".to_string(), Value::string(name)),
            ("channel".to_string(), Value::string("sp")),
        ]
        .into();
        for (k, v) in extra {
            fields.insert(k.to_string(), v.clone());
        }
        QueryRow {
            id: id.into(),
            type_key: type_key.into(),
            updated_at,
            fields,
            ..QueryRow::default()
        }
    }

    #[test]
    fn a_request_naming_a_page_ranks_it_first_and_setup_never_shows() {
        let rows = vec![
            row("hunt", "page", "Hunter Overview", 1, &[]),
            row("notes", "page", "Hunting notes", 9, &[]),
            row("agent", "agent", "Hunter Overview", 9, &[]),
            row("other", "page", "Groceries", 9, &[]),
        ];
        let hits = rank_pages(&rows, "sp", "add this link to the hunter overview", 8);
        // "Hunting notes" shares no word with it; "hunting" finds both, the whole word first.
        let hunting = rank_pages(&rows, "sp", "my hunting", 8);
        assert_eq!(hunting.first().map(|h| h.id.as_str()), Some("notes"));
        assert_eq!(
            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            ["hunt"]
        );
        // Another space's page never shows.
        assert!(rank_pages(&rows, "elsewhere", "hunter", 8).is_empty());
    }

    #[test]
    fn a_hand_off_goes_to_the_asked_agent_else_the_pages_else_the_space_agent() {
        let page = row(
            "p",
            "page",
            "P",
            0,
            &[("agent", Value::link("a-page", "agent"))],
        );
        let bare = row("q", "page", "Q", 0, &[]);
        let rows = vec![
            row("a-asked", "agent", "Asked", 0, &[]),
            row("a-page", "agent", "Page's", 0, &[]),
            row("space-agent-sp", "agent", "Sp Space", 0, &[]),
            row("finder-agent-sp", "agent", "Finder", 0, &[]),
        ];
        let pick = |page: &QueryRow, asked: &str| hand_off_agent(&rows, page, "sp", "Sp", asked);
        assert_eq!(pick(&page, "a-asked"), Some(("a-asked".into(), false)));
        assert_eq!(pick(&page, "nobody"), Some(("a-page".into(), true)));
        assert_eq!(pick(&bare, ""), Some(("space-agent-sp".into(), true)));
        // Finder never hands to itself.
        assert_eq!(
            pick(&bare, "finder-agent-sp"),
            Some(("space-agent-sp".into(), true))
        );
    }

    #[test]
    fn the_plan_login_is_used_while_good_then_the_api_key() {
        let now = 1_000_000;
        let fields = |expires: i64, error: &str, key: &str| -> Fields {
            [
                ("plan_token".to_string(), Value::string("sk-ant-oat-1")),
                ("plan_expires".to_string(), Value::int(expires)),
                ("plan_error".to_string(), Value::string(error)),
                ("key_api_key".to_string(), Value::string(key)),
            ]
            .into()
        };
        assert_eq!(
            anthropic_key(&fields(now + 3_600_000, "", "sk-k"), now).as_deref(),
            Some("sk-ant-oat-1")
        );
        // Expiring within the minute, or refused: the API key.
        assert_eq!(
            anthropic_key(&fields(now + 30_000, "", "sk-k"), now).as_deref(),
            Some("sk-k")
        );
        assert_eq!(
            anthropic_key(&fields(now + 3_600_000, "revoked", "sk-k"), now).as_deref(),
            Some("sk-k")
        );
        assert_eq!(anthropic_key(&fields(0, "", " "), now), None);
    }

    #[test]
    fn a_plan_reply_drops_the_tool_prefix() {
        let reply = parse_reply(
            json!({ "content": [
                { "type": "text", "text": "Looking" },
                { "type": "tool_use", "id": "t1", "name": "proxy_find_pages", "input": { "query": "hunter" } }
            ] }),
            true,
        );
        assert_eq!(reply.text, "Looking");
        assert_eq!(reply.tool_uses[0].name, "find_pages");
        assert_eq!(reply.tool_uses[0].input["query"], "hunter");
    }
}

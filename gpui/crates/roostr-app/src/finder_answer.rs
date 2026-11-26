//! Finder answers here, in the app the person types in - iPhone, Mac or browser - not on a
//! computer: with the space's Finder agent (its System prompt, Model and Anthropic login, all
//! synced objects) it calls Claude, searches the space's pages in this app's own copy of the
//! vault, shows cards and buttons, and hands off by posting "@Agent …" as Finder on the page's
//! chat - the @ wakes that agent wherever it runs.
//!
//! With no model to ask (offline, no login, a model the app can't call) it still shows the
//! pages the words name, as cards that open the page.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, anyhow};
use roostr_core::finder::{self, PageHit};
use roostr_core::types::{Object, QueryRow, Value, field_str};
use roostr_core::{JsonMap, State, api};
use roostr_rt::AsyncCx;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// A button under Finder's message (drawn by `finder.rs`): a page card, a new page, an
/// offline card that opens its page, or a step of the confirm.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub id: String,
    pub name: String,
    /// "page" | "new" | "open" | "do" | "pick_agent" | "agent".
    pub kind: String,
    #[serde(default, rename = "type", skip_serializing_if = "String::is_empty")]
    pub type_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
}

/// A hand-off waiting on [✅ Do it].
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    page: String,
    agent: String,
    message: String,
}

/// One chat line of a session.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub author: String,
    pub ts: i64,
    pub text: String,
}

/// The session's chat, oldest first.
pub(crate) fn lines(session: &Object) -> Vec<Line> {
    let mut out: Vec<Line> = session
        .blocks
        .iter()
        .filter_map(|b| {
            let c = b.content.custom.as_ref()?;
            if c.content_type != "chat" {
                return None;
            }
            let meta = c.meta.as_ref()?;
            let get = |k: &str| meta.get(k).cloned().unwrap_or_default();
            // Reactions ride on chat lines too; they are not messages.
            if get("kind") == "reaction" {
                return None;
            }
            Some(Line {
                author: get("author"),
                ts: get("ts").parse().unwrap_or(0),
                text: get("text"),
            })
        })
        .collect();
    out.sort_by_key(|l| l.ts);
    out
}

fn parse<T: for<'de> Deserialize<'de>>(raw: &str) -> Option<T> {
    serde_json::from_str(raw).ok()
}

/// What one answer works with.
struct Ctx {
    session: Object,
    space: String,
    space_name: String,
    finder: Object,
    rows: Vec<QueryRow>,
}

impl Ctx {
    fn page(&self, id: &str) -> Option<&QueryRow> {
        self.rows
            .iter()
            .find(|r| r.id == id && finder::is_page(&r.type_key))
    }
}

async fn fetch(cx: &AsyncCx<State>, id: &str) -> Result<Object> {
    cx.update(|s| s.store.fetch_object(id)).await
}

async fn mutate(cx: &AsyncCx<State>, action: &str, params: Json) -> Result<JsonMap> {
    let Json::Object(params) = params else {
        return Err(anyhow!("mutate params must be an object"));
    };
    cx.update(|s| api::mutate(action, params, s)).await
}

async fn say(cx: &AsyncCx<State>, ctx: &Ctx, text: &str) -> Result<()> {
    mutate(
        cx,
        "chat_post",
        json!({ "object_id": ctx.session.id, "text": text, "as_author": ctx.finder.id }),
    )
    .await
    .map(drop)
}

async fn set(cx: &AsyncCx<State>, object: &str, key: &str, value: &str) -> Result<()> {
    let value = Value::string(value);
    cx.update(|s| api::note::set_field(object, key, value, s))
        .await
        .map(drop)
}

/// Write the buttons under Finder's next message, and that message.
async fn offer(cx: &AsyncCx<State>, ctx: &Ctx, line: &str, options: &[Choice]) -> Result<()> {
    set(
        cx,
        &ctx.session.id,
        "finder_options",
        &serde_json::to_string(options)?,
    )
    .await?;
    say(cx, ctx, line).await
}

fn page_cards(ctx: &Ctx, ids: &[String]) -> Vec<Choice> {
    let mut cards: Vec<Choice> = ids
        .iter()
        .filter_map(|id| ctx.page(id))
        .take(3)
        .map(|r| Choice {
            id: r.id.clone(),
            name: field_str(&r.fields, "name").to_string(),
            kind: "page".into(),
            type_key: r.type_key.clone(),
            icon: field_str(&r.fields, "iconEmoji").to_string(),
        })
        .collect();
    cards.push(Choice {
        id: "new".into(),
        name: "＋ New page".into(),
        kind: "new".into(),
        ..Choice::default()
    });
    cards
}

/// Open `page` in the person's app (the hand-off the panel follows), asking no agent.
async fn open_page(cx: &AsyncCx<State>, ctx: &Ctx, id: &str, name: &str) -> Result<()> {
    let at = roostr_core::finder::now_ms();
    set(
        cx,
        &ctx.session.id,
        "finder_handoff",
        &json!({ "page": id, "at": at }).to_string(),
    )
    .await?;
    set(cx, &ctx.session.id, "finder_options", "").await?;
    set(cx, &ctx.session.id, "finder_pending", "").await?;
    say(cx, ctx, &format!("Opened {}.", finder::page_link(id, name))).await
}

/// Post the request on the page as Finder, tagging the agent, and open the page.
async fn send(cx: &AsyncCx<State>, ctx: &Ctx, p: &Pending) -> Result<()> {
    let page = match ctx.rows.iter().find(|r| r.id == p.page) {
        Some(r) => Some((r.id.clone(), field_str(&r.fields, "name").to_string())),
        None => fetch(cx, &p.page)
            .await
            .ok()
            .filter(|o| !o.deleted)
            .map(|o| (o.id.clone(), o.field_str("name").to_string())),
    };
    let agent = ctx.rows.iter().find(|r| r.id == p.agent);
    let (Some((page, page_name)), Some(agent)) = (page, agent) else {
        return say(
            cx,
            ctx,
            "That page or agent is gone - tell me again what you need.",
        )
        .await;
    };
    let agent_name = field_str(&agent.fields, "name");
    mutate(
        cx,
        "chat_post",
        json!({ "object_id": page, "text": format!("@{agent_name} {}", p.message), "as_author": ctx.finder.id }),
    )
    .await?;
    let at = roostr_core::finder::now_ms();
    set(
        cx,
        &ctx.session.id,
        "finder_handoff",
        &json!({ "page": page, "at": at }).to_string(),
    )
    .await?;
    set(cx, &ctx.session.id, "finder_options", "").await?;
    set(cx, &ctx.session.id, "finder_pending", "").await?;
    say(
        cx,
        ctx,
        &format!(
            "➡️ Sent to @{agent_name} on {}.",
            finder::page_link(&page, &page_name)
        ),
    )
    .await
}

/// A tap on a button offered (its name, posted) that needs no model. True when handled.
async fn tapped(cx: &AsyncCx<State>, ctx: &Ctx, text: &str) -> Result<bool> {
    let options: Vec<Choice> = parse(ctx.session.field_str("finder_options")).unwrap_or_default();
    let Some(hit) = options.iter().find(|o| o.name == text) else {
        return Ok(false);
    };
    match hit.kind.as_str() {
        "open" => {
            open_page(cx, ctx, &hit.id, &hit.name).await?;
            return Ok(true);
        }
        "page" | "new" => return Ok(false),
        _ => {}
    }
    let Some(pending) = parse::<Pending>(ctx.session.field_str("finder_pending")) else {
        return Ok(false);
    };
    match hit.kind.as_str() {
        "do" => send(cx, ctx, &pending).await?,
        "pick_agent" => {
            let agents: Vec<Choice> = finder::agents(&ctx.rows)
                .map(|a| Choice {
                    id: a.id.clone(),
                    name: field_str(&a.fields, "name").to_string(),
                    kind: "agent".into(),
                    icon: field_str(&a.fields, "iconEmoji").to_string(),
                    ..Choice::default()
                })
                .collect();
            offer(cx, ctx, "Which agent?", &agents).await?;
        }
        _ => {
            let pending = Pending {
                agent: hit.id.clone(),
                ..pending
            };
            send(cx, ctx, &pending).await?;
        }
    }
    Ok(true)
}

/// The prompt object's text: its paragraphs, not its chat.
fn object_text(o: &Object) -> String {
    o.blocks
        .iter()
        .filter_map(|b| b.content.text.as_ref().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The key the Finder agent calls Claude with: its first Anthropic credential that has one.
async fn model_key(cx: &AsyncCx<State>, agent: &Object) -> Result<String> {
    let now = roostr_core::finder::now_ms();
    let ids = agent
        .fields
        .get("credentials")
        .map(Value::link_ids)
        .unwrap_or_default();
    let mut empty = String::new();
    for id in ids {
        let Ok(c) = fetch(cx, &id).await else {
            continue;
        };
        if c.deleted || c.field_str("service") != "anthropic" {
            continue;
        }
        if let Some(key) = finder::anthropic_key(&c.fields, now) {
            return Ok(key);
        }
        if empty.is_empty() {
            empty = c.field_str("name").to_string();
        }
    }
    Err(anyhow!(if empty.is_empty() {
        "Finder has no Anthropic login".to_string()
    } else {
        format!("the \"{empty}\" login isn't signed in")
    }))
}

/// Answer the person's newest message in `session` as the space's Finder.
pub(crate) async fn answer(cx: AsyncCx<State>, session: String, space: String) {
    let finder_id = finder::finder_id(&space);
    if let Err(error) = run(&cx, &session, &space, &finder_id).await {
        log::warn!("[finder] {error:#}");
        let text = format!("Finder couldn't run: {error}");
        let text: String = text.chars().take(400).collect();
        let _ = mutate(
            &cx,
            "chat_post",
            json!({ "object_id": session, "text": text, "as_author": finder_id }),
        )
        .await;
    }
}

async fn run(cx: &AsyncCx<State>, session: &str, space: &str, finder_id: &str) -> Result<()> {
    let session = fetch(cx, session).await?;
    let finder = fetch(cx, finder_id)
        .await
        .ok()
        .filter(|o| !o.deleted)
        .context("this space has no Finder yet - its computer adds one")?;
    let body = json!({ "filters": [{ "key": "channel", "condition": "equal", "value": space }] });
    let rows = cx.update(|s| api::query_all(body, s)).await?;
    let space_name = fetch(cx, space)
        .await
        .map(|o| o.field_str("name").to_string())
        .unwrap_or_default();
    let msgs = lines(&session);
    let newest = msgs
        .last()
        .map(|l| l.text.trim().to_string())
        .unwrap_or_default();
    let ctx = Ctx {
        session,
        space: space.to_string(),
        space_name,
        finder,
        rows,
    };
    if tapped(cx, &ctx, &newest).await? {
        return Ok(());
    }
    match think(cx, &ctx, &msgs).await {
        Ok(()) => Ok(()),
        Err(error) => offline(cx, &ctx, &newest, &error.to_string()).await,
    }
}

/// No model to ask: the pages the words name, as cards that open them.
async fn offline(cx: &AsyncCx<State>, ctx: &Ctx, asked: &str, why: &str) -> Result<()> {
    log::info!("[finder] answering without the model: {why}");
    let hits: Vec<PageHit> = finder::rank_pages(&ctx.rows, &ctx.space, asked, 3);
    if hits.is_empty() {
        return say(
            cx,
            ctx,
            &format!("I can't think right now ({why}), and no page's name matches those words. Try again when you're online, or use words from the page's name."),
        )
        .await;
    }
    let cards: Vec<Choice> = hits
        .into_iter()
        .map(|h| {
            let icon = ctx
                .page(&h.id)
                .map(|r| field_str(&r.fields, "iconEmoji").to_string())
                .unwrap_or_default();
            Choice {
                id: h.id,
                name: h.name,
                kind: "open".into(),
                type_key: h.type_key,
                icon,
            }
        })
        .collect();
    offer(
        cx,
        ctx,
        &format!(
            "I can't think right now ({why}) - these pages match your words. Tap one to open it."
        ),
        &cards,
    )
    .await
}

/// The model's turns: search, cards, open or hand off. An `Err` before anything was said
/// means the model couldn't be asked; [`offline`] answers then.
async fn think(cx: &AsyncCx<State>, ctx: &Ctx, msgs: &[Line]) -> Result<()> {
    let finder = &ctx.finder;
    let model = finder::model_of(&finder.fields);
    let key = model_key(cx, finder).await?;
    let prompt = match finder.fields.get("prompt").map(Value::link_ids) {
        Some(ids) if !ids.is_empty() => fetch(cx, &ids[0])
            .await
            .map(|o| object_text(&o))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let offered: Vec<Choice> = parse(ctx.session.field_str("finder_options")).unwrap_or_default();
    let buttons = offered
        .iter()
        .map(|o| format!("{} → {}", o.name, o.id))
        .collect::<Vec<_>>()
        .join("; ");
    let system = [
        prompt,
        format!("This space: {} (id {}).", ctx.space_name, ctx.space),
        "When the person only wants to find, see, open or look something up, open the page with open_page once they picked it (or it is the one clear match) - never hand it to an agent. Hand off only when an agent has work to do.".into(),
        format!("Its agents:\n{}", finder::roster(&ctx.rows)),
        if buttons.is_empty() { String::new() } else { format!("Buttons you last showed (name → id): {buttons}") },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let mut turns: Vec<Json> = msgs[msgs.len().saturating_sub(12)..]
        .iter()
        .map(|m| {
            let role = if m.author == finder.id {
                "assistant"
            } else {
                "user"
            };
            json!({ "role": role, "content": m.text })
        })
        .skip_while(|t| t["role"] == "assistant")
        .collect();
    let wants_new = msgs.last().is_some_and(|m| m.text.trim() == "＋ New page");
    let tools = finder::tools();
    let mut rows_made: BTreeMap<String, String> = BTreeMap::new();
    let mut last_hits: Vec<String> = Vec::new();
    let mut ctx_rows = ctx.rows.clone();
    for _ in 0..finder::MAX_STEPS {
        let (k, m, sys, t, tl) = (
            key.clone(),
            model.clone(),
            system.clone(),
            turns.clone(),
            tools.clone(),
        );
        let reply = cx
            .update(|s| {
                s.rt.background(async move { finder::call_claude(&k, &m, &sys, &t, &tl).await })
            })
            .await?;
        let ctx = Ctx {
            session: ctx.session.clone(),
            space: ctx.space.clone(),
            space_name: ctx.space_name.clone(),
            finder: ctx.finder.clone(),
            rows: ctx_rows.clone(),
        };
        if reply.tool_uses.is_empty() {
            let text = reply.text.trim();
            // Show, don't ask: pages it found go out as cards, whatever it wrote.
            if !last_hits.is_empty() {
                let line = text
                    .lines()
                    .next()
                    .filter(|l| !l.is_empty())
                    .unwrap_or("Which page?");
                offer(cx, &ctx, line, &page_cards(&ctx, &last_hits)).await?;
            } else if !text.is_empty() {
                say(cx, &ctx, text).await?;
            }
            return Ok(());
        }
        let mut results: Vec<Json> = Vec::new();
        let mut result = |id: &str, content: String, error: bool| {
            results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": content, "is_error": error }));
        };
        for usage in &reply.tool_uses {
            let input = &usage.input;
            let arg = |k: &str| input[k].as_str().unwrap_or_default().trim().to_string();
            match usage.name.as_str() {
                "find_pages" => {
                    let hits = finder::rank_pages(&ctx.rows, &ctx.space, &arg("query"), 8);
                    last_hits = hits.iter().map(|h| h.id.clone()).collect();
                    let list: Vec<Json> = hits
                        .iter()
                        .map(|h| json!({ "id": h.id, "name": h.name, "type": h.type_key }))
                        .collect();
                    result(&usage.id, Json::Array(list).to_string(), false);
                }
                "show_pages" => {
                    let ids: Vec<String> = input["page_ids"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_else(|| last_hits.clone());
                    let prompt = Some(arg("prompt"))
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| "Which page?".into());
                    return offer(cx, &ctx, &prompt, &page_cards(&ctx, &ids)).await;
                }
                "new_page" => {
                    if !wants_new {
                        result(&usage.id, "not yet: show_pages first; a new page is made only after the person taps '+ New page'".into(), true);
                        continue;
                    }
                    let name = Some(arg("name"))
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "New page".into());
                    let fields = [("channel".to_string(), Value::string(&ctx.space))].into();
                    let made = cx
                        .update(|s| api::note::create(&name, Some("page"), Some(fields), s))
                        .await;
                    match made {
                        Ok(id) => {
                            let now = roostr_core::finder::now_ms();
                            ctx_rows.push(QueryRow {
                                id: id.clone(),
                                type_key: "page".into(),
                                created_at: now,
                                updated_at: now,
                                fields: [
                                    ("name".to_string(), Value::string(&name)),
                                    ("channel".to_string(), Value::string(&ctx.space)),
                                ]
                                .into(),
                                ..QueryRow::default()
                            });
                            rows_made.insert(id.clone(), name.clone());
                            result(
                                &usage.id,
                                json!({ "id": id, "name": name }).to_string(),
                                false,
                            );
                        }
                        Err(e) => result(&usage.id, format!("could not make the page: {e}"), true),
                    }
                }
                "open_page" => {
                    let id = arg("page_id");
                    let name = ctx
                        .page(&id)
                        .map(|r| field_str(&r.fields, "name").to_string())
                        .or_else(|| rows_made.get(&id).cloned());
                    match name {
                        Some(name) => return open_page(cx, &ctx, &id, &name).await,
                        None => result(
                            &usage.id,
                            "page_id must be a page id from find_pages".into(),
                            true,
                        ),
                    }
                }
                "hand_off" => {
                    let message = arg("message");
                    let page = ctx_rows
                        .iter()
                        .find(|r| r.id == arg("page_id") && finder::is_page(&r.type_key));
                    let (Some(page), false) = (page, message.is_empty()) else {
                        result(&usage.id, "page_id must be a page id from find_pages or new_page, and message not empty".into(), true);
                        continue;
                    };
                    let Some((agent, fallback)) = finder::hand_off_agent(
                        &ctx_rows,
                        page,
                        &ctx.space,
                        &ctx.space_name,
                        &arg("agent_id"),
                    ) else {
                        result(&usage.id, "this space has no agent to take it".into(), true);
                        continue;
                    };
                    let pending = Pending {
                        page: page.id.clone(),
                        agent,
                        message,
                    };
                    if input["sure"].as_bool() == Some(true) && !fallback {
                        return send(cx, &ctx, &pending).await;
                    }
                    set(
                        cx,
                        &ctx.session.id,
                        "finder_pending",
                        &serde_json::to_string(&pending)?,
                    )
                    .await?;
                    let agent_name = ctx_rows
                        .iter()
                        .find(|r| r.id == pending.agent)
                        .map(|r| field_str(&r.fields, "name").to_string())
                        .unwrap_or_default();
                    // "Grant asks: add …" reads "add …" in the confirm.
                    let request = match pending.message.find("asks:") {
                        Some(at) if at < 40 => pending.message[at + 5..].trim(),
                        _ => pending.message.trim(),
                    };
                    let line = format!(
                        "{request} - on {}, with @{agent_name}?",
                        finder::page_link(&page.id, field_str(&page.fields, "name"))
                    );
                    return offer(
                        cx,
                        &ctx,
                        &line,
                        &[
                            Choice {
                                id: "do".into(),
                                name: "✅ Do it".into(),
                                kind: "do".into(),
                                ..Choice::default()
                            },
                            Choice {
                                id: "pick".into(),
                                name: "Pick another agent".into(),
                                kind: "pick_agent".into(),
                                ..Choice::default()
                            },
                        ],
                    )
                    .await;
                }
                other => result(&usage.id, format!("no tool {other}"), true),
            }
        }
        turns.push(json!({ "role": "assistant", "content": reply.content }));
        turns.push(json!({ "role": "user", "content": results }));
    }
    let ctx = Ctx {
        rows: ctx_rows,
        session: ctx.session.clone(),
        space: ctx.space.clone(),
        space_name: ctx.space_name.clone(),
        finder: ctx.finder.clone(),
    };
    if last_hits.is_empty() {
        say(
            cx,
            &ctx,
            "I couldn't find where this goes - tell me the page's name.",
        )
        .await
    } else {
        offer(cx, &ctx, "Which page?", &page_cards(&ctx, &last_hits)).await
    }
}

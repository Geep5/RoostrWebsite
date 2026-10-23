//! The chat timeline of an object (`src/lib/chat.ts`): `chat` blocks under a
//! conversation root plus mailbox envelopes.
//!
//! Ordered by the message's own timestamp, NOT by block order. Block order is
//! DAG merge order, and a device that commits while its replica is behind
//! lists stale heads as parents - its message then merges in at a position
//! that has nothing to do with when it was written. Ties keep block order, so
//! a same-millisecond pair stays stable.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use roostr_core::{AgentEndpoint, AgentMessage, Block, MailboxEntry, MessageDelivery, Object};

/// The human thread every object has.
pub(crate) const DISCUSSION: &str = "__discussion__";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChatMessage {
    pub id: String,
    pub author: String,
    pub ts: i64,
    pub text: String,
    pub origin: String,
    pub mailbox: Option<MailboxEntry>,
}

fn meta<'a>(block: &'a Block, key: &str) -> &'a str {
    block
        .content
        .custom
        .as_ref()
        .and_then(|c| c.meta.as_ref())
        .and_then(|m| m.get(key))
        .map_or("", String::as_str)
}

/// `Number(meta.ts ?? 0)`.
fn meta_ts(block: &Block) -> i64 {
    let raw = meta(block, "ts").trim();
    if raw.is_empty() {
        return 0;
    }
    raw.parse::<f64>().map_or(0, |n| n as i64)
}

/// The harness scheduler posts with origin "schedule" and the recurring object under `origin_object`.
fn meta_origin(block: &Block) -> String {
    if meta(block, "origin") == "schedule" {
        meta(block, "origin_object").to_string()
    } else {
        meta(block, "origin").to_string()
    }
}

/// One address per object; prefer its agent over its human-only inbox.
pub(crate) fn unique_endpoints<'a>(
    endpoints: impl IntoIterator<Item = &'a AgentEndpoint>,
) -> Vec<AgentEndpoint> {
    let mut out: Vec<AgentEndpoint> = Vec::new();
    for endpoint in endpoints {
        match out.iter_mut().find(|e| e.object_id == endpoint.object_id) {
            None => out.push(endpoint.clone()),
            Some(prior) if prior.agent_id.is_empty() && !endpoint.agent_id.is_empty() => {
                *prior = endpoint.clone()
            }
            Some(_) => {}
        }
    }
    out
}

/// Human replies may address their own object's agent, but never themselves.
pub(crate) fn reply_recipients(
    message: &AgentMessage,
    responder: &AgentEndpoint,
) -> Vec<AgentEndpoint> {
    unique_endpoints(std::iter::once(&message.sender).chain(&message.recipients))
        .into_iter()
        .filter(|e| {
            e.object_id != responder.object_id
                || (responder.agent_id.is_empty() && !e.agent_id.is_empty())
        })
        .collect()
}

fn delivery_priority(status: &str) -> Option<u8> {
    match status {
        "pending" => Some(0),
        "failed" => Some(1),
        "delivered" => Some(2),
        _ => None,
    }
}

fn processing_priority(status: &str) -> Option<u8> {
    match status {
        "pending" => Some(0),
        "awaiting_approval" => Some(1),
        "processing" => Some(2),
        "failed" => Some(3),
        "processed" => Some(4),
        _ => None,
    }
}

/// JS `a > b` over possibly-undefined numbers.
fn greater(a: Option<u8>, b: Option<u8>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a > b)
}

fn compare_entries(a: &MailboxEntry, b: &MailboxEntry) -> Ordering {
    a.message
        .sent_at
        .cmp(&b.message.sent_at)
        .then_with(|| a.message.id.cmp(&b.message.id))
}

/// Duplicate delivery must neither duplicate a bubble nor hide a newer receipt.
pub(crate) fn mailbox_entries(object: &Object, root_id: Option<&str>) -> Vec<MailboxEntry> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, MailboxEntry> = HashMap::new();
    for entry in object.mailbox.iter().flatten() {
        if root_id.is_some_and(|root| entry.thread_id != root) {
            continue;
        }
        let Some(prior) = by_id.get(&entry.message.id) else {
            order.push(entry.message.id.clone());
            by_id.insert(entry.message.id.clone(), entry.clone());
            continue;
        };
        let mut deliveries: Vec<MessageDelivery> = Vec::new();
        for d in &prior.deliveries {
            match deliveries
                .iter_mut()
                .find(|x| x.recipient.object_id == d.recipient.object_id)
            {
                Some(slot) => *slot = d.clone(),
                None => deliveries.push(d.clone()),
            }
        }
        for delivery in &entry.deliveries {
            let held = deliveries
                .iter()
                .position(|d| d.recipient.object_id == delivery.recipient.object_id);
            let take = match held.map(|i| &deliveries[i]) {
                None => true,
                Some(held) => {
                    held.status != "delivered"
                        && (delivery.status == "delivered"
                            || delivery.at > held.at
                            || (delivery.at == held.at
                                && (greater(
                                    delivery_priority(&delivery.status),
                                    delivery_priority(&held.status),
                                ) || (delivery.status == held.status
                                    && delivery.error > held.error))))
                }
            };
            if take {
                match held {
                    Some(i) => deliveries[i] = delivery.clone(),
                    None => deliveries.push(delivery.clone()),
                }
            }
        }
        let (p, e) = (&prior.processing, &entry.processing);
        let processing = if p.status == "processed" {
            p.clone()
        } else if e.status == "processed"
            || e.at > p.at
            || (e.at == p.at
                && (greater(
                    processing_priority(&e.status),
                    processing_priority(&p.status),
                ) || (e.status == p.status && e.error > p.error)))
        {
            e.clone()
        } else {
            p.clone()
        };
        let merged = MailboxEntry {
            incoming: prior.incoming || entry.incoming,
            outgoing: prior.outgoing || entry.outgoing,
            deliveries,
            processing,
            ..prior.clone()
        };
        by_id.insert(entry.message.id.clone(), merged);
    }
    let mut sorted: Vec<MailboxEntry> = order
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect();
    sorted.sort_by(compare_entries);
    let mut children: HashMap<String, Vec<MailboxEntry>> = HashMap::new();
    let mut ready: Vec<MailboxEntry> = Vec::new();
    for entry in &sorted {
        let parent = &entry.message.reply_to;
        if !by_id.contains_key(parent) {
            ready.push(entry.clone());
        } else {
            children
                .entry(parent.clone())
                .or_default()
                .push(entry.clone());
        }
    }
    ready.reverse();
    let mut emitted: HashSet<String> = HashSet::new();
    let mut ordered: Vec<MailboxEntry> = Vec::new();
    // A parent must render before its reply, even with an offline clock.
    // Among available messages, use sentAt/id, never replica block order.
    while let Some(entry) = ready.pop() {
        emitted.insert(entry.message.id.clone());
        for child in children.get(&entry.message.id).into_iter().flatten() {
            let (mut low, mut high) = (0, ready.len());
            while low < high {
                let mid = (low + high) / 2;
                if compare_entries(&ready[mid], child) == Ordering::Greater {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            ready.insert(low, child.clone());
        }
        ordered.push(entry);
    }
    // Malformed cycles cannot satisfy causality, but must not hide history.
    for entry in sorted {
        if !emitted.contains(&entry.message.id) {
            ordered.push(entry);
        }
    }
    ordered
}

/// Legacy shared roots are history only until the mailbox migration completes.
pub(crate) fn is_legacy_exchange(object: &Object, root_id: &str) -> bool {
    let is_exchange = object
        .conversations
        .iter()
        .flatten()
        .any(|c| c.id == root_id && c.kind == "a2a")
        || (root_id == DISCUSSION && !object.field_str("a2a_pair").is_empty());
    is_exchange
        && !object
            .mailbox
            .iter()
            .flatten()
            .any(|e| e.thread_id == root_id)
}

/// Chat blocks under one conversation root, in block (DAG merge) order. The
/// harness also stores tool_use/tool_result/compaction blocks there - only
/// chat messages belong in a timeline.
pub(crate) fn chat_blocks<'a>(object: &'a Object, root_id: &str) -> Vec<&'a Block> {
    let by_id: HashMap<&str, &Block> = object.blocks.iter().map(|b| (b.id.as_str(), b)).collect();
    let Some(root) = by_id.get(root_id) else {
        return Vec::new();
    };
    root.children_ids
        .iter()
        .filter_map(|cid| by_id.get(cid.as_str()).copied())
        .filter(|b| {
            b.content
                .custom
                .as_ref()
                .is_some_and(|c| c.content_type == "chat")
        })
        .collect()
}

fn envelope_message(object: &Object, entry: &MailboxEntry) -> ChatMessage {
    let m = &entry.message;
    ChatMessage {
        id: m.id.clone(),
        author: if m.author.is_empty() {
            m.sender.agent_id.clone()
        } else {
            m.author.clone()
        },
        ts: m.sent_at,
        text: m.text.clone(),
        origin: if m.sender.object_id == object.id {
            String::new()
        } else {
            m.sender.object_id.clone()
        },
        mailbox: Some(entry.clone()),
    }
}

fn block_message(block: &Block) -> ChatMessage {
    ChatMessage {
        id: block.id.clone(),
        author: meta(block, "author").to_string(),
        ts: meta_ts(block),
        text: meta(block, "text").to_string(),
        origin: meta_origin(block),
        mailbox: None,
    }
}

/// One conversation's messages, oldest first by timestamp.
pub(crate) fn chat_messages(object: &Object, root_id: &str) -> Vec<ChatMessage> {
    let entries = mailbox_entries(object, Some(root_id));
    if !entries.is_empty() {
        return entries
            .iter()
            .map(|e| envelope_message(object, e))
            .collect();
    }
    let mut rows: Vec<(usize, ChatMessage)> = chat_blocks(object, root_id)
        .into_iter()
        .map(block_message)
        .enumerate()
        .collect();
    rows.sort_by(|a, b| a.1.ts.cmp(&b.1.ts).then(a.0.cmp(&b.0)));
    rows.into_iter().map(|(_, m)| m).collect()
}

/// The object's ONE chat: the human discussion plus every agent exchange,
/// merged into a single time-ordered timeline. Who a message involved rides on
/// the envelope (recipients), so a reply is "from" the agent and a tag is
/// visible on the message that carried it.
pub(crate) fn object_chat_messages(object: &Object) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // The same author's same text within the same ~10s window: one line
    // written twice (to two places, a moment apart).
    let echo_key = |author: &str, text: &str, ts: i64| {
        (
            author.to_string(),
            text.to_string(),
            (ts as f64 / 10_000.).round() as i64,
        )
    };
    // The window, or the adjacent one.
    let echoes = |shown: &HashSet<(String, String, i64)>, m: &ChatMessage| {
        [m.ts, m.ts - 10_000, m.ts + 10_000]
            .iter()
            .any(|&ts| shown.contains(&echo_key(&m.author, &m.text, ts)))
    };
    // Envelopes (human asks, agent replies) from every thread on the object.
    for entry in mailbox_entries(object, None) {
        let message = envelope_message(object, &entry);
        if seen.insert(message.id.clone()) {
            out.push(message);
        } else if let Some(slot) = out.iter_mut().find(|m| m.id == message.id) {
            *slot = message;
        }
    }
    // An agent's answer is its reply envelope; the plain line it also writes
    // (to its transcript or the discussion) is that same answer again.
    let enveloped: HashSet<(String, String, i64)> = out
        .iter()
        .map(|m| echo_key(&m.author, &m.text, m.ts))
        .collect();
    // Plain chat lines: the human discussion (all of it) and each agent's
    // private transcript (only what that agent wrote - its answers; the
    // harness's framed copies of the question are authored by the asker and
    // already shown as the envelope).
    let mut sources: Vec<(&str, Option<HashSet<&str>>)> = vec![(DISCUSSION, None)];
    for c in object.conversations.iter().flatten() {
        if c.kind == "agent_private" {
            sources.push((
                c.id.as_str(),
                Some(c.participants.iter().map(String::as_str).collect()),
            ));
        }
    }
    // An agent answering in the discussion also writes the same line to its
    // private transcript; show that copy only when the discussion lacks it.
    let mut in_discussion: HashSet<(String, String, i64)> = HashSet::new();
    for (root, authors) in &sources {
        for block in chat_blocks(object, root) {
            if seen.contains(&block.id) {
                continue;
            }
            let message = block_message(block);
            if echoes(&enveloped, &message) {
                continue;
            }
            if let Some(authors) = authors {
                if !authors.contains(message.author.as_str()) || echoes(&in_discussion, &message) {
                    continue;
                }
            } else {
                in_discussion.insert(echo_key(&message.author, &message.text, message.ts));
            }
            seen.insert(message.id.clone());
            out.push(message);
        }
    }
    out.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_prefers_agent() {
        let a = AgentEndpoint {
            object_id: "o".into(),
            agent_id: String::new(),
        };
        let b = AgentEndpoint {
            object_id: "o".into(),
            agent_id: "g".into(),
        };
        assert_eq!(unique_endpoints([&a, &b]), vec![b]);
    }

    fn object(value: serde_json::Value) -> Object {
        serde_json::from_value(value).expect("object fixture")
    }

    fn msg(id: &str, author: &str, ts: i64, text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "childrenIds": [],
            "content": { "custom": { "contentType": "chat", "meta": { "author": author, "ts": ts.to_string(), "text": text } } },
        })
    }

    /// An object whose discussion lists `order` (block merge order).
    fn discussion_with(order: Vec<serde_json::Value>) -> Object {
        let ids: Vec<&serde_json::Value> = order.iter().map(|b| &b["id"]).collect();
        let mut blocks =
            vec![serde_json::json!({ "id": DISCUSSION, "childrenIds": ids, "content": {} })];
        blocks.extend(order.iter().cloned());
        object(
            serde_json::json!({ "id": "task", "typeKey": "task", "fields": {}, "blocks": blocks }),
        )
    }

    fn texts(messages: &[ChatMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.text.as_str()).collect()
    }

    fn ids(messages: &[ChatMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn messages_render_oldest_first_by_timestamp_whatever_the_merge_order() {
        let merged = discussion_with(vec![
            msg("m1", "human", 100, "first"),
            msg("m3", "human", 300, "from the phone"),
            msg("m2", "agent", 200, "answer to first"),
        ]);
        assert_eq!(
            texts(&chat_messages(&merged, DISCUSSION)),
            ["first", "answer to first", "from the phone"]
        );
    }

    #[test]
    fn same_millisecond_messages_keep_a_stable_block_order() {
        let tied = discussion_with(vec![
            msg("a", "human", 100, "one"),
            msg("b", "agent", 100, "two"),
        ]);
        assert_eq!(texts(&chat_messages(&tied, DISCUSSION)), ["one", "two"]);
    }

    #[test]
    fn non_chat_blocks_under_the_discussion_are_not_messages() {
        let tool = serde_json::json!({
            "id": "t1", "childrenIds": [],
            "content": { "custom": { "contentType": "tool_use", "meta": {} } },
        });
        let with_tooling = discussion_with(vec![msg("m1", "human", 100, "asked"), tool]);
        assert_eq!(texts(&chat_messages(&with_tooling, DISCUSSION)), ["asked"]);
    }

    #[test]
    fn a_scheduler_post_reports_the_recurring_object_as_its_origin() {
        let mut post = msg("m1", "scheduler", 100, "due");
        let meta = &mut post["content"]["custom"]["meta"];
        meta["origin"] = "schedule".into();
        meta["origin_object"] = "task-7".into();
        let scheduled = discussion_with(vec![post]);
        assert_eq!(chat_messages(&scheduled, DISCUSSION)[0].origin, "task-7");
    }

    // ── Many conversations in one object ──

    const HUMAN: &str = "816854963a03323d";
    const AGENT: &str = "9b08be05-ed4b-4417-a976-1efead0cb561";

    fn root(id: &str, children: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": id, "childrenIds": children,
            "content": { "custom": { "contentType": "discussion", "meta": {} } },
        })
    }

    /// The human discussion, an A2A thread and a closed private thread.
    fn threaded() -> Object {
        object(serde_json::json!({
            "id": "obj", "typeKey": "note", "fields": {},
            "blocks": [
                root(DISCUSSION, &["m1"]),
                msg("m1", HUMAN, 1000, "what should we charge?"),
                root("__thread__a", &["m2", "m3"]),
                msg("m2", AGENT, 2000, "pulled the sheet"),
                msg("m3", AGENT, 3000, "median is $29"),
                root("__thread__b", &[]),
            ],
            "conversations": [
                { "id": DISCUSSION, "kind": "human", "messageCount": 1 },
                { "id": "__thread__a", "kind": "a2a", "title": "Pricing research", "participants": [AGENT], "createdAt": 1500, "openedBy": HUMAN, "aboutMessageId": "m1", "messageCount": 2 },
                { "id": "__thread__b", "kind": "agent_private", "participants": [AGENT], "createdAt": 500, "openedBy": AGENT, "closed": true },
            ],
            "updatedAt": 3000,
        }))
    }

    fn endpoint(object_id: &str, agent_id: &str) -> AgentEndpoint {
        AgentEndpoint {
            object_id: object_id.into(),
            agent_id: agent_id.into(),
        }
    }

    fn home() -> AgentEndpoint {
        endpoint("obj", "")
    }

    fn home_agent() -> AgentEndpoint {
        endpoint("obj", AGENT)
    }

    fn other() -> AgentEndpoint {
        endpoint("other", "other-agent")
    }

    fn third() -> AgentEndpoint {
        endpoint("third", "third-agent")
    }

    fn delivery(recipient: AgentEndpoint, status: &str, error: &str, at: i64) -> MessageDelivery {
        MessageDelivery {
            recipient,
            status: status.into(),
            error: error.into(),
            at,
        }
    }

    fn processing(
        status: &str,
        owner: &str,
        error: &str,
        at: i64,
    ) -> roostr_core::MessageProcessing {
        roostr_core::MessageProcessing {
            status: status.into(),
            owner: owner.into(),
            error: error.into(),
            at,
        }
    }

    /// A message from the human on `obj` to its agent and `other`, in thread `__thread__<exchange>`.
    fn envelope(id: &str, sent_at: i64, edit: impl FnOnce(&mut AgentMessage)) -> MailboxEntry {
        let mut message = AgentMessage {
            id: id.into(),
            exchange_id: "mail".into(),
            sender: home(),
            recipients: vec![home_agent(), other()],
            text: id.into(),
            sent_at,
            title: "Research group".into(),
            request_reply: true,
            author: HUMAN.into(),
            ..Default::default()
        };
        edit(&mut message);
        MailboxEntry {
            thread_id: format!("__thread__{}", message.exchange_id),
            message,
            incoming: true,
            outgoing: true,
            deliveries: vec![
                delivery(home_agent(), "pending", "", 0),
                delivery(other(), "pending", "", 0),
            ],
            processing: processing("pending", "", "", 0),
        }
    }

    fn with_mailbox(mailbox: Vec<MailboxEntry>) -> Object {
        Object {
            mailbox: Some(mailbox),
            ..threaded()
        }
    }

    #[test]
    fn the_object_counts_its_own_threads_without_the_human_one() {
        use crate::threads::object_thread_count;
        assert_eq!(object_thread_count(&threaded()), 2);
        let bare = Object {
            conversations: None,
            blocks: vec![],
            ..threaded()
        };
        assert_eq!(object_thread_count(&bare), 0);
        let mailed = with_mailbox(vec![envelope("first", 100, |_| {})]);
        assert_eq!(object_thread_count(&mailed), 3);
    }

    #[test]
    fn each_threads_timeline_holds_only_its_own_messages() {
        let object = threaded();
        assert_eq!(
            texts(&chat_messages(&object, DISCUSSION)),
            ["what should we charge?"]
        );
        assert_eq!(
            texts(&chat_messages(&object, "__thread__a")),
            ["pulled the sheet", "median is $29"]
        );
        assert!(chat_messages(&object, "__thread__b").is_empty());
    }

    #[test]
    fn an_agents_plain_copy_of_its_reply_envelope_shows_once() {
        let answer = "median is $29";
        let reply_id = format!("reply:q:{AGENT}");
        let reply = envelope(&reply_id, 1_790_193_115_461, |m| {
            m.sender = home_agent();
            m.recipients = vec![home()];
            m.author = AGENT.into();
            m.text = answer.into();
        });
        // The agent also wrote the answer as plain lines a moment earlier: to
        // its private transcript and to the discussion.
        let written = object(serde_json::json!({
            "id": "obj", "typeKey": "note", "fields": {},
            "blocks": [
                root(DISCUSSION, &["d1"]),
                msg("d1", AGENT, 1_790_193_115_460, answer),
                root("__thread__b", &["p1", "p2"]),
                msg("p1", AGENT, 1_790_193_115_459, answer),
                msg("p2", AGENT, 1_790_193_115_470, "and one more thing"),
            ],
            "conversations": [
                { "id": DISCUSSION, "kind": "human" },
                { "id": "__thread__b", "kind": "agent_private", "participants": [AGENT] },
            ],
        }));
        let shown = object_chat_messages(&Object {
            mailbox: Some(vec![reply]),
            ..written
        });
        assert_eq!(ids(&shown), [reply_id.as_str(), "p2"]);
    }

    #[test]
    fn mailbox_copies_project_one_bubble_per_message_without_leaking_into_private_chat() {
        let first = envelope("first", 100, |_| {});
        let reply = envelope("reply", 200, |m| {
            m.sender = other();
            m.recipients = vec![home_agent()];
            m.reply_to = "first".into();
            m.author = other().agent_id;
        });
        let mailed = with_mailbox(vec![reply.clone(), first, reply]);
        assert_eq!(
            ids(&chat_messages(&mailed, "__thread__mail")),
            ["first", "reply"]
        );
        assert_eq!(ids(&chat_messages(&mailed, DISCUSSION)), ["m1"]);
        assert_eq!(ids(&chat_messages(&mailed, "__thread__a")), ["m2", "m3"]);
        assert!(!is_legacy_exchange(&mailed, "__thread__mail"));
    }

    #[test]
    fn offline_replies_follow_their_parents_and_ties_converge_across_replicas() {
        let parent = envelope("parent", 300, |_| {});
        let reply = envelope("reply", 10, |m| m.reply_to = "parent".into());
        let independent = envelope("independent", 200, |_| {});
        let tie_b = envelope("tie-b", 400, |_| {});
        let tie_a = envelope("tie-a", 400, |_| {});
        let detached_private = envelope("private", 500, |m| {
            m.exchange_id = "private".into();
            m.reply_to = "unavailable-parent".into();
        });
        for entries in [
            vec![
                reply.clone(),
                tie_b.clone(),
                independent.clone(),
                detached_private.clone(),
                tie_a.clone(),
                parent.clone(),
            ],
            vec![
                parent.clone(),
                tie_a.clone(),
                independent.clone(),
                reply.clone(),
                tie_b.clone(),
                detached_private.clone(),
                reply.clone(),
            ],
        ] {
            let mailed = with_mailbox(entries);
            assert_eq!(
                ids(&chat_messages(&mailed, "__thread__mail")),
                ["independent", "parent", "reply", "tie-a", "tie-b"]
            );
            assert_eq!(
                ids(&chat_messages(&mailed, "__thread__private")),
                ["private"]
            );
        }
    }

    #[test]
    fn duplicate_delivery_cannot_regress_completed_receipts_or_replay_processed_work() {
        let pending = envelope("message", 100, |_| {});
        let completed = MailboxEntry {
            deliveries: vec![delivery(other(), "delivered", "", 200)],
            processing: processing("processed", "machine", "", 200),
            ..pending.clone()
        };
        let late_failure = MailboxEntry {
            deliveries: vec![delivery(other(), "failed", "offline", 300)],
            processing: processing("failed", "old-machine", "interrupted", 300),
            ..pending.clone()
        };
        for entries in [
            vec![pending.clone(), completed.clone(), late_failure.clone()],
            vec![late_failure.clone(), pending.clone(), completed.clone()],
        ] {
            let projected = &mailbox_entries(&with_mailbox(entries), None)[0];
            let to_other = projected
                .deliveries
                .iter()
                .find(|d| d.recipient.object_id == "other")
                .expect("receipt for other");
            assert_eq!(to_other.status, "delivered");
            assert_eq!(projected.processing.status, "processed");
        }
    }

    #[test]
    fn failed_delivery_and_processing_stay_until_a_newer_explicit_retry_resets_them() {
        let failed = MailboxEntry {
            deliveries: vec![delivery(other(), "failed", "recipient unavailable", 200)],
            processing: processing("failed", "machine", "interrupted", 200),
            ..envelope("message", 100, |_| {})
        };
        let reset = MailboxEntry {
            deliveries: vec![delivery(other(), "pending", "", 300)],
            processing: processing("pending", "", "", 300),
            ..failed.clone()
        };
        let alone = &mailbox_entries(&with_mailbox(vec![failed.clone()]), None)[0];
        assert_eq!(alone.deliveries[0].status, "failed");
        assert_eq!(alone.processing.status, "failed");
        for entries in [
            vec![failed.clone(), reset.clone()],
            vec![reset.clone(), failed.clone()],
        ] {
            let projected = &mailbox_entries(&with_mailbox(entries), None)[0];
            assert_eq!(projected.deliveries[0].status, "pending");
            assert_eq!(projected.processing.status, "pending");
            assert_eq!(projected.processing.error, "");
        }
    }

    #[test]
    fn reply_all_keeps_the_senders_own_agent_but_excludes_the_responders_object() {
        let message = envelope("message", 100, |m| {
            m.recipients = vec![home_agent(), other(), third()]
        })
        .message;
        assert_eq!(
            reply_recipients(&message, &home()),
            [home_agent(), other(), third()]
        );
        assert_eq!(
            reply_recipients(&message, &other()),
            [home_agent(), third()]
        );
        let human_only = AgentMessage {
            recipients: vec![other(), third()],
            ..message
        };
        assert_eq!(reply_recipients(&human_only, &other()), [home(), third()]);
        assert_eq!(reply_recipients(&human_only, &home()), [other(), third()]);
    }

    #[test]
    fn shared_a2a_history_is_read_only_until_its_own_mailbox_copy_exists() {
        let object = threaded();
        assert!(is_legacy_exchange(&object, "__thread__a"));
        assert!(!is_legacy_exchange(&object, DISCUSSION));
        assert!(!is_legacy_exchange(&object, "__thread__b"));
        let imported = with_mailbox(vec![envelope("imported", 100, |m| {
            m.exchange_id = "a".into();
            m.historical = true;
        })]);
        assert!(!is_legacy_exchange(&imported, "__thread__a"));
        let paired = Object {
            fields: [(
                "a2a_pair".to_string(),
                roostr_core::Value::string("old-pair"),
            )]
            .into(),
            conversations: Some(vec![]),
            ..threaded()
        };
        assert!(is_legacy_exchange(&paired, DISCUSSION));
    }
}

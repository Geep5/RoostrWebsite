//! Agents' live progress in a chat ([`roostr_core::live`]): each run's thinking and tool
//! steps in seq order, shown in a box under the agent's name until the run ends (its `done`
//! or `error`, its reply landing, or [`RUN_IDLE`] without a word), and the record a reply
//! keeps of them (its meta `thinking` / `thought_ms`).

use std::collections::BTreeMap;
use std::time::Duration;

use roostr_core::State;
use roostr_core::live::{LiveEvent, LiveKind};
use roostr_rt::Task;
use serde::Deserialize;

use crate::chat::ChatMessage;
use crate::discussion::Discussions;

/// A run with no event for this long is over (its harness went away, its `done` got lost).
pub(crate) const RUN_IDLE: Duration = Duration::from_secs(120);
/// The elapsed clock's step while a run is live.
const TICK: Duration = Duration::from_secs(1);

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// One piece of an agent's progress: thinking prose or a tool step's one-line summary.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Entry {
    Thinking(String),
    Step(String),
}

/// What a reply keeps of the work behind it: how long it took and what went on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Thought {
    pub ms: u64,
    pub entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct RecordItem {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    text: String,
}

impl Thought {
    /// A reply's meta `thinking` (a JSON array of `{kind, text, ts}`) and `thought_ms`;
    /// `None` when it has neither.
    pub(crate) fn from_meta(thinking: &str, thought_ms: &str) -> Option<Thought> {
        let (thinking, thought_ms) = (thinking.trim(), thought_ms.trim());
        if thinking.is_empty() && thought_ms.is_empty() {
            return None;
        }
        let entries = serde_json::from_str::<Vec<RecordItem>>(thinking)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| match item.kind.as_str() {
                "thinking" if !item.text.trim().is_empty() => Some(Entry::Thinking(item.text)),
                "step" => Some(Entry::Step(item.text.trim().to_string())),
                _ => None,
            })
            .collect();
        let ms = thought_ms.parse::<f64>().map_or(0, |ms| ms.max(0.) as u64);
        Some(Thought { ms, entries })
    }

    pub(crate) fn steps(&self) -> usize {
        steps(&self.entries)
    }

    /// `Thought for 1m 20s · 6 steps`.
    pub(crate) fn summary(&self) -> String {
        let steps = self.steps();
        let time = format!("Thought for {}", duration(self.ms));
        match steps {
            0 => time,
            1 => format!("{time} · 1 step"),
            n => format!("{time} · {n} steps"),
        }
    }
}

pub(crate) fn steps(entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter(|e| matches!(e, Entry::Step(_)))
        .count()
}

/// `42s`, `1m 20s`, `1h 5m`.
pub(crate) fn duration(ms: u64) -> String {
    let s = (ms / 1000).max(1);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s / 60 % 60),
    }
}

/// `0:42`, `12:05`, `1:02:03`.
pub(crate) fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Adds one piece in order: thinking deltas run on into the prose before them.
fn append(entries: &mut Vec<Entry>, kind: LiveKind, text: &str) {
    match kind {
        LiveKind::Thinking => match entries.last_mut() {
            Some(Entry::Thinking(prose)) => prose.push_str(text),
            _ => entries.push(Entry::Thinking(text.to_string())),
        },
        LiveKind::Step => entries.push(Entry::Step(text.trim().to_string())),
        LiveKind::Text | LiveKind::Done | LiveKind::Error => {}
    }
}

/// One agent run as this device heard it.
#[derive(Debug)]
pub(crate) struct Run {
    pub id: String,
    pub agent: String,
    pub object: String,
    pub thread: String,
    /// The first heard event's publisher time: a reply written since ends the run.
    pub since: i64,
    /// Where the elapsed clock starts (local ms).
    pub started: i64,
    /// When this device last heard of it (local ms).
    heard: i64,
    pub ended: bool,
    /// The thinking and step events by seq: a duplicate is dropped, a late one slots in.
    events: BTreeMap<u64, (LiveKind, String)>,
    /// `events` in seq order, consecutive thinking joined.
    pub entries: Vec<Entry>,
}

impl Run {
    fn new(e: &LiveEvent, now: i64) -> Run {
        // The publisher's clock dates the start (a late joiner sees the time already
        // spent), unless it is ahead of this device or implausibly far behind.
        let started = if e.ts > 0 && e.ts <= now && now - e.ts < 10 * 60_000 {
            e.ts
        } else {
            now
        };
        Run {
            id: e.run.clone(),
            agent: e.agent.clone(),
            object: e.object.clone(),
            thread: e.thread.clone(),
            since: if e.ts > 0 { e.ts } else { now },
            started,
            heard: now,
            ended: false,
            events: BTreeMap::new(),
            entries: Vec::new(),
        }
    }

    /// Takes one event; true when what the run shows changed.
    fn push(&mut self, e: &LiveEvent, now: i64) -> bool {
        self.heard = now;
        if self.ended {
            return false;
        }
        match e.kind {
            LiveKind::Done | LiveKind::Error => {
                self.ended = true;
                return true;
            }
            // The reply's own words show as the reply.
            LiveKind::Text => return false,
            LiveKind::Thinking | LiveKind::Step => {}
        }
        if self.events.contains_key(&e.seq) {
            return false;
        }
        let in_order = self
            .events
            .last_key_value()
            .is_none_or(|(&last, _)| last < e.seq);
        self.events.insert(e.seq, (e.kind, e.text.clone()));
        if in_order {
            append(&mut self.entries, e.kind, &e.text);
        } else {
            self.entries.clear();
            for (kind, text) in self.events.values() {
                append(&mut self.entries, *kind, text);
            }
        }
        true
    }

    /// `messages` hold this run's reply: its agent's message with a progress record,
    /// written since the run started.
    fn replied(&self, messages: &[ChatMessage]) -> bool {
        messages
            .iter()
            .any(|m| m.author == self.agent && m.ts >= self.since && m.thought.is_some())
    }

    /// Shown in the chat of `object_id` on `thread` (the object's one chat shows them all).
    pub(crate) fn belongs(&self, object_id: &str, thread: &str) -> bool {
        (self.object == object_id || self.agent == object_id)
            && (thread == crate::chat::DISCUSSION || self.thread == thread)
    }
}

/// Every run heard, live or recently ended (so a late event can't bring one back).
#[derive(Default)]
pub(crate) struct LiveRuns {
    runs: Vec<Run>,
    /// Steps the elapsed clocks and expiry while a run is live.
    tick: Option<Task<()>>,
}

impl LiveRuns {
    /// Takes one event; true when what a run shows changed.
    pub(crate) fn apply(&mut self, e: &LiveEvent, now: i64) -> bool {
        if let Some(run) = self.runs.iter_mut().find(|r| r.id == e.run) {
            return run.push(e, now);
        }
        let mut run = Run::new(e, now);
        run.push(e, now);
        let shows = !run.ended;
        self.runs.push(run);
        shows
    }

    /// Ends the runs silent for [`RUN_IDLE`] and forgets ended ones that long quiet.
    pub(crate) fn expire(&mut self, now: i64) -> bool {
        let idle = RUN_IDLE.as_millis() as i64;
        let mut changed = false;
        for run in &mut self.runs {
            if !run.ended && now - run.heard >= idle {
                run.ended = true;
                changed = true;
            }
        }
        self.runs.retain(|r| !r.ended || now - r.heard < idle);
        changed
    }

    /// Ends `object_id`'s runs whose reply `messages` hold.
    pub(crate) fn end_replied(&mut self, object_id: &str, messages: &[ChatMessage]) -> bool {
        let mut changed = false;
        for run in &mut self.runs {
            if !run.ended && run.object == object_id && run.replied(messages) {
                run.ended = true;
                changed = true;
            }
        }
        changed
    }

    /// The live runs the chat of `object_id` on `thread` shows, first heard first.
    pub(crate) fn shown<'a>(
        &'a self,
        object_id: &'a str,
        thread: &'a str,
    ) -> impl Iterator<Item = &'a Run> {
        self.runs
            .iter()
            .filter(move |r| !r.ended && r.belongs(object_id, thread))
    }

    fn any_live(&self) -> bool {
        self.runs.iter().any(|r| !r.ended)
    }
}

/// `StoreEvent::Live`: the run takes the event; a reply already shown ends it; the clock
/// runs while any run is live.
pub(crate) fn on_event(s: &mut State, e: &LiveEvent) {
    let now = now_ms();
    s.slot::<LiveRuns>().apply(e, now);
    let shown: Vec<_> = s
        .get::<Discussions>()
        .into_iter()
        .flat_map(|d| d.map.values())
        .filter(|d| d.object_id == e.object)
        .map(|d| d.messages.clone())
        .collect();
    let runs = s.slot::<LiveRuns>();
    for messages in shown {
        runs.end_replied(&e.object, &messages);
    }
    ensure_tick(s);
}

/// One tick a second while a run is live: the elapsed clocks move, quiet runs end.
fn ensure_tick(s: &mut State) {
    let runs = s.slot::<LiveRuns>();
    if runs.tick.is_some() || !runs.any_live() {
        return;
    }
    let task = s.rt.spawn(async move |cx| {
        loop {
            cx.timer(TICK).await;
            let more = cx.update(|s| {
                let runs = s.slot::<LiveRuns>();
                runs.expire(now_ms());
                let more = runs.any_live();
                if !more {
                    runs.tick = None;
                }
                more
            });
            if !more {
                return;
            }
        }
    });
    s.slot::<LiveRuns>().tick = Some(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(run: &str, seq: u64, kind: LiveKind, text: &str) -> LiveEvent {
        LiveEvent {
            run: run.into(),
            agent: "agent-1".into(),
            object: "page-1".into(),
            thread: crate::chat::DISCUSSION.into(),
            seq,
            kind,
            text: text.into(),
            ts: 1_000,
        }
    }

    fn shown(runs: &LiveRuns) -> Vec<Vec<Entry>> {
        runs.shown("page-1", crate::chat::DISCUSSION)
            .map(|r| r.entries.clone())
            .collect()
    }

    #[test]
    fn events_show_in_seq_order_and_duplicates_drop() {
        let mut runs = LiveRuns::default();
        assert!(runs.apply(&event("r", 0, LiveKind::Thinking, "Look"), 5_000));
        assert!(runs.apply(
            &event("r", 2, LiveKind::Step, "shell_exec: git status"),
            5_000
        ));
        // Late, out of order: it slots in before the step.
        assert!(runs.apply(&event("r", 1, LiveKind::Thinking, "ing around"), 5_000));
        // Twice: nothing changes.
        assert!(!runs.apply(&event("r", 1, LiveKind::Thinking, "ing around"), 5_000));
        assert!(!runs.apply(
            &event("r", 2, LiveKind::Step, "shell_exec: git status"),
            5_000
        ));
        assert!(runs.apply(&event("r", 3, LiveKind::Thinking, "Done."), 5_000));
        assert_eq!(
            shown(&runs),
            vec![vec![
                Entry::Thinking("Looking around".into()),
                Entry::Step("shell_exec: git status".into()),
                Entry::Thinking("Done.".into()),
            ]]
        );
    }

    #[test]
    fn a_run_ends_on_done_and_stays_ended() {
        let mut runs = LiveRuns::default();
        runs.apply(&event("r", 0, LiveKind::Thinking, "Hm"), 5_000);
        assert_eq!(shown(&runs).len(), 1);
        assert!(runs.apply(&event("r", 1, LiveKind::Done, ""), 5_000));
        assert!(shown(&runs).is_empty(), "done ends the run");
        // A straggler doesn't bring it back.
        runs.apply(&event("r", 0, LiveKind::Step, "late"), 5_100);
        assert!(shown(&runs).is_empty());
        // An error ends a run too.
        runs.apply(&event("e", 0, LiveKind::Step, "x"), 5_000);
        runs.apply(&event("e", 1, LiveKind::Error, "boom"), 5_000);
        assert!(shown(&runs).is_empty());
    }

    #[test]
    fn a_run_ends_when_its_reply_lands_or_it_falls_silent() {
        let mut runs = LiveRuns::default();
        runs.apply(&event("r", 0, LiveKind::Thinking, "Hm"), 5_000);
        let mut reply = ChatMessage {
            id: "m1".into(),
            author: "agent-1".into(),
            ts: 2_000,
            text: "Answer".into(),
            origin: String::new(),
            mailbox: None,
            files: Vec::new(),
            thought: None,
            reactions: Vec::new(),
        };
        assert!(!runs.end_replied("page-1", std::slice::from_ref(&reply)));
        reply.thought = Some(Thought::default());
        assert!(runs.end_replied("page-1", &[reply]));
        assert!(shown(&runs).is_empty());

        runs.apply(&event("q", 0, LiveKind::Thinking, "Hm"), 5_000);
        let idle = RUN_IDLE.as_millis() as i64;
        assert!(!runs.expire(5_000 + idle - 1));
        assert_eq!(shown(&runs).len(), 1);
        assert!(runs.expire(5_000 + idle));
        assert!(shown(&runs).is_empty());
    }

    #[test]
    fn a_record_reads_from_meta_and_sums_up() {
        let thought = Thought::from_meta(
            r#"[{"kind":"thinking","text":"Plan","ts":1},{"kind":"step","text":"read_file: a.rs","ts":2},
               {"kind":"step","text":"shell_exec: ls","ts":3},{"kind":"other","text":"x","ts":4}]"#,
            "80000",
        )
        .expect("a record");
        assert_eq!(thought.entries.len(), 3);
        assert_eq!(thought.summary(), "Thought for 1m 20s · 2 steps");
        let quiet = Thought::from_meta("", "4200").expect("a record");
        assert_eq!(quiet.summary(), "Thought for 4s");
        assert_eq!(Thought::from_meta("", ""), None);
        assert_eq!(clock(42_000), "0:42");
        assert_eq!(clock(3_723_000), "1:02:03");
    }
}

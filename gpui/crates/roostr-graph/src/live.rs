//! This computer's engine and harness, live on the graph: what the local
//! harness says its agents are doing (`GET /agent/status`) and the local
//! engine's record store (`GET /api/records/status`), asked every couple of
//! seconds while the graph is open. Only this computer: nothing is written
//! anywhere, so other computers' engines and harnesses don't show.

use serde_json::Value as Json;

use crate::model::MACHINE_TYPE;
use crate::scene::Built;

/// This computer on the graph: its Computer object, its two lines of state,
/// its agents (where, and which are working) and its working turns.
pub(crate) struct Hub {
    pub at: (f64, f64),
    pub name: String,
    pub lines: [String; 2],
    /// The harness answered.
    pub answering: bool,
    pub agents: Vec<((f64, f64), bool)>,
    /// A working agent → the object its turn is on (both in the graph).
    pub pulses: Vec<((f64, f64), (f64, f64))>,
    /// 0..1 through the pulse cycle.
    pub phase: f64,
}

impl Hub {
    /// `None` when this computer's Computer object isn't in the graph (it
    /// serves none of the space's agents) or isn't known yet.
    pub fn build(b: &Built, local: &Local, phase: f64) -> Option<Hub> {
        if local.machine_id.is_empty() {
            return None;
        }
        let computer =
            b.g.types
                .iter()
                .filter(|t| t.key == MACHINE_TYPE)
                .flat_map(|t| &t.objects)
                .find(|o| {
                    o.fields
                        .get("machine_id")
                        .and_then(|v| v.string_value.as_deref())
                        == Some(&local.machine_id)
                })?;
        let pos = |id: &str| {
            b.obj_index.get(id).map(|&(t, j)| {
                let o = b.object(t, j);
                (o.x, o.y)
            })
        };
        let working: Vec<&Turn> = local.working().collect();
        let agents =
            b.g.types
                .iter()
                .flat_map(|t| &t.objects)
                .filter(|o| o.links.iter().any(|l| *l == computer.id))
                .map(|o| ((o.x, o.y), working.iter().any(|t| t.agent == o.id)))
                .collect();
        let pulses = working
            .iter()
            .filter(|t| t.object != t.agent)
            .filter_map(|t| Some((pos(&t.agent)?, pos(&t.object)?)))
            .collect();
        Some(Hub {
            at: (computer.x, computer.y),
            name: computer.name.clone(),
            lines: [local.engine_line(), local.harness_line()],
            answering: local.turns.is_some(),
            agents,
            pulses,
            phase,
        })
    }
}

/// An agent's turn on this computer, as its harness reports it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Turn {
    pub agent: String,
    pub working: bool,
    pub failed: bool,
    /// The object the turn is on (its surface).
    pub object: String,
}

/// The engine's record store (`/api/records/status`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Engine {
    /// Edits written here, not yet sent to the relay (its journal).
    pub unsent: u64,
}

/// One reading of this computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Local {
    /// This computer's `machine_id` (its Computer object's).
    pub machine_id: String,
    /// `None`: the harness did not answer.
    pub turns: Option<Vec<Turn>>,
    /// `None`: the engine did not answer (or keeps no record store).
    pub engine: Option<Engine>,
}

impl Local {
    pub fn turns_from(status: &Json) -> Vec<Turn> {
        status["agents"]
            .as_array()
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|a| {
                        Some(Turn {
                            agent: a["id"].as_str()?.to_string(),
                            working: a["state"] == "working",
                            failed: a["state"] == "error",
                            object: a["surface"].as_str().unwrap_or_default().to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn engine_from(status: &serde_json::Map<String, Json>) -> Option<Engine> {
        Some(Engine {
            unsent: status.get("journal")?.as_u64()?,
        })
    }

    pub fn working(&self) -> impl Iterator<Item = &Turn> {
        self.turns.iter().flatten().filter(|t| t.working)
    }

    /// "Harness · 2 working · 1 failed", "Harness · idle", "Harness · not answering".
    pub fn harness_line(&self) -> String {
        let Some(turns) = &self.turns else {
            return "Harness · not answering".into();
        };
        let working = turns.iter().filter(|t| t.working).count();
        let failed = turns.iter().filter(|t| t.failed).count();
        let mut parts = vec!["Harness".to_string()];
        if working > 0 {
            parts.push(format!("{working} working"));
        }
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        if parts.len() == 1 {
            parts.push("idle".into());
        }
        parts.join(" · ")
    }

    /// "Engine · all edits sent", "Engine · 3 edits waiting to send", "Engine · not answering".
    pub fn engine_line(&self) -> String {
        match self.engine {
            Some(Engine { unsent: 0 }) => "Engine · all edits sent".into(),
            Some(Engine { unsent: 1 }) => "Engine · 1 edit waiting to send".into(),
            Some(e) => format!("Engine · {} edits waiting to send", thousands(e.unsent)),
            None => "Engine · not answering".into(),
        }
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_harness_line_counts_working_and_failed_agents() {
        let status = json!({ "agents": [
            { "id": "a", "state": "working", "surface": "page-1" },
            { "id": "b", "state": "working", "surface": "page-2" },
            { "id": "c", "state": "error", "surface": "page-3" },
            { "id": "d", "state": "idle", "surface": "" },
        ] });
        let local = Local {
            turns: Some(Local::turns_from(&status)),
            ..Local::default()
        };
        assert_eq!(local.harness_line(), "Harness · 2 working · 1 failed");
        assert_eq!(
            local
                .working()
                .map(|t| t.object.as_str())
                .collect::<Vec<_>>(),
            ["page-1", "page-2"]
        );
        let idle = Local {
            turns: Some(Vec::new()),
            ..Local::default()
        };
        assert_eq!(idle.harness_line(), "Harness · idle");
        assert_eq!(Local::default().harness_line(), "Harness · not answering");
    }

    #[test]
    fn the_engine_line_says_whether_edits_wait_to_be_sent() {
        // `records` is the record-mode flag; `journal` the edits not yet sent.
        let line = |status: serde_json::Value| {
            Local {
                engine: Local::engine_from(status.as_object().unwrap()),
                ..Local::default()
            }
            .engine_line()
        };
        assert_eq!(
            line(json!({ "records": true, "journal": 0 })),
            "Engine · all edits sent"
        );
        assert_eq!(
            line(json!({ "records": true, "journal": 1 })),
            "Engine · 1 edit waiting to send"
        );
        assert_eq!(
            line(json!({ "records": true, "journal": 1204 })),
            "Engine · 1,204 edits waiting to send"
        );
        assert_eq!(Local::default().engine_line(), "Engine · not answering");
    }
}

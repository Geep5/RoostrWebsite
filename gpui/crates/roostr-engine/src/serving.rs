//! `serving` (docs/object-serving.md): which machine serves an object.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Engine, EngineError};

/// The states the serving rule reads.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ServingInputs<'a> {
    /// The object; `None` resolves `unserved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<&'a Value>,
    /// Agent objects its guest list names, any order (the object's list order decides).
    pub agents: &'a [Value],
    /// Every machine object.
    pub machines: &'a [Value],
    /// Skill objects it lists.
    pub skills: &'a [Value],
    /// Capability objects: each machine's status per key.
    pub capabilities: &'a [Value],
}

/// The serving machine, why, the skills it needs and every candidate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Serving {
    pub machine_id: String,
    pub reason: String,
    pub skills: Vec<String>,
    pub candidates: Vec<String>,
}

#[derive(Serialize)]
struct Resolve<'a> {
    action: &'a str,
    #[serde(flatten)]
    inputs: &'a ServingInputs<'a>,
}

impl Engine {
    pub async fn resolve_serving(
        &self,
        inputs: &ServingInputs<'_>,
    ) -> Result<Serving, EngineError> {
        self.call_as(
            "serving",
            &Resolve {
                action: "resolve",
                inputs,
            },
            None,
        )
        .await
    }
}

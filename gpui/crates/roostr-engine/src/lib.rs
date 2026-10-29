//! The shared Odin engine (glonOdin) behind every Roostr replica.
//!
//! Every domain rule of the daemon-less replica - the change codec, replay,
//! state sync, the query cache, the mutation planner, relay wire sealing, the
//! receive session and outbox, shared-space authority and serving - lives in
//! one engine that the Svelte site (`static/engine.wasm`), the iOS app
//! (`Glon.xcframework`) and the daemon all run. This crate binds it for Rust:
//!
//! - **Desktop / iOS** (`not(target_family = "wasm")`): `build.rs` compiles
//!   glonOdin `native/` into a static library with the flags of
//!   `build-xcframework.mjs` and links it; [`Engine::native`] initialises it
//!   once per process.
//! - **Browser** (`wasm32`): [`Engine::load`] fetches the site's
//!   `engine.wasm` and `engine-core.json`, checks the module's sha256 against
//!   the manifest, and instantiates it with the headless Odin runtime of
//!   `src/lib/engine/odin-runtime.ts`.
//!
//! Both speak the same ABI (`glonOdin/abi/abi.odin`): one UTF-8 JSON request
//! `{method, payload}` plus an optional binary blob, answered by a `{result}`
//! or `{error}` envelope. The engine is single-flight: calls are serialised
//! (a process-wide mutex natively; the browser has one thread).
//!
//! Calls are `async` on both targets so callers share one shape, but the work
//! is synchronous: on the desktop the future blocks while it is polled, like
//! `roostr_core`'s HTTP clients, so it belongs on a background executor
//! (`roostr_core::io::spawn`). Futures are `Send` natively and need not be
//! in the browser.
//!
//! Changes cross the boundary in the engine's own JSON form: protobuf maps as
//! ordered `[key, value]` pairs ([`Change`], and [`pack_core_value_maps`] for
//! plain object states), exactly as the iOS host keeps them, so map order -
//! part of a change's content address - survives every round trip.

mod authority;
mod codec;
mod contracts;
pub mod deflate;
mod error;
mod mutation;
pub mod pack;
mod query;
pub mod record;
mod record_decode;
mod replay;
mod serving;
mod session;
mod values;
mod wire;

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

pub use authority::{
    Authorization, AuthorizeSubject, SpaceAuthority, VanishCandidate, VanishedEntry,
};
pub use codec::MessageKind;
pub use contracts::{BlockWire, Change, ObjectHistory, Op, SharedProvenance};
pub use error::EngineError;
pub use mutation::{Applied, MutationPlan, MutationRequest, MutationResponse};
pub use query::{CorpusLoad, QueryResult, QueryUpdate};
pub use replay::{
    BaseBlobOpened, BaseCurrent, BaseDecoded, BaseSummary, Classification, ClassificationBase,
    CompactPlan, EngineRecord, Rebased, RebasedChange, StateInputs,
};
pub use serving::{Serving, ServingInputs};
pub use session::{
    BaseBlob, CheckpointSummary, EnqueueResult, IngestItem, IngestResult, OutboxItem, OutboxNext,
    OutboxPending, RelayEvent, SessionOpen, SessionSpace, SessionState, SettleResult,
};
pub use values::{pack_core_value_maps, unpack_core_value_maps};
#[cfg(target_family = "wasm")]
pub use web::WebSource;
pub use wire::{
    Bech32, SealBlob, SealRequest, SealSpace, Sealed, SealedBase, SealedBaseEvent, SealedBlob,
    SealedPart, Verified,
};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// ABI the binding speaks (`abi.odin ABI_VERSION`; 2 added the blob side-channel).
pub const ABI_VERSION: u32 = 2;
/// Largest encoded JSON request (`abi.odin REQUEST_LIMIT`).
pub const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
/// Largest binary side-channel of one request (`abi.odin BLOB_LIMIT`).
pub const BLOB_LIMIT: usize = 32 * 1024 * 1024;

/// Kind of a NIP-44 self-encrypted change event.
pub const CHANGE_KIND: u16 = 1078;
/// Kind of a legacy sealed checkpoint event (docs/checkpoint-sync.md).
pub const CHECKPOINT_KIND: u16 = 1079;
/// Kind of an object's addressable base event (docs/state-sync.md).
pub const BASE_KIND: u16 = 31078;
/// Kind of a base's parts 2..n.
pub const BASE_PART_KIND: u16 = 1080;

/// A handle on the engine. Cheap to clone; every clone drives the same
/// instance, whose query cache and receive session are shared state.
#[derive(Clone)]
pub struct Engine {
    #[cfg(not(target_family = "wasm"))]
    host: native::Host,
    #[cfg(target_family = "wasm")]
    host: std::rc::Rc<web::Host>,
}

#[derive(Serialize)]
struct Request<'a, P: ?Sized> {
    method: &'a str,
    payload: &'a P,
}

/// The `{result}` / `{error}` envelope. `result` may be JSON `null` (an empty
/// replay), which is distinct from a missing one.
#[derive(Deserialize)]
#[serde(bound(deserialize = "R: Deserialize<'de>"))]
struct Envelope<R> {
    #[serde(default, deserialize_with = "present")]
    result: Option<R>,
    #[serde(default)]
    error: Option<String>,
}

fn present<'de, D, R>(deserializer: D) -> Result<Option<R>, D::Error>
where
    D: Deserializer<'de>,
    R: Deserialize<'de>,
{
    R::deserialize(deserializer).map(Some)
}

/// Decodes a response envelope straight out of the engine's response bytes.
fn decode_envelope<R: DeserializeOwned>(bytes: &[u8]) -> Result<R, EngineError> {
    match serde_json::from_slice::<Envelope<R>>(bytes) {
        Ok(Envelope {
            error: Some(message),
            ..
        }) => Err(EngineError::Domain(message)),
        Ok(Envelope {
            result: Some(result),
            ..
        }) => Ok(result),
        Ok(Envelope { result: None, .. }) => Err(EngineError::Runtime(
            "engine response is missing its result".into(),
        )),
        Err(error) if error.classify() == serde_json::error::Category::Data => {
            Err(EngineError::Decode(error.to_string()))
        }
        Err(error) => Err(EngineError::Runtime(format!(
            "malformed engine response: {error}"
        ))),
    }
}

impl Engine {
    /// The process-wide native engine; initialises the linked library on
    /// first use and checks its ABI version.
    #[cfg(not(target_family = "wasm"))]
    pub fn native() -> Result<Engine, EngineError> {
        Ok(Engine {
            host: native::Host::get()?,
        })
    }

    /// Fetches, verifies and instantiates the browser engine from `source`.
    /// Each call creates a separate instance: keep one per page.
    #[cfg(target_family = "wasm")]
    pub async fn load(source: &WebSource) -> Result<Engine, EngineError> {
        Ok(Engine {
            host: std::rc::Rc::new(web::Host::load(source).await?),
        })
    }

    /// `sourceFingerprint` of the Odin sources this engine was built from:
    /// natively the `build-xcframework.mjs` formula over the linked sources
    /// (equal to iOS `glon-core.json` for the same tree), in the browser the
    /// verified `engine-core.json`. Persisted engine output (replay caches)
    /// is keyed on it.
    pub fn source_fingerprint(&self) -> &str {
        self.host.source_fingerprint()
    }

    /// Bumped whenever the engine dropped its persistent state - a runtime
    /// fault reset it, or [`Engine::reset_all`] ran - so a host knows its
    /// query cache must be reloaded (`core.ts coreGeneration`).
    pub fn generation(&self) -> u64 {
        self.host.generation()
    }

    /// Executes `{method, payload}` with an optional binary side-channel and
    /// returns the envelope's `result`.
    pub async fn call(
        &self,
        method: &str,
        payload: Value,
        blob: Option<&[u8]>,
    ) -> Result<Value, EngineError> {
        self.call_as(method, &payload, blob).await
    }

    /// [`Engine::call`] with a typed payload and result: the payload is
    /// serialised straight into the request and the result decoded straight
    /// out of the response.
    pub async fn call_as<P, R>(
        &self,
        method: &str,
        payload: &P,
        blob: Option<&[u8]>,
    ) -> Result<R, EngineError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.execute(method, payload, blob)
    }

    /// Releases the engine's persistent query cache (`core_reset(1)`); the
    /// next query must reset or reload the corpus.
    pub async fn reset_all(&self) {
        self.host.reset_all();
    }

    fn execute<P, R>(
        &self,
        method: &str,
        payload: &P,
        blob: Option<&[u8]>,
    ) -> Result<R, EngineError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let request = serde_json::to_vec(&Request { method, payload })
            .map_err(|error| EngineError::Encode(error.to_string()))?;
        if request.len() > REQUEST_LIMIT {
            return Err(EngineError::RequestTooLarge(request.len()));
        }
        let blob = blob.filter(|bytes| !bytes.is_empty());
        if let Some(bytes) = blob
            && bytes.len() > BLOB_LIMIT
        {
            return Err(EngineError::BlobTooLarge(bytes.len()));
        }
        self.host.execute(&request, blob, decode_envelope::<R>)
    }
}

//! `query`, `corpus` and the committed-change cache apply (`query.ts`).
//!
//! The engine keeps queried objects resident, so a query carries only what
//! changed since the last one; which objects those are, and when the engine's
//! cache needs a cold reload ([`Engine::generation`]), is the host's to track.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::values::Packed;
use crate::{Change, Engine, EngineError, ObjectHistory};

/// Upsert batches stay well under the 16 MiB request reservation (`query.ts PUSH_BUDGET_BYTES`).
const PUSH_BUDGET_BYTES: usize = 6 * 1024 * 1024;
/// Objects per corpus batch (`query.ts CORPUS_BATCH_OBJECTS`).
const CORPUS_BATCH_OBJECTS: usize = 4000;
/// Framed bytes per corpus batch, under the 32 MiB blob (`query.ts CORPUS_BATCH_BYTES`).
const CORPUS_BATCH_BYTES: usize = 24 * 1024 * 1024;
/// Length-word bit marking a Checkpoint frame (`core/corpus.odin CORPUS_CHECKPOINT_FLAG`).
const CORPUS_CHECKPOINT_FLAG: u32 = 0x8000_0000;

/// One cache update, optionally followed by a query (`/api/query` body).
#[derive(Debug, Clone, Copy)]
pub struct QueryUpdate<'a> {
    /// Object states that changed, as replay returned them (packed on the way in).
    pub upserts: &'a [Value],
    /// Ids that no longer exist.
    pub removed: &'a [String],
    /// Replace the whole cache (a cold load) instead of applying a delta.
    pub reset: bool,
    /// The query to run afterwards; `None` only updates the cache (what a
    /// mutation does before it plans over it).
    pub body: Option<&'a Value>,
    /// Unix ms the query's relative dates resolve against.
    pub now_ms: i64,
}

/// `/api/query` result: the total match count and the requested page. A
/// cache update without a query body answers neither (both default empty).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QueryResult {
    #[serde(default)]
    pub total: i64,
    /// `{id, typeKey, name, fields, createdAt, updatedAt, deleted?, snippet?}` rows.
    #[serde(default)]
    pub records: Vec<Value>,
}

/// What a corpus load did, summed across its batches (`query.ts CorpusLoad`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct CorpusLoad {
    /// Objects this load replayed.
    pub objects: u64,
    pub changes: u64,
    pub bytes: u64,
    /// Undecodable changes, skipped and counted rather than hidden.
    pub skipped: u64,
    /// Objects whose history could not be replayed (a cycle), left out of the cache.
    #[serde(default)]
    pub unreplayable: u64,
    /// Objects the engine holds afterwards: the authoritative total.
    pub cached: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryRequest<'a> {
    upserts: Vec<Packed<'a>>,
    removed: &'a [String],
    reset: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a Value>,
    now_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ObjectsQuery<'a> {
    objects: Vec<Packed<'a>>,
    body: &'a Value,
    now_ms: i64,
}

#[derive(Serialize)]
struct ApplyRequest<'a> {
    action: &'a str,
    changes: &'a [Change],
}

#[derive(Serialize)]
struct CorpusRequest {
    action: &'static str,
    reset: bool,
}

/// `apply`: which committed changes the cache took and which it refused.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Applied {
    pub applied: Vec<String>,
    pub refused: Vec<String>,
}

/// Counts serialised bytes without keeping them.
struct Counter(usize);

impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn packed_len(value: &Value) -> usize {
    let mut counter = Counter(0);
    // Writing to a counter cannot fail; a Value always serialises.
    let _ = serde_json::to_writer(&mut counter, &Packed::new(value));
    counter.0
}

impl Engine {
    /// Applies a cache update and runs the query, split into batches that
    /// each fit the request reservation (`query.ts pushAndQuery`): removals
    /// and the reset ride on the first batch only, the query on every one,
    /// and the last batch's result is returned. A refused batch leaves the
    /// earlier ones applied: the host must then reset.
    pub async fn query(&self, update: QueryUpdate<'_>) -> Result<QueryResult, EngineError> {
        let mut batches: Vec<&[Value]> = Vec::new();
        let mut start = 0;
        let mut size = 0;
        for (index, state) in update.upserts.iter().enumerate() {
            let bytes = packed_len(state);
            if index > start && size + bytes > PUSH_BUDGET_BYTES {
                batches.push(&update.upserts[start..index]);
                start = index;
                size = 0;
            }
            size += bytes;
        }
        batches.push(&update.upserts[start..]);
        let mut result = QueryResult {
            total: 0,
            records: Vec::new(),
        };
        for (index, batch) in batches.into_iter().enumerate() {
            let first = index == 0;
            let request = QueryRequest {
                upserts: batch.iter().map(Packed::new).collect(),
                removed: if first { update.removed } else { &[] },
                reset: first && update.reset,
                body: update.body,
                now_ms: update.now_ms,
            };
            result = self.call_as("query", &request, None).await?;
        }
        Ok(result)
    }

    /// Runs a query over exactly `objects`, without touching the cache.
    pub async fn query_objects(
        &self,
        objects: &[Value],
        body: &Value,
        now_ms: i64,
    ) -> Result<QueryResult, EngineError> {
        let request = ObjectsQuery {
            objects: objects.iter().map(Packed::new).collect(),
            body,
            now_ms,
        };
        self.call_as("query", &request, None).await
    }

    /// Applies locally committed, addressed changes to the cache in order
    /// (`query_cache_apply`) instead of re-sending their objects' states. The
    /// engine applies a change only on exactly the heads it caches; a refused
    /// one leaves its object untouched, and the host upserts its state.
    pub async fn apply_committed(&self, changes: &[Change]) -> Result<Applied, EngineError> {
        self.call_as(
            "mutation",
            &ApplyRequest {
                action: "apply",
                changes,
            },
            None,
        )
        .await
    }

    /// Loads a vault into the cache straight from stored protobuf
    /// (`query.ts loadCorpus`): length-prefixed frames, a checkpoint's
    /// length word flagged, at most 4000 objects / 24 MiB per batch. Each
    /// history must hold every change of one distinct object; one larger than
    /// a batch fails with [`EngineError::BlobTooLarge`] rather than replaying
    /// a prefix. `reset` replaces the cache (an empty reset load clears it).
    pub async fn load_corpus<'a>(
        &self,
        histories: impl IntoIterator<Item = &'a ObjectHistory>,
        reset: bool,
        now_ms: i64,
    ) -> Result<CorpusLoad, EngineError> {
        let mut out = CorpusLoad::default();
        let mut batch: Vec<&'a ObjectHistory> = Vec::new();
        let mut size = 0;
        let mut first = reset;
        for history in histories {
            if history.changes.is_empty() && history.checkpoint.is_none() {
                continue;
            }
            let bytes = framed_len(history);
            if bytes > CORPUS_BATCH_BYTES {
                return Err(EngineError::BlobTooLarge(bytes));
            }
            if batch.len() >= CORPUS_BATCH_OBJECTS || size + bytes > CORPUS_BATCH_BYTES {
                self.push_corpus(&batch, size, first, &mut out).await?;
                first = false;
                batch.clear();
                size = 0;
            }
            batch.push(history);
            size += bytes;
        }
        if !batch.is_empty() {
            self.push_corpus(&batch, size, first, &mut out).await?;
            first = false;
        }
        if first {
            // An empty reset load must clear the previous corpus too.
            let body = serde_json::json!({"limit": 1});
            self.query(QueryUpdate {
                upserts: &[],
                removed: &[],
                reset: true,
                body: Some(&body),
                now_ms,
            })
            .await?;
        }
        Ok(out)
    }

    async fn push_corpus(
        &self,
        histories: &[&ObjectHistory],
        size: usize,
        reset: bool,
        out: &mut CorpusLoad,
    ) -> Result<(), EngineError> {
        let mut blob = Vec::with_capacity(size);
        for history in histories {
            if let Some(checkpoint) = &history.checkpoint {
                push_frame(&mut blob, checkpoint, CORPUS_CHECKPOINT_FLAG);
            }
            for change in &history.changes {
                push_frame(&mut blob, change, 0);
            }
        }
        let part: CorpusLoad = self
            .call_as(
                "corpus",
                &CorpusRequest {
                    action: "push",
                    reset,
                },
                Some(&blob),
            )
            .await?;
        out.objects += part.objects;
        out.changes += part.changes;
        out.bytes += part.bytes;
        out.skipped += part.skipped;
        out.unreplayable += part.unreplayable;
        out.cached = part.cached;
        Ok(())
    }
}

fn framed_len(history: &ObjectHistory) -> usize {
    history
        .checkpoint
        .as_ref()
        .map_or(0, |checkpoint| 4 + checkpoint.len())
        + history
            .changes
            .iter()
            .map(|change| 4 + change.len())
            .sum::<usize>()
}

fn push_frame(blob: &mut Vec<u8>, bytes: &[u8], flag: u32) {
    // Bounded by CORPUS_BATCH_BYTES, so the length fits below the flag bit.
    blob.extend_from_slice(&(bytes.len() as u32 | flag).to_le_bytes());
    blob.extend_from_slice(bytes);
}

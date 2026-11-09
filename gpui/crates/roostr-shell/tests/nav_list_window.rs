//! A pinned list in the nav builds only the rows in and near the window, and
//! scrolling the nav builds the rest as they come near. Field report: opening
//! a page's Properties column lagged - every click redraws the whole window,
//! and the nav laid out all of its pinned list's rows (up to 500) each time.
#![cfg(not(target_family = "wasm"))]

mod common;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use roostr_core::types::{
    Object, ObjectSummary, QueryResult, QueryRow, RelationDef, Settings, Space, Value,
};
use roostr_core::{Backend, BackendEvent, JsonMap};
use roostr_rt::{BoxFuture, BoxStream};
use serde_json::Value as Json;

use common::Harness;

const ROWS: usize = 300;
const FIRST: &str = "w-row-agent-0";
const MIDDLE: &str = "w-row-agent-150";
const LAST: &str = "w-row-agent-299";

/// One space with one pinned query listing `ROWS` agents.
struct Vault;

fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
    Box::pin(std::future::ready(value))
}

fn row(n: usize) -> QueryRow {
    QueryRow {
        id: format!("agent-{n}"),
        type_key: "agent".into(),
        name: Some(format!("Agent {n}")),
        ..QueryRow::default()
    }
}

impl Backend for Vault {
    fn describe(&self) -> String {
        "one long pinned query".into()
    }
    fn settings(&self) -> BoxFuture<Result<Settings>> {
        ready(Ok(Settings {
            has_key: true,
            ..Settings::default()
        }))
    }
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        ready(Ok(vec![ObjectSummary {
            id: "q-1".into(),
            type_key: "query".into(),
            name: "Agents".into(),
            channel_id: "sp-1".into(),
            updated_at: 1_000,
            ..ObjectSummary::default()
        }]))
    }
    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        ready(if id == "q-1" {
            Ok(Object {
                id: id.into(),
                type_key: "query".into(),
                fields: [
                    ("name".to_string(), Value::string("Agents")),
                    ("channel".to_string(), Value::string("sp-1")),
                ]
                .into(),
                ..Object::default()
            })
        } else {
            Err(anyhow::anyhow!("unknown object {id}"))
        })
    }
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        ready(Ok(vec![Space {
            id: "sp-1".into(),
            name: "Personal".into(),
            pinned_ids: vec!["q-1".into()],
            created_at: 1,
            ..Space::default()
        }]))
    }
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        ready(Ok(Vec::new()))
    }
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        if body["setId"] != "q-1" {
            return ready(Ok(QueryResult::default()));
        }
        let records: Vec<QueryRow> = (0..ROWS).map(row).collect();
        ready(Ok(QueryResult {
            total: records.len() as u64,
            records,
        }))
    }
    fn mutate(&self, _: &str, _: JsonMap) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn serving(&self, _: &[String]) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        ready(Ok(JsonMap::new()))
    }
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        Box::pin(futures::stream::pending())
    }
}

/// Deliver what the backend sent, then run past the store's refresh throttle.
fn settle(h: &mut Harness) {
    h.settle();
    h.advance(Duration::from_secs(2));
}

#[test]
fn the_navs_pinned_list_builds_only_the_rows_near_the_window() {
    let mut h = Harness::new(Arc::new(Vault), (1280., 800.));
    settle(&mut h);
    settle(&mut h);

    assert!(h.has(FIRST), "the list's first row");
    assert!(
        !h.has(LAST),
        "the last of {ROWS} rows, far below the window, isn't built"
    );
    assert!(!h.has(MIDDLE), "nor is row 150");

    // Scrolling the nav to its end builds the rows that come into view.
    // Positive wheel deltas move content down: scroll with a negative one, as GPUI did.
    h.wheel(FIRST, -(ROWS as f32) * 30.);
    settle(&mut h);
    assert!(h.has(LAST), "scrolled to the end, the last row is built");
    assert!(!h.has(FIRST), "and the first, far above now, is not");
}

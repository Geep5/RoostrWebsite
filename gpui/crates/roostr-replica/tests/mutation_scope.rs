//! A mutation request carries no states: the planner reads the engine's
//! query cache, which the host brings up to date right before planning
//! (`Inner::plan`). An object the params name that the cache lacks comes
//! back missing and is planned again once with the host's state of it as
//! overlay; ids the host does not hold go as absent. Every plan must equal
//! the one planned over the caller's complete snapshot.

#![cfg(not(target_family = "wasm"))]

mod support;

use std::collections::HashMap;

use roostr_engine::{
    Change, Engine, MutationPlan, MutationRequest, MutationResponse, Op, QueryUpdate, StateInputs,
};
use serde_json::{Value, json};
use support::{ENGINE_TURN, block_on};

const AUTHOR: &str = "scope-test";

fn request<'a>(
    action: &'a str,
    params: &'a Value,
    seed: &'a str,
    timestamp: i64,
) -> MutationRequest<'a> {
    MutationRequest {
        action,
        params,
        timestamp,
        author: AUTHOR,
        id_seed: seed,
        key_id: 0,
        objects: None,
        overlay: &[],
        absent: &[],
    }
}

fn plan_of(response: MutationResponse) -> MutationPlan {
    match response {
        MutationResponse::Plan(plan) => plan,
        MutationResponse::Missing(ids) => panic!("planner wanted {ids:?}"),
    }
}

/// The reference answer: the plan over exactly `objects`.
async fn snapshot_plan(
    engine: &Engine,
    req: MutationRequest<'_>,
    objects: &[Value],
) -> MutationPlan {
    plan_of(
        engine
            .mutation(&MutationRequest {
                objects: Some(objects),
                ..req
            })
            .await
            .unwrap(),
    )
}

/// Brings the engine's cache up to date with `states` (no query).
async fn sync_cache(engine: &Engine, states: &[Value]) {
    engine
        .query(QueryUpdate {
            upserts: states,
            removed: &[],
            reset: false,
            body: None,
            now_ms: 0,
        })
        .await
        .unwrap();
}

/// Plans over the cache; on `missing`, retries once with `fetch`'s states
/// of those ids as overlay and the rest absent (`Inner::plan`). Returns the
/// plan and every id list fetched.
async fn plan_over_cache(
    engine: &Engine,
    req: MutationRequest<'_>,
    fetch: impl Fn(&[String]) -> Vec<Value>,
) -> (MutationPlan, Vec<Vec<String>>) {
    let missing = match engine.mutation(&req).await.unwrap() {
        MutationResponse::Plan(plan) => return (plan, Vec::new()),
        MutationResponse::Missing(ids) => ids,
    };
    let overlay = fetch(&missing);
    let absent: Vec<String> = missing
        .iter()
        .filter(|id| !overlay.iter().any(|s| s["id"] == id.as_str()))
        .cloned()
        .collect();
    let retried = engine
        .mutation(&MutationRequest {
            overlay: &overlay,
            absent: &absent,
            ..req
        })
        .await
        .unwrap();
    (plan_of(retried), vec![missing])
}

/// Planned changes committed into per-object histories, as a host commits them.
#[derive(Default)]
struct Histories {
    by_id: HashMap<String, Vec<Change>>,
}

impl Histories {
    /// Returns the ids of the objects `plan` touched.
    async fn commit(&mut self, engine: &Engine, plan: &MutationPlan) -> Vec<String> {
        let mut touched = Vec::new();
        for change in &plan.changes {
            let mut change = change.clone();
            let history = self.by_id.entry(change.object_id.clone()).or_default();
            change.parent_ids = history.last().map(|c| c.id.clone()).into_iter().collect();
            change.id = engine.change_id(&change).await.unwrap();
            if !touched.contains(&change.object_id) {
                touched.push(change.object_id.clone());
            }
            history.push(change);
        }
        touched
    }

    async fn state(&self, engine: &Engine, id: &str) -> Value {
        engine
            .replay_from_base(&StateInputs {
                changes: self.by_id[id].clone(),
                ..StateInputs::default()
            })
            .await
            .unwrap()
            .unwrap()
    }
}

fn text_block(state: &Value) -> &Value {
    state["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["content"].get("text").is_some())
        .unwrap_or_else(|| panic!("no text block in {state}"))
}

fn block_add(target: &Value) -> Value {
    json!({
        "object_id": target["id"],
        "block": { "content": { "text": { "text": "second" } } },
        "target_id": text_block(target)["id"],
        "position": 2,
    })
}

/// A note with one text block, replayed from the planner's own changes.
async fn note(engine: &Engine, histories: &mut Histories, name: &str, seed: &str) -> Value {
    let params = json!({ "name": name });
    let created = snapshot_plan(engine, request("create", &params, seed, 1_000), &[]).await;
    let id = created.result["id"].as_str().unwrap().to_string();
    histories.commit(engine, &created).await;
    let params = json!({
        "object_id": id,
        "block": { "content": { "text": { "text": "first" } } },
        "target_id": "",
        "position": 0,
    });
    let state = histories.state(engine, &id).await;
    let block_seed = format!("{seed}-b");
    let added = snapshot_plan(
        engine,
        request("block_add", &params, &block_seed, 2_000),
        &[state],
    )
    .await;
    histories.commit(engine, &added).await;
    histories.state(engine, &id).await
}

#[test]
fn a_mutation_plans_over_the_cache_with_no_states_shipped() {
    let _turn = ENGINE_TURN.lock();
    let engine = Engine::native().unwrap();
    block_on(async {
        engine.reset_all().await;
        let mut histories = Histories::default();
        let target = note(&engine, &mut histories, "Target", "target").await;
        let big = note(&engine, &mut histories, "Big", "big").await;
        let template = text_block(&big).clone();
        let filler = "lorem ipsum ".repeat(25);
        // ~10 MB of blocks: the object that used to overrun the per-request arena.
        let mut huge = big.clone();
        let blocks = huge["blocks"].as_array_mut().unwrap();
        for i in 0..25_000 {
            let mut block = template.clone();
            block["id"] = json!(format!("filler-{i}"));
            block["content"] = json!({ "text": { "text": format!("{i} {filler}") } });
            blocks.push(block);
        }
        sync_cache(&engine, &[target.clone(), huge.clone()]).await;

        let params = block_add(&target);
        let req = request("block_add", &params, "seed", 1_000);
        let (plan, fetched) = plan_over_cache(&engine, req, |_| Vec::new()).await;
        assert!(fetched.is_empty(), "nothing fetched: {fetched:?}");
        assert!(!plan.changes.is_empty());
        assert_eq!(plan, snapshot_plan(&engine, req, &[target, huge]).await);
    });
}

#[test]
fn an_object_the_cache_lacks_is_fetched_and_planned_again_once() {
    let _turn = ENGINE_TURN.lock();
    let engine = Engine::native().unwrap();
    block_on(async {
        engine.reset_all().await;
        let mut histories = Histories::default();
        let target = note(&engine, &mut histories, "Uncached", "uncached").await;
        let target_id = target["id"].as_str().unwrap().to_string();
        let params = block_add(&target);
        let req = request("block_add", &params, "seed-missing", 1_000);
        let (plan, fetched) = plan_over_cache(&engine, req, |ids| {
            if ids.contains(&target_id) {
                vec![target.clone()]
            } else {
                Vec::new()
            }
        })
        .await;
        assert_eq!(fetched, vec![vec![target_id.clone()]]);
        assert_eq!(
            plan,
            snapshot_plan(&engine, req, std::slice::from_ref(&target)).await
        );

        // An id no replica holds plans as the snapshot without it would.
        let params = json!({ "object_id": "ghost-object", "key": "status", "value": { "stringValue": "done" } });
        let ghost = request("set_field", &params, "seed-ghost", 1_000);
        let (plan, fetched) = plan_over_cache(&engine, ghost, |_| Vec::new()).await;
        assert_eq!(fetched, vec![vec!["ghost-object".to_string()]]);
        assert_eq!(plan, snapshot_plan(&engine, ghost, &[]).await);
    });
}

#[test]
fn consecutive_mutations_see_each_other_through_the_cache() {
    let _turn = ENGINE_TURN.lock();
    let engine = Engine::native().unwrap();
    block_on(async {
        engine.reset_all().await;
        let mut histories = Histories::default();
        let mut fetched = Vec::new();
        let params = json!({ "type_key": "channel", "name": "Home" });
        let (space, f) =
            plan_over_cache(&engine, request("create", &params, "space", 1_000), |_| {
                Vec::new()
            })
            .await;
        fetched.extend(f);
        let space_id = space.result["id"].as_str().unwrap().to_string();
        // What the backend's dirty set holds between a commit and the next plan.
        let pending = histories.commit(&engine, &space).await;
        let mut states = Vec::new();
        for id in &pending {
            states.push(histories.state(&engine, id).await);
        }
        sync_cache(&engine, &states).await;

        let params = json!({ "name": "Fresh" });
        let (created, f) =
            plan_over_cache(&engine, request("create", &params, "fresh", 2_000), |_| {
                Vec::new()
            })
            .await;
        fetched.extend(f);
        let id = created.result["id"].as_str().unwrap().to_string();
        // The note lands in the space created just before it: a cache scan.
        let channel = created
            .changes
            .iter()
            .filter(|c| c.object_id == id)
            .flat_map(|c| &c.ops)
            .find_map(|op| match op {
                Op::FieldSet { key, value } if key == "channel" => value.clone(),
                _ => None,
            });
        assert_eq!(channel, Some(json!({ "stringValue": space_id })));
        let pending = histories.commit(&engine, &created).await;
        let mut states = Vec::new();
        for id in &pending {
            states.push(histories.state(&engine, id).await);
        }
        sync_cache(&engine, &states).await;

        let params = json!({
            "object_id": id,
            "block": { "content": { "text": { "text": "hi" } } },
            "target_id": "",
            "position": 0,
        });
        let (added, f) = plan_over_cache(
            &engine,
            request("block_add", &params, "fresh-b", 3_000),
            |_| Vec::new(),
        )
        .await;
        fetched.extend(f);
        assert!(fetched.is_empty(), "nothing fetched: {fetched:?}");
        let touched: Vec<&str> = added.changes.iter().map(|c| c.object_id.as_str()).collect();
        assert_eq!(touched, [id.as_str()]);
    });
}

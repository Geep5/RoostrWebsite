//! Browser backend: raw IndexedDB through web-sys, the schema shape of the
//! Svelte site's `engine/store.ts` in its own database (default
//! [`IdbChangeStore::DEFAULT_NAME`]).
//!
//! Every request is awaited through a [`Request`] future whose handlers wake
//! the task; wasm-bindgen-futures polls woken tasks in a microtask, so a
//! read-then-write inside one transaction keeps it active. Every operation
//! runs under the [`IDB_TIMEOUT_MS`] guard: WebKit can leave a request
//! pending with no event at all, which then surfaces as
//! [`StoreError::Unavailable`] instead of a future that never settles.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::FutureExt as _;
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{
    Event, IdbCursor, IdbCursorDirection, IdbDatabase, IdbFactory, IdbKeyRange, IdbObjectStore,
    IdbObjectStoreParameters, IdbOpenDbRequest, IdbRequest, IdbTransaction, IdbTransactionMode,
};

use crate::*;

pub mod svelte_import;

/// Bound on any one store operation (`store.ts` `IDB_TIMEOUT_MS`).
pub const IDB_TIMEOUT_MS: u32 = 12_000;

const DB_VERSION: u32 = 1;
const CHANGES: &str = "changes";
const CHECKPOINTS: &str = "checkpoints";
const META: &str = "meta";
const STATES: &str = "states";
const RELAY_EVENTS: &str = "relay-events";
const BASES: &str = "bases";
const BASE_CURRENT: &str = "base-current";
const ALL_STORES: [&str; 7] = [
    CHANGES,
    CHECKPOINTS,
    META,
    STATES,
    RELAY_EVENTS,
    BASES,
    BASE_CURRENT,
];

type Handler = Closure<dyn FnMut(Event)>;

// ------------------------------------------------------------- futures

#[derive(Default)]
struct Slot {
    outcome: Option<Result<JsValue, JsValue>>,
    waker: Option<Waker>,
}

fn settle(slot: &Rc<RefCell<Slot>>, outcome: Result<JsValue, JsValue>) {
    let mut slot = slot.borrow_mut();
    slot.outcome = Some(outcome);
    if let Some(waker) = slot.waker.take() {
        waker.wake();
    }
}

fn poll_slot(slot: &Rc<RefCell<Slot>>, cx: &mut Context<'_>) -> Poll<Result<JsValue, JsValue>> {
    let mut slot = slot.borrow_mut();
    match slot.outcome.take() {
        Some(outcome) => Poll::Ready(outcome),
        None => {
            slot.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// One `IDBRequest`, awaitable repeatedly (a cursor re-fires on `continue`).
/// Its handlers live exactly as long as this value.
struct Request {
    request: IdbRequest,
    slot: Rc<RefCell<Slot>>,
    _success: Handler,
    _error: Handler,
}

impl Request {
    fn new(request: IdbRequest) -> Self {
        let slot = Rc::new(RefCell::new(Slot::default()));
        let success = {
            let (slot, request) = (slot.clone(), request.clone());
            Handler::new(move |_: Event| settle(&slot, request.result()))
        };
        let error = {
            let (slot, request) = (slot.clone(), request.clone());
            Handler::new(move |event: Event| {
                // Handled here: a failed request must not abort its transaction by itself.
                event.prevent_default();
                let error = request
                    .error()
                    .ok()
                    .flatten()
                    .map(JsValue::from)
                    .unwrap_or_else(|| "request failed".into());
                settle(&slot, Err(error));
            })
        };
        request.set_onsuccess(Some(success.as_ref().unchecked_ref()));
        request.set_onerror(Some(error.as_ref().unchecked_ref()));
        Self {
            request,
            slot,
            _success: success,
            _error: error,
        }
    }

    /// The next success (or error) of the request.
    async fn next(&mut self) -> Result<JsValue, StoreError> {
        std::future::poll_fn(|cx| poll_slot(&self.slot, cx))
            .await
            .map_err(js_error)
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.request.set_onsuccess(None);
        self.request.set_onerror(None);
    }
}

/// Awaits a single request.
async fn req(request: Result<IdbRequest, JsValue>) -> Result<JsValue, StoreError> {
    Request::new(request.map_err(js_error)?).next().await
}

/// Resolves when `tx` commits; errors when it fails or aborts.
async fn done(tx: &IdbTransaction) -> Result<(), StoreError> {
    let slot = Rc::new(RefCell::new(Slot::default()));
    let complete = {
        let slot = slot.clone();
        Handler::new(move |_: Event| settle(&slot, Ok(JsValue::UNDEFINED)))
    };
    let failed = {
        let (slot, tx) = (slot.clone(), tx.clone());
        Handler::new(move |_: Event| {
            let error = tx
                .error()
                .map(JsValue::from)
                .unwrap_or_else(|| "transaction aborted".into());
            settle(&slot, Err(error));
        })
    };
    tx.set_oncomplete(Some(complete.as_ref().unchecked_ref()));
    tx.set_onerror(Some(failed.as_ref().unchecked_ref()));
    tx.set_onabort(Some(failed.as_ref().unchecked_ref()));
    let outcome = std::future::poll_fn(|cx| poll_slot(&slot, cx)).await;
    tx.set_oncomplete(None);
    tx.set_onerror(None);
    tx.set_onabort(None);
    outcome.map(drop).map_err(js_error)
}

/// `setTimeout` on whichever global scope runs us (window or worker).
struct Timer {
    id: JsValue,
    slot: Rc<RefCell<Slot>>,
    _fire: Closure<dyn FnMut()>,
}

impl Timer {
    fn new(ms: u32) -> Result<Self, StoreError> {
        let slot = Rc::new(RefCell::new(Slot::default()));
        let fire = {
            let slot = slot.clone();
            Closure::<dyn FnMut()>::new(move || settle(&slot, Ok(JsValue::UNDEFINED)))
        };
        let set: js_sys::Function = global_fn("setTimeout")?;
        let id = set
            .call2(&js_sys::global(), fire.as_ref(), &JsValue::from(ms))
            .map_err(js_error)?;
        Ok(Self {
            id,
            slot,
            _fire: fire,
        })
    }
}

impl Future for Timer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        poll_slot(&self.slot, cx).map(drop)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Ok(clear) = global_fn("clearTimeout") {
            let _ = clear.call1(&js_sys::global(), &self.id);
        }
    }
}

fn global_fn(name: &str) -> Result<js_sys::Function, StoreError> {
    Reflect::get(&js_sys::global(), &name.into())
        .ok()
        .and_then(|f| f.dyn_into().ok())
        .ok_or_else(|| StoreError::Unavailable(format!("no {name} in this context")))
}

/// Runs `body` under the [`IDB_TIMEOUT_MS`] guard.
///
/// The body runs as its own `spawn_local` task, whatever executor polls the
/// returned future: an IndexedDB transaction commits as soon as control
/// returns to the event loop with no request pending, so a body that issues
/// one request after another must resume on a microtask
/// (wasm-bindgen-futures' queue). Polled straight from an executor that
/// wakes on a macrotask (GPUI's), the second request found its transaction
/// finished (`TransactionInactiveError`).
fn guarded<T: 'static>(
    what: &'static str,
    body: impl Future<Output = Result<T, StoreError>> + 'static,
) -> StoreFuture<'static, T> {
    Box::pin(async move {
        let timer = Timer::new(IDB_TIMEOUT_MS)?;
        let (tx, rx) = futures::channel::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(body.await);
        });
        let body = async move { rx.await.unwrap_or(Err(StoreError::Closed)) };
        futures::select_biased! {
            result = body.fuse() => result,
            () = timer.fuse() => Err(StoreError::Unavailable(format!("local storage stopped responding ({what}); reload the page"))),
        }
    })
}

fn js_error(value: JsValue) -> StoreError {
    let message = value
        .dyn_ref::<web_sys::DomException>()
        .map(|e| format!("{}: {}", e.name(), e.message()))
        .or_else(|| {
            value
                .dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .or_else(|| value.as_string())
        .unwrap_or_else(|| format!("{value:?}"));
    StoreError::Backend(message)
}

// ------------------------------------------------------------- values

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let out = Object::new();
    for (key, value) in fields {
        if !value.is_undefined() {
            let _ = Reflect::set(&out, &(*key).into(), value);
        }
    }
    out.into()
}

fn field(value: &JsValue, key: &str) -> JsValue {
    Reflect::get(value, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn text(value: &JsValue, key: &str) -> Result<String, StoreError> {
    field(value, key)
        .as_string()
        .ok_or_else(|| StoreError::Corrupt(format!("{key} is not a string")))
}

fn opt_text(value: &JsValue, key: &str) -> Option<String> {
    field(value, key).as_string()
}

fn number(value: &JsValue, key: &str) -> Result<i64, StoreError> {
    field(value, key)
        .as_f64()
        .map(|n| n as i64)
        .ok_or_else(|| StoreError::Corrupt(format!("{key} is not a number")))
}

fn bytes(value: &JsValue, key: &str) -> Result<Vec<u8>, StoreError> {
    field(value, key)
        .dyn_into::<Uint8Array>()
        .map(|array| array.to_vec())
        .map_err(|_| StoreError::Corrupt(format!("{key} is not bytes")))
}

fn opt(value: &Option<String>) -> JsValue {
    value
        .as_deref()
        .map(JsValue::from)
        .unwrap_or(JsValue::UNDEFINED)
}

fn key2(a: impl Into<JsValue>, b: impl Into<JsValue>) -> JsValue {
    Array::of2(&a.into(), &b.into()).into()
}

/// Every compound key `[first, *]`: one-element arrays sort first, and an
/// array sorts after every number and string.
fn prefix(first: &str) -> Result<JsValue, StoreError> {
    IdbKeyRange::bound(&Array::of1(&first.into()), &key2(first, Array::new()))
        .map(Into::into)
        .map_err(js_error)
}

fn array(value: JsValue) -> Vec<JsValue> {
    value
        .dyn_into::<Array>()
        .map(|a| a.to_vec())
        .unwrap_or_default()
}

fn change_value(record: &ChangeRecord, seq: i64) -> JsValue {
    object(&[
        ("objectId", record.object_id.as_str().into()),
        ("seq", (seq as f64).into()),
        ("bytes", Uint8Array::from(record.bytes.as_slice()).into()),
        ("json", record.json.as_str().into()),
        ("b", opt(&record.b)),
        ("rebased", record.rebased.into()),
    ])
}

fn change_from(id: String, value: &JsValue) -> Result<ChangeRecord, StoreError> {
    Ok(ChangeRecord {
        id,
        object_id: text(value, "objectId")?,
        bytes: bytes(value, "bytes")?,
        json: text(value, "json")?,
        b: opt_text(value, "b"),
        rebased: field(value, "rebased").is_truthy(),
    })
}

fn checkpoint_value(record: &CheckpointRecord) -> JsValue {
    let heads: Array = record
        .heads
        .iter()
        .map(|head| JsValue::from(head.as_str()))
        .collect();
    object(&[
        ("objectId", record.object_id.as_str().into()),
        ("bytes", Uint8Array::from(record.bytes.as_slice()).into()),
        ("hash", record.hash.as_str().into()),
        ("heads", heads.into()),
        ("covered", (record.covered as f64).into()),
    ])
}

fn checkpoint_from(value: &JsValue) -> Result<CheckpointRecord, StoreError> {
    Ok(CheckpointRecord {
        object_id: text(value, "objectId")?,
        bytes: bytes(value, "bytes")?,
        hash: text(value, "hash")?,
        heads: array(field(value, "heads"))
            .iter()
            .filter_map(JsValue::as_string)
            .collect(),
        covered: number(value, "covered")? as u64,
    })
}

fn base_value(record: &BaseRecord) -> JsValue {
    object(&[
        ("objectId", record.object_id.as_str().into()),
        ("hash", record.hash.as_str().into()),
        ("epoch", (record.epoch as f64).into()),
        ("createdAt", (record.created_at as f64).into()),
        ("prevBase", record.prev_base.as_str().into()),
        ("bytes", Uint8Array::from(record.bytes.as_slice()).into()),
    ])
}

fn base_from(value: &JsValue) -> Result<BaseRecord, StoreError> {
    Ok(BaseRecord {
        object_id: text(value, "objectId")?,
        hash: text(value, "hash")?,
        epoch: number(value, "epoch")?,
        created_at: number(value, "createdAt")?,
        prev_base: opt_text(value, "prevBase").unwrap_or_default(),
        bytes: bytes(value, "bytes")?,
    })
}

fn current_from(value: &JsValue) -> Result<CurrentBase, StoreError> {
    Ok(CurrentBase {
        hash: text(value, "hash")?,
        created_at: number(value, "createdAt")?,
    })
}

fn sort_bases(bases: &mut [BaseRecord]) {
    bases.sort_by(|a, b| (a.epoch, a.created_at, &a.hash).cmp(&(b.epoch, b.created_at, &b.hash)));
}

// ------------------------------------------------------------- store

/// [`ChangeStore`] over IndexedDB. Object stores (version 1):
/// `changes` (key: change id → `{objectId, seq, bytes, json, b?, rebased}`,
/// indexes `objectId`, `objectSeq` = `[objectId, seq]`, `seq`),
/// `checkpoints` (key: objectId → `{objectId, bytes, hash, heads, covered}`),
/// `meta` (key → string), `states` (key: objectId → replay memo),
/// `relay-events` (keyPath `[scope, id]` → `{scope, id, createdAt, kind, b?}`,
/// indexes `scopeKind`, `scopeB`), `bases` (keyPath `[objectId, hash]`) and
/// `base-current` (key: objectId → `{hash, createdAt}`).
#[derive(Clone)]
pub struct IdbChangeStore {
    db: Rc<RefCell<Option<IdbDatabase>>>,
    _on_version_change: Rc<Handler>,
}

impl IdbChangeStore {
    /// The Rust replica's database; distinct from the Svelte site's `roostr`.
    pub const DEFAULT_NAME: &'static str = "roostr-replica";

    /// Opens (creating or upgrading) database `name`. Another connection
    /// deleting or upgrading it (a logout in another tab) closes this one:
    /// later calls fail with [`StoreError::Closed`].
    pub async fn open(name: &str) -> Result<Self, StoreError> {
        let name = name.to_owned();
        guarded("open", async move {
            let request: IdbOpenDbRequest = factory()?.open_with_u32(&name, DB_VERSION).map_err(unavailable)?;
            let upgrade = {
                let request = request.clone();
                Handler::new(move |_: Event| {
                    // A failed upgrade aborts its versionchange transaction: the open then fails.
                    if upgrade(&request).is_err()
                        && let Some(tx) = request.transaction()
                    {
                        let _ = tx.abort();
                    }
                })
            };
            let blocked_slot = Rc::new(RefCell::new(Slot::default()));
            let blocked = {
                let slot = blocked_slot.clone();
                Handler::new(move |_: Event| settle(&slot, Err(JsValue::UNDEFINED)))
            };
            request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
            request.set_onblocked(Some(blocked.as_ref().unchecked_ref()));
            let mut opening = Request::new(request.clone().unchecked_into());
            let blocked_wait = std::future::poll_fn(|cx| poll_slot(&blocked_slot, cx));
            let opened = futures::select_biased! {
                db = opening.next().fuse() => db.map_err(|e| StoreError::Unavailable(e.to_string())),
                _ = blocked_wait.fuse() => Err(StoreError::Unavailable(
                    "another Roostr tab is holding an older local database; close it (or reload every Roostr tab) and try again".into(),
                )),
            };
            request.set_onupgradeneeded(None);
            request.set_onblocked(None);
            let db: IdbDatabase = opened?.unchecked_into();
            let cell = Rc::new(RefCell::new(Some(db.clone())));
            let on_version_change = {
                let cell = cell.clone();
                Handler::new(move |_: Event| {
                    if let Some(db) = cell.borrow_mut().take() {
                        db.close();
                    }
                })
            };
            db.set_onversionchange(Some(on_version_change.as_ref().unchecked_ref()));
            Ok(Self { db: cell, _on_version_change: Rc::new(on_version_change) })
        })
        .await
    }

    /// Deletes database `name` (logout / identity switch). Close this
    /// origin's connections first; a still-open one in another tab closes
    /// itself on the `versionchange` this fires.
    pub async fn destroy(name: &str) -> Result<(), StoreError> {
        let name = name.to_owned();
        guarded("destroy", async move {
            let request = factory()?.delete_database(&name).map_err(unavailable)?;
            Request::new(request.unchecked_into())
                .next()
                .await
                .map(drop)
        })
        .await
    }

    fn tx(&self, stores: &[&str], mode: IdbTransactionMode) -> Result<IdbTransaction, StoreError> {
        let db = self.db.borrow();
        let db = db.as_ref().ok_or(StoreError::Closed)?;
        let names: Array = stores.iter().map(|name| JsValue::from(*name)).collect();
        db.transaction_with_str_sequence_and_mode(&names, mode)
            .map_err(js_error)
    }

    fn read(&self, stores: &[&str]) -> Result<IdbTransaction, StoreError> {
        self.tx(stores, IdbTransactionMode::Readonly)
    }

    fn write(&self, stores: &[&str]) -> Result<IdbTransaction, StoreError> {
        self.tx(stores, IdbTransactionMode::Readwrite)
    }

    async fn get_meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        let store = os(&self.read(&[META])?, META)?;
        Ok(req(store.get(&key.into())).await?.as_string())
    }

    async fn put_meta(&self, key: &str, value: Option<&str>) -> Result<(), StoreError> {
        let tx = self.write(&[META])?;
        let store = os(&tx, META)?;
        match value {
            Some(value) => store.put_with_key(&value.into(), &key.into()),
            None => store.delete(&key.into()),
        }
        .map_err(js_error)?;
        done(&tx).await
    }

    /// Distinct object ids holding changes, by their first change's insertion.
    async fn change_object_ids(&self) -> Result<Vec<String>, StoreError> {
        let tx = self.read(&[CHANGES])?;
        let changes = os(&tx, CHANGES)?;
        // WebKit throws on a nextunique cursor over an empty index (dexie#1030).
        if req(changes.count()).await?.as_f64().unwrap_or(0.0) == 0.0 {
            return Ok(Vec::new());
        }
        let by_object = changes.index("objectId").map_err(js_error)?;
        let by_seq = changes.index("objectSeq").map_err(js_error)?;
        let mut cursor = Request::new(
            by_object
                .open_key_cursor_with_range_and_direction(
                    &JsValue::NULL,
                    IdbCursorDirection::Nextunique,
                )
                .map_err(js_error)?,
        );
        let mut ids = Vec::new();
        while let Ok(at) = cursor.next().await?.dyn_into::<IdbCursor>() {
            ids.push(at.key().map_err(js_error)?.as_string().unwrap_or_default());
            at.continue_().map_err(js_error)?;
        }
        let mut firsts = Vec::with_capacity(ids.len());
        for id in ids {
            let first = req(by_seq.get(&prefix(&id)?)).await?;
            firsts.push((number(&first, "seq")?, id));
        }
        firsts.sort();
        Ok(firsts.into_iter().map(|(_, id)| id).collect())
    }

    async fn keys_of(&self, name: &str) -> Result<Vec<String>, StoreError> {
        let store = os(&self.read(&[name])?, name)?;
        Ok(array(req(store.get_all_keys()).await?)
            .iter()
            .filter_map(JsValue::as_string)
            .collect())
    }

    async fn insert_changes(
        store: &IdbObjectStore,
        records: &[ChangeRecord],
    ) -> Result<usize, StoreError> {
        let newest = Request::new(
            store
                .index("seq")
                .map_err(js_error)?
                .open_key_cursor_with_range_and_direction(&JsValue::NULL, IdbCursorDirection::Prev)
                .map_err(js_error)?,
        )
        .next()
        .await?;
        let mut seq = match newest.dyn_into::<IdbCursor>() {
            Ok(cursor) => cursor.key().map_err(js_error)?.as_f64().unwrap_or(0.0) as i64,
            Err(_) => 0,
        };
        let mut seen = HashSet::new();
        let mut inserted = 0;
        for record in records {
            if !seen.insert(record.id.as_str())
                || !req(store.get_key(&record.id.as_str().into()))
                    .await?
                    .is_undefined()
            {
                continue;
            }
            seq += 1;
            store
                .put_with_key(&change_value(record, seq), &record.id.as_str().into())
                .map_err(js_error)?;
            inserted += 1;
        }
        Ok(inserted)
    }

    async fn changes_in(
        changes: &IdbObjectStore,
        object_id: &str,
    ) -> Result<Vec<ChangeRecord>, StoreError> {
        let index = changes.index("objectSeq").map_err(js_error)?;
        let range = prefix(object_id)?;
        let keys = array(req(index.get_all_keys_with_key(&range)).await?);
        let values = array(req(index.get_all_with_key(&range)).await?);
        keys.into_iter()
            .zip(values)
            .map(|(id, value)| change_from(id.as_string().unwrap_or_default(), &value))
            .collect()
    }

    async fn bases_in(
        bases: &IdbObjectStore,
        object_id: &str,
    ) -> Result<Vec<BaseRecord>, StoreError> {
        let mut out = array(req(bases.get_all_with_key(&prefix(object_id)?)).await?)
            .iter()
            .map(base_from)
            .collect::<Result<Vec<_>, _>>()?;
        sort_bases(&mut out);
        Ok(out)
    }
}

fn os(tx: &IdbTransaction, name: &str) -> Result<IdbObjectStore, StoreError> {
    tx.object_store(name).map_err(js_error)
}

fn unavailable(error: JsValue) -> StoreError {
    StoreError::Unavailable(js_error(error).to_string())
}

fn factory() -> Result<IdbFactory, StoreError> {
    let global = js_sys::global();
    let factory = if let Some(window) = global.dyn_ref::<web_sys::Window>() {
        window.indexed_db()
    } else if let Some(worker) = global.dyn_ref::<web_sys::WorkerGlobalScope>() {
        worker.indexed_db()
    } else {
        Ok(None)
    };
    factory
        .map_err(unavailable)?
        .ok_or_else(|| StoreError::Unavailable("IndexedDB is not available here".into()))
}

/// `onupgradeneeded`: creates whatever this version lacks, keeping every row.
fn upgrade(request: &IdbOpenDbRequest) -> Result<(), StoreError> {
    let db: IdbDatabase = request.result().map_err(js_error)?.unchecked_into();
    let names = db.object_store_names();
    let keyed = |name: &str, key_path: &[&str]| -> Result<IdbObjectStore, StoreError> {
        let parameters = IdbObjectStoreParameters::new();
        let path: Array = key_path.iter().map(|key| JsValue::from(*key)).collect();
        parameters.set_key_path(&path);
        db.create_object_store_with_optional_parameters(name, &parameters)
            .map_err(js_error)
    };
    let pair = |a: &str, b: &str| -> JsValue { key2(a, b) };
    if !names.contains(CHANGES) {
        let changes = db.create_object_store(CHANGES).map_err(js_error)?;
        changes
            .create_index_with_str("objectId", "objectId")
            .map_err(js_error)?;
        changes
            .create_index_with_str_sequence("objectSeq", &pair("objectId", "seq"))
            .map_err(js_error)?;
        changes
            .create_index_with_str("seq", "seq")
            .map_err(js_error)?;
    }
    for plain in [CHECKPOINTS, META, STATES, BASE_CURRENT] {
        if !names.contains(plain) {
            db.create_object_store(plain).map_err(js_error)?;
        }
    }
    if !names.contains(RELAY_EVENTS) {
        let events = keyed(RELAY_EVENTS, &["scope", "id"])?;
        events
            .create_index_with_str_sequence("scopeKind", &pair("scope", "kind"))
            .map_err(js_error)?;
        events
            .create_index_with_str_sequence("scopeB", &pair("scope", "b"))
            .map_err(js_error)?;
    }
    if !names.contains(BASES) {
        keyed(BASES, &["objectId", "hash"])?;
    }
    Ok(())
}

impl ChangeStore for IdbChangeStore {
    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize> {
        let this = self.clone();
        guarded("write", async move {
            if records.is_empty() {
                return Ok(0);
            }
            let tx = this.write(&[CHANGES])?;
            let inserted = Self::insert_changes(&os(&tx, CHANGES)?, &records).await?;
            done(&tx).await?;
            Ok(inserted)
        })
    }

    fn add_local_change(
        &self,
        record: ChangeRecord,
        pending: PendingPublish,
    ) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[CHANGES, META])?;
            Self::insert_changes(&os(&tx, CHANGES)?, std::slice::from_ref(&record)).await?;
            let key = format!("{}{}", keys::PENDING, pending.key);
            os(&tx, META)?
                .put_with_key(&encode_pending(&pending)?.into(), &key.into())
                .map_err(js_error)?;
            done(&tx).await
        })
    }

    fn add_rebased(&self, orphan_id: String, record: ChangeRecord) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[CHANGES])?;
            let changes = os(&tx, CHANGES)?;
            Self::insert_changes(&changes, std::slice::from_ref(&record)).await?;
            mark(&changes, &orphan_id).await?;
            done(&tx).await
        })
    }

    fn mark_rebased(&self, ids: Vec<String>) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            if ids.is_empty() {
                return Ok(());
            }
            let tx = this.write(&[CHANGES])?;
            let changes = os(&tx, CHANGES)?;
            for id in &ids {
                mark(&changes, id).await?;
            }
            done(&tx).await
        })
    }

    fn changes_for(&self, object_id: String) -> StoreFuture<'_, Vec<ChangeRecord>> {
        let this = self.clone();
        guarded("read", async move {
            Self::changes_in(&os(&this.read(&[CHANGES])?, CHANGES)?, &object_id).await
        })
    }

    fn object_ids(&self) -> StoreFuture<'_, Vec<String>> {
        let this = self.clone();
        guarded("read", async move {
            let mut ids = this.change_object_ids().await?;
            let mut seen: HashSet<String> = ids.iter().cloned().collect();
            let mut rest: Vec<String> = this.keys_of(CHECKPOINTS).await?;
            rest.extend(this.keys_of(BASE_CURRENT).await?);
            rest.sort();
            rest.retain(|id| seen.insert(id.clone()));
            ids.extend(rest);
            Ok(ids)
        })
    }

    fn change_counts(&self) -> StoreFuture<'_, HashMap<String, u64>> {
        let this = self.clone();
        guarded("read", async move {
            let ids = this.change_object_ids().await?;
            let index = os(&this.read(&[CHANGES])?, CHANGES)?
                .index("objectId")
                .map_err(js_error)?;
            let mut counts = HashMap::new();
            for id in ids {
                let count = req(index.count_with_key(&id.as_str().into()))
                    .await?
                    .as_f64()
                    .unwrap_or(0.0) as u64;
                counts.insert(id, count);
            }
            Ok(counts)
        })
    }

    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs> {
        let this = self.clone();
        guarded("read", async move {
            let tx = this.read(&[CHANGES, CHECKPOINTS, BASES, BASE_CURRENT])?;
            let all = Self::changes_in(&os(&tx, CHANGES)?, &object_id).await?;
            let bases = Self::bases_in(&os(&tx, BASES)?, &object_id).await?;
            let checkpoint = req(os(&tx, CHECKPOINTS)?.get(&object_id.as_str().into())).await?;
            let current = req(os(&tx, BASE_CURRENT)?.get(&object_id.as_str().into())).await?;
            Ok(ObjectInputs {
                count: all.len() as u64,
                changes: all.into_iter().filter(|change| !change.rebased).collect(),
                bases,
                checkpoint: if checkpoint.is_undefined() {
                    None
                } else {
                    Some(checkpoint_from(&checkpoint)?)
                },
                current_base: if current.is_undefined() {
                    None
                } else {
                    Some(current_from(&current)?)
                },
            })
        })
    }

    fn all_change_histories(&self) -> StoreFuture<'_, Vec<ObjectHistory>> {
        let this = self.clone();
        guarded("read", async move {
            let tx = this.read(&[CHANGES, CHECKPOINTS])?;
            let rows =
                array(req(os(&tx, CHANGES)?.index("seq").map_err(js_error)?.get_all()).await?);
            let checkpoints = array(req(os(&tx, CHECKPOINTS)?.get_all()).await?);
            let mut histories: Vec<ObjectHistory> = Vec::new();
            let mut index: HashMap<String, usize> = HashMap::new();
            let mut slot = |histories: &mut Vec<ObjectHistory>, object_id: String| {
                *index.entry(object_id.clone()).or_insert_with(|| {
                    histories.push(ObjectHistory {
                        object_id,
                        ..Default::default()
                    });
                    histories.len() - 1
                })
            };
            for row in &rows {
                let at = slot(&mut histories, text(row, "objectId")?);
                histories[at].changes.push(bytes(row, "bytes")?);
            }
            let mut held = checkpoints
                .iter()
                .map(checkpoint_from)
                .collect::<Result<Vec<_>, _>>()?;
            held.sort_by(|a, b| a.object_id.cmp(&b.object_id));
            for checkpoint in held {
                let at = slot(&mut histories, checkpoint.object_id);
                histories[at].checkpoint = Some(checkpoint.bytes);
            }
            Ok(histories)
        })
    }

    fn put_base(&self, record: BaseRecord, streams: Vec<String>) -> StoreFuture<'_, bool> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[BASES, BASE_CURRENT, META])?;
            let (bases, meta) = (os(&tx, BASES)?, os(&tx, META)?);
            let raw = req(meta.get(&keys::BASE_STREAMS.into())).await?.as_string();
            let mut by_stream = decode_streams(raw.as_deref())?;
            if advance_streams(&mut by_stream, &streams, record.created_at) {
                let encoded = serde_json::to_string(&by_stream)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                meta.put_with_key(&encoded.into(), &keys::BASE_STREAMS.into())
                    .map_err(js_error)?;
            }
            let key = key2(record.object_id.as_str(), record.hash.as_str());
            if !req(bases.get_key(&key)).await?.is_undefined() {
                done(&tx).await?;
                return Ok(false);
            }
            bases.put(&base_value(&record)).map_err(js_error)?;
            let mut held = Self::bases_in(&bases, &record.object_id).await?;
            let newest = held
                .iter()
                .map(|base| base.epoch)
                .max()
                .unwrap_or(record.epoch);
            for base in held
                .iter()
                .filter(|base| base.epoch <= newest - BASE_CHAIN_KEPT)
            {
                bases
                    .delete(&key2(base.object_id.as_str(), base.hash.as_str()))
                    .map_err(js_error)?;
            }
            held.retain(|base| base.epoch > newest - BASE_CHAIN_KEPT);
            if let Some(current) = BaseRecord::current(&held) {
                let value = object(&[
                    ("hash", current.hash.as_str().into()),
                    ("createdAt", (current.created_at as f64).into()),
                ]);
                os(&tx, BASE_CURRENT)?
                    .put_with_key(&value, &record.object_id.as_str().into())
                    .map_err(js_error)?;
            }
            done(&tx).await?;
            Ok(true)
        })
    }

    fn bases(&self, object_id: String) -> StoreFuture<'_, Vec<BaseRecord>> {
        let this = self.clone();
        guarded("read", async move {
            Self::bases_in(&os(&this.read(&[BASES])?, BASES)?, &object_id).await
        })
    }

    fn current_base(&self, object_id: String) -> StoreFuture<'_, Option<CurrentBase>> {
        let this = self.clone();
        guarded("read", async move {
            let value = req(
                os(&this.read(&[BASE_CURRENT])?, BASE_CURRENT)?.get(&object_id.as_str().into())
            )
            .await?;
            if value.is_undefined() {
                Ok(None)
            } else {
                current_from(&value).map(Some)
            }
        })
    }

    fn current_bases(&self) -> StoreFuture<'_, HashMap<String, CurrentBase>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[BASE_CURRENT])?, BASE_CURRENT)?;
            let keys = array(req(store.get_all_keys()).await?);
            let values = array(req(store.get_all()).await?);
            keys.into_iter()
                .zip(values)
                .map(|(key, value)| {
                    Ok((key.as_string().unwrap_or_default(), current_from(&value)?))
                })
                .collect()
        })
    }

    fn current_base_hashes(&self) -> StoreFuture<'_, Vec<String>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[BASE_CURRENT])?, BASE_CURRENT)?;
            let mut hashes = array(req(store.get_all()).await?)
                .iter()
                .map(|v| text(v, "hash"))
                .collect::<Result<Vec<_>, _>>()?;
            hashes.sort();
            Ok(hashes)
        })
    }

    fn objects_without_base(&self) -> StoreFuture<'_, Vec<String>> {
        let this = self.clone();
        guarded("read", async move {
            let with_base: HashSet<String> =
                this.keys_of(BASE_CURRENT).await?.into_iter().collect();
            let mut ids: Vec<String> = this
                .change_object_ids()
                .await?
                .into_iter()
                .filter(|id| !with_base.contains(id))
                .collect();
            ids.sort();
            Ok(ids)
        })
    }

    fn newest_base_by_stream(&self) -> StoreFuture<'_, HashMap<String, i64>> {
        let this = self.clone();
        guarded("read", async move {
            decode_streams(this.get_meta(keys::BASE_STREAMS).await?.as_deref())
        })
    }

    fn checkpoint(&self, object_id: String) -> StoreFuture<'_, Option<CheckpointRecord>> {
        let this = self.clone();
        guarded("read", async move {
            let value =
                req(os(&this.read(&[CHECKPOINTS])?, CHECKPOINTS)?.get(&object_id.as_str().into()))
                    .await?;
            if value.is_undefined() {
                Ok(None)
            } else {
                checkpoint_from(&value).map(Some)
            }
        })
    }

    fn put_checkpoint(
        &self,
        engine: &roostr_engine::Engine,
        record: CheckpointRecord,
    ) -> StoreFuture<'_, bool> {
        let this = self.clone();
        let engine = engine.clone();
        guarded("write", async move {
            let tx = this.write(&[CHECKPOINTS])?;
            let store = os(&tx, CHECKPOINTS)?;
            let held = req(store.get(&record.object_id.as_str().into())).await?;
            // The engine call completes without yielding, so the transaction stays active.
            if !held.is_undefined() && !record.supersedes(&checkpoint_from(&held)?, &engine).await?
            {
                done(&tx).await?;
                return Ok(false);
            }
            store
                .put_with_key(
                    &checkpoint_value(&record),
                    &record.object_id.as_str().into(),
                )
                .map_err(js_error)?;
            done(&tx).await?;
            Ok(true)
        })
    }

    fn all_checkpoints(&self) -> StoreFuture<'_, HashMap<String, CheckpointRecord>> {
        let this = self.clone();
        guarded("read", async move {
            let rows = array(req(os(&this.read(&[CHECKPOINTS])?, CHECKPOINTS)?.get_all()).await?);
            rows.iter()
                .map(|row| checkpoint_from(row).map(|record| (record.object_id.clone(), record)))
                .collect()
        })
    }

    fn forget_checkpoint_floors(&self) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[META])?;
            let meta = os(&tx, META)?;
            if let Some(raw) = req(meta.get(&keys::CHECKPOINT_FLOORS.into()))
                .await?
                .as_string()
            {
                meta.delete(&keys::CHECKPOINT_FLOORS.into())
                    .map_err(js_error)?;
                if floors_shortened_walk(&raw) {
                    meta.delete(&keys::BOOTSTRAPPED.into()).map_err(js_error)?;
                }
            }
            done(&tx).await
        })
    }

    fn meta(&self, key: String) -> StoreFuture<'_, Option<String>> {
        let this = self.clone();
        guarded("read", async move { this.get_meta(&key).await })
    }

    fn set_meta(&self, key: String, value: Option<String>) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            this.put_meta(&key, value.as_deref()).await
        })
    }

    fn cursor(&self) -> StoreFuture<'_, i64> {
        let this = self.clone();
        guarded("read", async move {
            Ok(this
                .get_meta(keys::CURSOR)
                .await?
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(0))
        })
    }

    fn set_cursor(
        &self,
        cursor: i64,
        replay_groups: Option<Vec<(String, i64)>>,
    ) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[META])?;
            let meta = os(&tx, META)?;
            meta.put_with_key(&cursor.to_string().into(), &keys::CURSOR.into())
                .map_err(js_error)?;
            if let Some(groups) = replay_groups {
                meta.put_with_key(&encode_groups(&groups).into(), &keys::REPLAY_GROUPS.into())
                    .map_err(js_error)?;
            }
            done(&tx).await
        })
    }

    fn replay_groups(&self) -> StoreFuture<'_, Vec<(String, i64)>> {
        let this = self.clone();
        guarded("read", async move {
            match this.get_meta(keys::REPLAY_GROUPS).await? {
                Some(raw) => decode_groups(&raw),
                None => Ok(Vec::new()),
            }
        })
    }

    fn bootstrapped(&self) -> StoreFuture<'_, bool> {
        let this = self.clone();
        guarded("read", async move {
            Ok(this.get_meta(keys::BOOTSTRAPPED).await?.as_deref() == Some("1"))
        })
    }

    fn set_bootstrapped(&self) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            this.put_meta(keys::BOOTSTRAPPED, Some("1")).await
        })
    }

    fn bootstrap_floor(&self) -> StoreFuture<'_, Option<i64>> {
        let this = self.clone();
        guarded("read", async move {
            Ok(this
                .get_meta(keys::BOOTSTRAP_FLOOR)
                .await?
                .and_then(|raw| raw.parse().ok()))
        })
    }

    fn set_bootstrap_floor(&self, floor: Option<i64>) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            this.put_meta(
                keys::BOOTSTRAP_FLOOR,
                floor.map(|f| f.to_string()).as_deref(),
            )
            .await
        })
    }

    fn pending_publishes(&self) -> StoreFuture<'_, Vec<PendingPublish>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[META])?, META)?;
            let range =
                IdbKeyRange::bound(&"pending:".into(), &"pending;".into()).map_err(js_error)?;
            array(req(store.get_all_with_key(&range)).await?)
                .iter()
                .map(|value| decode_pending(&value.as_string().unwrap_or_default()))
                .collect()
        })
    }

    fn pending(&self, key: String) -> StoreFuture<'_, Option<PendingPublish>> {
        let this = self.clone();
        guarded("read", async move {
            this.get_meta(&format!("{}{key}", keys::PENDING))
                .await?
                .as_deref()
                .map(decode_pending)
                .transpose()
        })
    }

    fn save_pending(&self, item: PendingPublish) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            this.put_meta(
                &format!("{}{}", keys::PENDING, item.key),
                Some(&encode_pending(&item)?),
            )
            .await
        })
    }

    fn pending_change_ids(&self) -> StoreFuture<'_, HashSet<String>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[META])?, META)?;
            let range =
                IdbKeyRange::bound(&"pending:".into(), &"pending;".into()).map_err(js_error)?;
            Ok(array(req(store.get_all_keys_with_key(&range)).await?)
                .iter()
                .filter_map(JsValue::as_string)
                .map(|key| pending_change_id(&key[keys::PENDING.len()..]).to_owned())
                .collect())
        })
    }

    fn is_published(&self, key: String) -> StoreFuture<'_, bool> {
        let this = self.clone();
        guarded("read", async move {
            Ok(this
                .get_meta(&format!("{}{key}", keys::PUBLISHED))
                .await?
                .is_some())
        })
    }

    fn mark_published(&self, key: String) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[META])?;
            let meta = os(&tx, META)?;
            meta.put_with_key(&"1".into(), &format!("{}{key}", keys::PUBLISHED).into())
                .map_err(js_error)?;
            meta.delete(&format!("{}{key}", keys::PENDING).into())
                .map_err(js_error)?;
            done(&tx).await
        })
    }

    fn record_held(&self, scope: String, events: Vec<HeldEvent>) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            if events.is_empty() {
                return Ok(());
            }
            let tx = this.write(&[RELAY_EVENTS])?;
            let store = os(&tx, RELAY_EVENTS)?;
            for event in events {
                if !req(store.get_key(&key2(scope.as_str(), event.id.as_str())))
                    .await?
                    .is_undefined()
                {
                    continue;
                }
                let value = object(&[
                    ("scope", scope.as_str().into()),
                    ("id", event.id.as_str().into()),
                    ("createdAt", (event.created_at as f64).into()),
                    ("kind", event.kind.into()),
                    ("b", opt(&event.b)),
                ]);
                store.put(&value).map_err(js_error)?;
            }
            done(&tx).await
        })
    }

    fn held_events(&self, scope: String, query: HeldQuery) -> StoreFuture<'_, Vec<HeldItem>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[RELAY_EVENTS])?, RELAY_EVENTS)?;
            let mut rows = Vec::new();
            match &query.b {
                Some(bases) => {
                    let index = store.index("scopeB").map_err(js_error)?;
                    for base in bases {
                        rows.extend(array(
                            req(index.get_all_with_key(&key2(scope.as_str(), base.as_str())))
                                .await?,
                        ));
                    }
                }
                None => {
                    let index = store.index("scopeKind").map_err(js_error)?;
                    for kind in &query.kinds {
                        rows.extend(array(
                            req(index.get_all_with_key(&key2(scope.as_str(), *kind))).await?,
                        ));
                    }
                }
            }
            let mut items = Vec::with_capacity(rows.len());
            for row in &rows {
                let kind = number(row, "kind")?;
                let created_at = number(row, "createdAt")?;
                if !query.kinds.iter().any(|k| i64::from(*k) == kind)
                    || query.since.is_some_and(|since| created_at < since)
                    || query.until.is_some_and(|until| created_at > until)
                {
                    continue;
                }
                items.push(HeldItem {
                    created_at,
                    id: text(row, "id")?,
                });
            }
            items.sort();
            items.dedup();
            Ok(items)
        })
    }

    fn replay_inputs(&self) -> StoreFuture<'_, HashMap<String, ReplayInput>> {
        let this = self.clone();
        guarded("read", async move {
            let tx = this.read(&[CHANGES, CHECKPOINTS, BASES])?;
            let index = os(&tx, CHANGES)?.index("seq").map_err(js_error)?;
            let ids_in_order = array(req(index.get_all_keys()).await?);
            let rows = array(req(index.get_all()).await?);
            let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (id, row) in ids_in_order.iter().zip(&rows) {
                if !field(row, "rebased").is_truthy() {
                    ids.entry(text(row, "objectId")?)
                        .or_default()
                        .push(id.as_string().unwrap_or_default());
                }
            }
            let checkpoints = array(req(os(&tx, CHECKPOINTS)?.get_all()).await?)
                .iter()
                .map(|row| Ok((text(row, "objectId")?, text(row, "hash")?)))
                .collect::<Result<HashMap<_, _>, StoreError>>()?;
            let mut bases: HashMap<String, Vec<String>> = HashMap::new();
            for row in array(req(os(&tx, BASES)?.get_all()).await?) {
                bases
                    .entry(text(&row, "objectId")?)
                    .or_default()
                    .push(text(&row, "hash")?);
            }
            Ok(replay_inputs_from(ids, checkpoints, bases))
        })
    }

    fn cached_replays(
        &self,
        object_ids: Vec<String>,
        engine_version: String,
    ) -> StoreFuture<'_, HashMap<String, CachedReplay>> {
        let this = self.clone();
        guarded("read", async move {
            let store = os(&this.read(&[STATES])?, STATES)?;
            let mut rows = HashMap::new();
            for object_id in object_ids {
                let value = req(store.get(&object_id.as_str().into())).await?;
                if value.is_undefined()
                    || opt_text(&value, "engineVersion").as_deref() != Some(engine_version.as_str())
                {
                    continue;
                }
                let input = ReplayInput {
                    change_count: number(&value, "changeCount")? as u64,
                    change_digest: text(&value, "changeDigest")?,
                    checkpoint_hash: text(&value, "checkpointHash")?,
                    base_hashes: text(&value, "baseHashes")?,
                };
                let state = bytes(&value, "state")?;
                rows.insert(
                    object_id.clone(),
                    CachedReplay {
                        object_id,
                        input,
                        state,
                    },
                );
            }
            Ok(rows)
        })
    }

    fn put_cached_replays(
        &self,
        rows: Vec<CachedReplay>,
        engine_version: String,
    ) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            if rows.is_empty() {
                return Ok(());
            }
            let tx = this.write(&[STATES])?;
            let store = os(&tx, STATES)?;
            for row in rows {
                let value = object(&[
                    ("engineVersion", engine_version.as_str().into()),
                    ("changeCount", (row.input.change_count as f64).into()),
                    ("changeDigest", row.input.change_digest.as_str().into()),
                    ("checkpointHash", row.input.checkpoint_hash.as_str().into()),
                    ("baseHashes", row.input.base_hashes.as_str().into()),
                    ("state", Uint8Array::from(row.state.as_slice()).into()),
                ]);
                store
                    .put_with_key(&value, &row.object_id.as_str().into())
                    .map_err(js_error)?;
            }
            done(&tx).await
        })
    }

    fn clear_cached_replays(&self) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&[STATES])?;
            os(&tx, STATES)?.clear().map_err(js_error)?;
            done(&tx).await
        })
    }

    fn clear(&self) -> StoreFuture<'_, ()> {
        let this = self.clone();
        guarded("write", async move {
            let tx = this.write(&ALL_STORES)?;
            for name in ALL_STORES {
                os(&tx, name)?.clear().map_err(js_error)?;
            }
            done(&tx).await
        })
    }

    fn close(&self) -> StoreFuture<'_, ()> {
        if let Some(db) = self.db.borrow_mut().take() {
            db.close();
        }
        Box::pin(async { Ok(()) })
    }
}

/// Marks one stored change rebased (inside the caller's transaction).
async fn mark(changes: &IdbObjectStore, id: &str) -> Result<(), StoreError> {
    let row = req(changes.get(&id.into())).await?;
    if row.is_undefined() {
        return Ok(());
    }
    Reflect::set(&row, &"rebased".into(), &true.into()).map_err(js_error)?;
    changes.put_with_key(&row, &id.into()).map_err(js_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    /// WebKit's silent failure: a request that fires neither success nor
    /// error. The guard rejects it, naming the failure for the UI.
    #[wasm_bindgen_test]
    async fn an_operation_that_never_settles_rejects_instead_of_hanging() {
        let started = js_sys::Date::now();
        let result = guarded::<()>("read", futures::future::pending()).await;
        assert_eq!(
            result,
            Err(StoreError::Unavailable(
                "local storage stopped responding (read); reload the page".into()
            ))
        );
        assert!(js_sys::Date::now() - started < f64::from(IDB_TIMEOUT_MS + 5_000));
    }

    #[wasm_bindgen_test]
    fn the_default_budget_is_the_one_the_store_documents() {
        assert_eq!(IDB_TIMEOUT_MS, 12_000);
    }
}

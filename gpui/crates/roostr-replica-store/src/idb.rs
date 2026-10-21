//! Browser backend: raw IndexedDB through web-sys, the schema shape of the
//! Svelte site's `engine/store.ts` in its own database (default
//! [`IdbChangeStore::DEFAULT_NAME`]).
//!
//! Every request is awaited through a [`Request`] future whose handlers wake
//! the task; wasm-bindgen-futures polls woken tasks in a microtask, so a
//! read-then-write inside one transaction keeps it active. Every operation
//! runs under the [`IDB_TIMEOUT_MS`] watchdog: WebKit can leave a request
//! pending with no event at all, which then surfaces as
//! [`StoreError::Unavailable`] instead of a future that never settles. The
//! watchdog counts only time the page was responsive and restarts on every
//! event any request or transaction fires, so a busy main thread (a long
//! replay, a slow machine), a large read that keeps answering, or an
//! operation queued behind another's long transaction is never mistaken
//! for a stalled database. Large reads go [`CHUNK`] rows per request
//! ([`each_row`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures::FutureExt as _;
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{
    Event, IdbCursor, IdbCursorDirection, IdbDatabase, IdbFactory, IdbIndex, IdbKeyRange,
    IdbObjectStore, IdbObjectStoreParameters, IdbOpenDbRequest, IdbRequest, IdbTransaction,
    IdbTransactionMode,
};

use crate::*;

pub mod svelte_import;

/// How long an operation may go without any of its requests or
/// transactions answering, in time the page was responsive, before the
/// database counts as stalled (`store.ts` `IDB_TIMEOUT_MS`).
pub const IDB_TIMEOUT_MS: u32 = 12_000;

/// Watchdog beats per budget: a beat that comes late (the main thread was
/// busy, or the tab is in the background) counts as [`LATE_BEAT`] beats.
const BEATS: u32 = 24;
const LATE_BEAT: f64 = 1.5;

/// Rows per request of a chunked read ([`each_row`]).
const CHUNK: u32 = 500;

/// Memo rows per write transaction of `put_cached_replays`.
const MEMO_BATCH: usize = 200;

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

thread_local! {
    /// Events IndexedDB has fired for this page's requests and transactions:
    /// while it moves, the database is answering.
    static ANSWERS: Cell<u64> = const { Cell::new(0) };
}

fn answers() -> u64 {
    ANSWERS.with(Cell::get)
}

#[derive(Default)]
struct Slot {
    outcome: Option<Result<JsValue, JsValue>>,
    waker: Option<Waker>,
    /// Settled by IndexedDB (not a timer): each settle counts in [`ANSWERS`].
    idb: bool,
}

impl Slot {
    /// A slot IndexedDB settles.
    fn idb() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            idb: true,
            ..Self::default()
        }))
    }
}

fn settle(slot: &Rc<RefCell<Slot>>, outcome: Result<JsValue, JsValue>) {
    let mut slot = slot.borrow_mut();
    if slot.idb {
        ANSWERS.with(|n| n.set(n.get() + 1));
    }
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
        let slot = Slot::idb();
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

/// Awaits `requests`, every one issued before the first is awaited: the
/// database answers them back to back instead of one event-loop turn each,
/// which on a busy page (a slow machine rendering and replaying) is what a
/// request costs.
async fn all(
    requests: impl IntoIterator<Item = Result<IdbRequest, JsValue>>,
) -> Result<Vec<JsValue>, StoreError> {
    let pending = requests
        .into_iter()
        .map(|request| request.map(Request::new).map_err(js_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::with_capacity(pending.len());
    for mut request in pending {
        out.push(request.next().await?);
    }
    Ok(out)
}

/// Resolves when `tx` commits; errors when it fails or aborts.
async fn done(tx: &IdbTransaction) -> Result<(), StoreError> {
    let slot = Slot::idb();
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

/// One watchdog beat's share of the time since the last: all of it while
/// the page keeps time, at most [`LATE_BEAT`] beats when the beat came late.
/// A beat is late when the main thread was busy (a long replay, a slow
/// machine) or the tab is in the background: time IndexedDB's answer could
/// not have been delivered in, so it never counts towards a stall.
fn responsive_ms(elapsed_ms: f64, beat_ms: u32) -> f64 {
    elapsed_ms.clamp(0.0, f64::from(beat_ms) * LATE_BEAT)
}

/// Resolves once IndexedDB has answered nothing ([`ANSWERS`]) for
/// `budget_ms` of responsive time ([`responsive_ms`]); errs only when no
/// timer can be set. Any answer counts, not only this operation's: one
/// queued behind another's long transaction waits as long as the database
/// keeps answering, and a request that is lost while others proceed is
/// reported once the database goes quiet.
async fn stall(budget_ms: u32) -> Result<(), StoreError> {
    let beat = (budget_ms / BEATS).max(1);
    let mut seen = answers();
    let mut quiet = 0.0;
    let mut last = js_sys::Date::now();
    while quiet < f64::from(budget_ms) {
        Timer::new(beat)?.await;
        let now = js_sys::Date::now();
        quiet += responsive_ms(now - last, beat);
        last = now;
        if answers() != seen {
            seen = answers();
            quiet = 0.0;
        }
    }
    Ok(())
}

/// Runs `body` under the [`IDB_TIMEOUT_MS`] watchdog ([`stall`]).
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
    guarded_within(IDB_TIMEOUT_MS, what, body)
}

fn guarded_within<T: 'static>(
    budget_ms: u32,
    what: &'static str,
    body: impl Future<Output = Result<T, StoreError>> + 'static,
) -> StoreFuture<'static, T> {
    Box::pin(async move {
        let (tx, rx) = futures::channel::oneshot::channel();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(body.await);
        });
        let body = async move { rx.await.unwrap_or(Err(StoreError::Closed)) };
        futures::select_biased! {
            result = body.fuse() => result,
            stalled = stall(budget_ms).fuse() => Err(stalled.err().unwrap_or_else(|| {
                StoreError::Unavailable(format!("local storage stopped responding ({what}); reload the page"))
            })),
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

/// Where [`each_row`] reads: an object store, in primary key order, or an
/// index with unique keys, in index key order (the value field holding it).
enum Rows<'a> {
    Store(&'a IdbObjectStore),
    Index(&'a IdbIndex, &'static str),
}

impl Rows<'_> {
    fn keys(&self, range: &JsValue) -> Result<IdbRequest, JsValue> {
        match self {
            Rows::Store(store) => store.get_all_keys_with_key_and_limit(range, CHUNK),
            Rows::Index(index, _) => index.get_all_keys_with_key_and_limit(range, CHUNK),
        }
    }

    fn values(&self, range: &JsValue) -> Result<IdbRequest, JsValue> {
        match self {
            Rows::Store(store) => store.get_all_with_key_and_limit(range, CHUNK),
            Rows::Index(index, _) => index.get_all_with_key_and_limit(range, CHUNK),
        }
    }

    /// The source key of a row: the next chunk starts after it.
    fn key_of(&self, primary: &JsValue, value: &JsValue) -> JsValue {
        match self {
            Rows::Store(_) => primary.clone(),
            Rows::Index(_, key) => field(value, key),
        }
    }
}

/// Every row of `rows` as `(primary key, value)`, [`CHUNK`] per request in
/// the caller's transaction: a large store is never one structured clone of
/// everything, and every chunk answered is progress the watchdog sees.
async fn each_row(
    rows: Rows<'_>,
    mut each: impl FnMut(JsValue, JsValue) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut range = JsValue::UNDEFINED;
    loop {
        // Both requested before either is awaited: one round trip per chunk.
        let mut keys = Request::new(rows.keys(&range).map_err(js_error)?);
        let mut values = Request::new(rows.values(&range).map_err(js_error)?);
        let keys = array(keys.next().await?);
        let values = array(values.next().await?);
        let Some(after) = keys
            .last()
            .zip(values.last())
            .map(|(k, v)| rows.key_of(k, v))
        else {
            return Ok(());
        };
        let full = values.len() >= CHUNK as usize;
        for (key, value) in keys.into_iter().zip(values) {
            each(key, value)?;
        }
        if !full {
            return Ok(());
        }
        range = IdbKeyRange::lower_bound_with_open(&after, true)
            .map_err(js_error)?
            .into();
    }
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
        let ranges = ids
            .iter()
            .map(|id| prefix(id))
            .collect::<Result<Vec<_>, _>>()?;
        let firsts = all(ranges.iter().map(|range| by_seq.get(range))).await?;
        let mut firsts = firsts
            .iter()
            .zip(ids)
            .map(|(first, id)| Ok((number(first, "seq")?, id)))
            .collect::<Result<Vec<_>, StoreError>>()?;
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
        let mut newest = Request::new(
            store
                .index("seq")
                .map_err(js_error)?
                .open_key_cursor_with_range_and_direction(&JsValue::NULL, IdbCursorDirection::Prev)
                .map_err(js_error)?,
        );
        let mut seen = HashSet::new();
        let fresh: Vec<&ChangeRecord> = records
            .iter()
            .filter(|record| seen.insert(record.id.as_str()))
            .collect();
        let held = all(fresh
            .iter()
            .map(|record| store.get_key(&record.id.as_str().into())))
        .await?;
        let mut seq = match newest.next().await?.dyn_into::<IdbCursor>() {
            Ok(cursor) => cursor.key().map_err(js_error)?.as_f64().unwrap_or(0.0) as i64,
            Err(_) => 0,
        };
        let mut inserted = 0;
        for (record, held) in fresh.into_iter().zip(held) {
            if !held.is_undefined() {
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
        let (mut keys, mut values) = Self::change_requests(changes, object_id)?;
        Self::changes_from(keys.next().await?, values.next().await?)
    }

    /// The keys and values requests of one object's changes, issued now.
    fn change_requests(
        changes: &IdbObjectStore,
        object_id: &str,
    ) -> Result<(Request, Request), StoreError> {
        let index = changes.index("objectSeq").map_err(js_error)?;
        let range = prefix(object_id)?;
        Ok((
            Request::new(index.get_all_keys_with_key(&range).map_err(js_error)?),
            Request::new(index.get_all_with_key(&range).map_err(js_error)?),
        ))
    }

    fn changes_from(keys: JsValue, values: JsValue) -> Result<Vec<ChangeRecord>, StoreError> {
        array(keys)
            .into_iter()
            .zip(array(values))
            .map(|(id, value)| change_from(id.as_string().unwrap_or_default(), &value))
            .collect()
    }

    async fn bases_in(
        bases: &IdbObjectStore,
        object_id: &str,
    ) -> Result<Vec<BaseRecord>, StoreError> {
        Self::bases_from(Self::base_request(bases, object_id)?.next().await?)
    }

    /// The request of one object's bases, issued now.
    fn base_request(bases: &IdbObjectStore, object_id: &str) -> Result<Request, StoreError> {
        Ok(Request::new(
            bases
                .get_all_with_key(&prefix(object_id)?)
                .map_err(js_error)?,
        ))
    }

    fn bases_from(values: JsValue) -> Result<Vec<BaseRecord>, StoreError> {
        let mut out = array(values)
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
            let rows = all(ids.iter().map(|id| changes.get(&id.as_str().into()))).await?;
            for (id, row) in ids.iter().zip(rows) {
                mark_row(&changes, id, row)?;
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
            let counts = all(ids
                .iter()
                .map(|id| index.count_with_key(&id.as_str().into())))
            .await?;
            Ok(ids
                .into_iter()
                .zip(counts)
                .map(|(id, count)| (id, count.as_f64().unwrap_or(0.0) as u64))
                .collect())
        })
    }

    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs> {
        let this = self.clone();
        guarded("read", async move {
            let tx = this.read(&[CHANGES, CHECKPOINTS, BASES, BASE_CURRENT])?;
            let key: JsValue = object_id.as_str().into();
            // Every request issued before any is awaited: one round trip.
            let (mut keys, mut values) = Self::change_requests(&os(&tx, CHANGES)?, &object_id)?;
            let mut bases = Self::base_request(&os(&tx, BASES)?, &object_id)?;
            let mut checkpoint = Request::new(os(&tx, CHECKPOINTS)?.get(&key).map_err(js_error)?);
            let mut current = Request::new(os(&tx, BASE_CURRENT)?.get(&key).map_err(js_error)?);
            let all = Self::changes_from(keys.next().await?, values.next().await?)?;
            let bases = Self::bases_from(bases.next().await?)?;
            let checkpoint = checkpoint.next().await?;
            let current = current.next().await?;
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
            let by_seq = os(&tx, CHANGES)?.index("seq").map_err(js_error)?;
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
            each_row(Rows::Index(&by_seq, "seq"), |_, row| {
                let at = slot(&mut histories, text(&row, "objectId")?);
                histories[at].changes.push(bytes(&row, "bytes")?);
                Ok(())
            })
            .await?;
            let mut held = Vec::new();
            each_row(Rows::Store(&os(&tx, CHECKPOINTS)?), |_, row| {
                held.push(checkpoint_from(&row)?);
                Ok(())
            })
            .await?;
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
            let mut out = HashMap::new();
            each_row(
                Rows::Store(&os(&this.read(&[CHECKPOINTS])?, CHECKPOINTS)?),
                |_, row| {
                    let record = checkpoint_from(&row)?;
                    out.insert(record.object_id.clone(), record);
                    Ok(())
                },
            )
            .await?;
            Ok(out)
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
            let held = all(events
                .iter()
                .map(|event| store.get_key(&key2(scope.as_str(), event.id.as_str()))))
            .await?;
            for (event, held) in events.into_iter().zip(held) {
                if !held.is_undefined() {
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
            let by_seq = os(&tx, CHANGES)?.index("seq").map_err(js_error)?;
            let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
            each_row(Rows::Index(&by_seq, "seq"), |id, row| {
                if !field(&row, "rebased").is_truthy() {
                    ids.entry(text(&row, "objectId")?)
                        .or_default()
                        .push(id.as_string().unwrap_or_default());
                }
                Ok(())
            })
            .await?;
            let mut checkpoints = HashMap::new();
            each_row(Rows::Store(&os(&tx, CHECKPOINTS)?), |_, row| {
                checkpoints.insert(text(&row, "objectId")?, text(&row, "hash")?);
                Ok(())
            })
            .await?;
            // `[objectId, hash]` is the key: the base protobufs stay unread.
            let mut bases: HashMap<String, Vec<String>> = HashMap::new();
            for key in array(req(os(&tx, BASES)?.get_all_keys()).await?) {
                let key = array(key);
                let (Some(object_id), Some(hash)) = (
                    key.first().and_then(JsValue::as_string),
                    key.get(1).and_then(JsValue::as_string),
                ) else {
                    return Err(StoreError::Corrupt(
                        "base key is not [objectId, hash]".into(),
                    ));
                };
                bases.entry(object_id).or_default().push(hash);
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
            for batch in object_ids.chunks(CHUNK as usize) {
                let values = all(batch.iter().map(|id| store.get(&id.as_str().into()))).await?;
                for (object_id, value) in batch.iter().zip(values) {
                    if value.is_undefined()
                        || opt_text(&value, "engineVersion").as_deref()
                            != Some(engine_version.as_str())
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
                            object_id: object_id.clone(),
                            input,
                            state,
                        },
                    );
                }
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
            // Memo rows are independent: one transaction per batch, each
            // commit progress the watchdog sees.
            for batch in rows.chunks(MEMO_BATCH) {
                let tx = this.write(&[STATES])?;
                let store = os(&tx, STATES)?;
                for row in batch {
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
                done(&tx).await?;
            }
            Ok(())
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
    mark_row(changes, id, row)
}

/// Writes `row` (change `id` as read, if held) back marked rebased.
fn mark_row(changes: &IdbObjectStore, id: &str, row: JsValue) -> Result<(), StoreError> {
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
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    wasm_bindgen_test_configure!(run_in_browser);

    /// A short budget keeps these quick; the beat scales with it.
    const BUDGET: u32 = 1_200;

    fn stalled<T>(what: &str) -> Result<T, StoreError> {
        Err(StoreError::Unavailable(format!(
            "local storage stopped responding ({what}); reload the page"
        )))
    }

    /// Holds the main thread, as a long replay on a slow machine does.
    fn busy(ms: f64) {
        let until = js_sys::Date::now() + ms;
        while js_sys::Date::now() < until {}
    }

    async fn sleep(ms: u32) {
        Timer::new(ms).unwrap().await;
    }

    async fn scratch(name: &str) -> IdbChangeStore {
        IdbChangeStore::destroy(name).await.unwrap();
        IdbChangeStore::open(name).await.unwrap()
    }

    /// WebKit's silent failure: a request that fires neither success nor
    /// error. The watchdog rejects it, naming the failure for the UI.
    #[wasm_bindgen_test]
    async fn an_operation_that_never_settles_rejects_instead_of_hanging() {
        let started = js_sys::Date::now();
        let result = guarded_within::<()>(BUDGET, "read", futures::future::pending()).await;
        assert_eq!(result, stalled("read"));
        let took = js_sys::Date::now() - started;
        assert!(
            took >= f64::from(BUDGET) && took < f64::from(BUDGET) * 3.0,
            "{took}"
        );
    }

    /// The false alarm on large vaults: IndexedDB answered, but the main
    /// thread stayed busy past the budget before the answer was delivered.
    #[wasm_bindgen_test]
    async fn a_busy_main_thread_is_not_a_stalled_database() {
        let store = scratch("roostr-guard-busy").await;
        let reader = store.clone();
        let result = guarded_within(BUDGET, "read", async move {
            let mut read = Request::new(
                os(&reader.read(&[META])?, META)?
                    .get(&"k".into())
                    .map_err(js_error)?,
            );
            busy(f64::from(BUDGET) * 3.0);
            Ok(read.next().await?.is_undefined())
        })
        .await;
        assert_eq!(result, Ok(true));

        // Busy in another task while the operation is between requests.
        let reader = store.clone();
        let op = guarded_within(BUDGET, "read", async move {
            for _ in 0..5 {
                reader.get_meta("k").await?;
                sleep(20).await;
            }
            Ok(())
        });
        wasm_bindgen_futures::spawn_local(async {
            sleep(30).await;
            busy(f64::from(BUDGET) * 3.0);
        });
        assert_eq!(op.await, Ok(()));
        store.close().await.unwrap();
    }

    /// The budget is for silence, not for the whole operation.
    #[wasm_bindgen_test]
    async fn an_operation_that_keeps_answering_outlives_the_budget() {
        let store = scratch("roostr-guard-long").await;
        let reader = store.clone();
        let started = js_sys::Date::now();
        let reads = guarded_within(BUDGET, "read", async move {
            let mut reads = 0;
            while js_sys::Date::now() - started < f64::from(BUDGET) * 3.0 {
                reader.get_meta("k").await?;
                sleep(BUDGET / 4).await;
                reads += 1;
            }
            Ok(reads)
        })
        .await;
        assert!(reads.unwrap() > 4);
        store.close().await.unwrap();
    }

    /// An operation queued behind another's long transaction waits for it:
    /// the database is answering, only not this operation yet.
    #[wasm_bindgen_test]
    async fn an_operation_queued_behind_a_long_transaction_waits_for_it() {
        let store = scratch("roostr-guard-queued").await;
        let started = js_sys::Date::now();
        let writer = store.clone();
        let long = guarded_within(BUDGET, "write", async move {
            let tx = writer.write(&[META])?;
            let meta = os(&tx, META)?;
            // One request after another holds the transaction past the budget.
            while js_sys::Date::now() - started < f64::from(BUDGET) * 2.5 {
                req(meta.get(&"k".into())).await?;
            }
            meta.put_with_key(&"v".into(), &"k".into())
                .map_err(js_error)?;
            done(&tx).await
        });
        let reader = store.clone();
        let queued = guarded_within(BUDGET, "read", async move { reader.get_meta("k").await });
        let (long, queued) = futures::join!(long, queued);
        assert_eq!(long, Ok(()));
        assert_eq!(queued, Ok(Some("v".into())));
        store.close().await.unwrap();
    }

    /// A request lost while others proceed is reported once the database
    /// goes quiet.
    #[wasm_bindgen_test]
    async fn a_lost_request_is_reported_once_the_database_goes_quiet() {
        let store = scratch("roostr-guard-others").await;
        let started = js_sys::Date::now();
        let lost = guarded_within::<()>(BUDGET, "read", futures::future::pending());
        let others = async {
            while js_sys::Date::now() - started < f64::from(BUDGET) * 2.5 {
                store.meta("k".into()).await.unwrap();
                sleep(30).await;
            }
        };
        let (lost, ()) = futures::join!(lost, others);
        assert_eq!(lost, stalled("read"));
        let took = js_sys::Date::now() - started;
        assert!(took >= f64::from(BUDGET) * 3.0, "{took}");
        store.close().await.unwrap();
    }

    #[wasm_bindgen_test]
    fn a_late_beat_counts_one_and_a_half_beats_at_most() {
        assert_eq!(responsive_ms(400.0, 500), 400.0);
        assert_eq!(responsive_ms(15_000.0, 500), 750.0);
        assert_eq!(responsive_ms(-3.0, 500), 0.0);
    }

    /// Reads span chunks: every row comes back once, in insertion order.
    #[wasm_bindgen_test]
    async fn chunked_reads_return_every_row_in_order() {
        const NAME: &str = "roostr-replica-chunks";
        let store = scratch(NAME).await;
        let n = CHUNK as usize * 2 + 7;
        // Ids sort against insertion order: the seq index, not the key, orders rows.
        let records: Vec<ChangeRecord> = (0..n)
            .map(|i| ChangeRecord {
                id: format!("c{:05}", n - i),
                object_id: format!("o{}", i % 3),
                bytes: vec![(i % 256) as u8, (i / 256) as u8],
                json: "{}".into(),
                b: None,
                rebased: i == 5,
            })
            .collect();
        assert_eq!(store.add_changes(records.clone()).await.unwrap(), n);

        let histories = store.all_change_histories().await.unwrap();
        let objects: Vec<&str> = histories.iter().map(|h| h.object_id.as_str()).collect();
        assert_eq!(objects, ["o0", "o1", "o2"]);
        for history in &histories {
            let expected: Vec<Vec<u8>> = records
                .iter()
                .filter(|r| r.object_id == history.object_id)
                .map(|r| r.bytes.clone())
                .collect();
            assert_eq!(history.changes, expected);
        }

        let inputs = store.replay_inputs().await.unwrap();
        assert_eq!(inputs.len(), 3);
        for (object_id, input) in &inputs {
            let live = records
                .iter()
                .filter(|r| &r.object_id == object_id && !r.rebased)
                .map(|r| r.id.as_str());
            assert_eq!(input, &ReplayInput::new(live, "", []));
        }
        assert!(store.all_checkpoints().await.unwrap().is_empty());
        store.close().await.unwrap();
        IdbChangeStore::destroy(NAME).await.unwrap();
    }

    #[wasm_bindgen_test]
    fn the_default_budget_is_the_one_the_store_documents() {
        assert_eq!(IDB_TIMEOUT_MS, 12_000);
    }
}

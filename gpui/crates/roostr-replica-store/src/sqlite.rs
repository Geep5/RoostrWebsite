//! Native backend: one SQLite connection (bundled, WAL) behind a mutex; every
//! call runs on the `blocking` thread pool, never on the caller's thread.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter,
};

use crate::*;

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Backend(error.to_string())
    }
}

/// Schema version written to `PRAGMA user_version` once [`migrate`] ran.
const SCHEMA_VERSION: i64 = 1;

/// [`ChangeStore`] over SQLite. Tables mirror the iOS store: `changes`,
/// `meta`, `checkpoints`, `bases`, `held` (scoped like the browser's
/// `relay-events`) and `replay_cache`.
#[derive(Clone)]
pub struct SqliteChangeStore {
    connection: Arc<Mutex<Option<Connection>>>,
}

impl SqliteChangeStore {
    /// Opens (creating and migrating) the database at `path`.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        blocking::unblock(move || Self::from_connection(Connection::open(&path)?)).await
    }

    /// A fresh in-memory store (tests, ephemeral sessions).
    pub async fn in_memory() -> Result<Self, StoreError> {
        blocking::unblock(|| Self::from_connection(Connection::open_in_memory()?)).await
    }

    /// Deletes the database at `path` with its WAL and shared-memory files
    /// (logout / identity switch). Close every store on it first.
    pub async fn destroy(path: impl Into<PathBuf>) -> Result<(), StoreError> {
        let path = path.into();
        blocking::unblock(move || {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut file = path.clone().into_os_string();
                file.push(suffix);
                match std::fs::remove_file(Path::new(&file)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(StoreError::Backend(format!(
                            "delete {}: {e}",
                            Path::new(&file).display()
                        )));
                    }
                }
            }
            Ok(())
        })
        .await
    }

    fn from_connection(mut connection: Connection) -> Result<Self, StoreError> {
        migrate(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(Some(connection))),
        })
    }

    fn run<T, F>(&self, body: F) -> StoreFuture<'static, T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let connection = self.connection.clone();
        Box::pin(blocking::unblock(move || {
            let mut guard = connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let connection = guard.as_mut().ok_or(StoreError::Closed)?;
            body(connection)
        }))
    }

    fn write<T, F>(&self, body: F) -> StoreFuture<'static, T>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
    {
        self.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = body(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }
}

/// Creates the schema, or brings an older one up to date keeping its rows.
fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS changes (
            id TEXT PRIMARY KEY,
            object_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            bytes BLOB NOT NULL,
            json TEXT NOT NULL,
            b TEXT,
            rebased INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS checkpoints (
            object_id TEXT PRIMARY KEY,
            bytes BLOB NOT NULL,
            hash TEXT NOT NULL,
            heads TEXT NOT NULL,
            covered INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS bases (
            object_id TEXT NOT NULL,
            hash TEXT NOT NULL,
            epoch INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            prev_base TEXT NOT NULL DEFAULT '',
            bytes BLOB NOT NULL,
            PRIMARY KEY (object_id, hash)
        );
        CREATE TABLE IF NOT EXISTS replay_cache (
            object_id TEXT PRIMARY KEY,
            engine_version TEXT NOT NULL,
            change_count INTEGER NOT NULL,
            change_digest TEXT NOT NULL,
            checkpoint_hash TEXT NOT NULL,
            state BLOB NOT NULL,
            base_hashes TEXT NOT NULL DEFAULT ''
        );",
    )?;
    add_column(&tx, "changes", "b", "b TEXT")?;
    add_column(
        &tx,
        "changes",
        "rebased",
        "rebased INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        &tx,
        "bases",
        "prev_base",
        "prev_base TEXT NOT NULL DEFAULT ''",
    )?;
    add_column(
        &tx,
        "replay_cache",
        "base_hashes",
        "base_hashes TEXT NOT NULL DEFAULT ''",
    )?;
    // An unscoped `held` (iOS layout) cannot be split into streams; it is a
    // relay-reconstructible set, so the next reconcile re-fetches it once.
    if table_exists(&tx, "held")? && !has_column(&tx, "held", "scope")? {
        tx.execute_batch("DROP TABLE held")?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS held (
            scope TEXT NOT NULL,
            id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            kind INTEGER NOT NULL,
            b TEXT,
            PRIMARY KEY (scope, id)
        );
        CREATE INDEX IF NOT EXISTS changes_object_id ON changes(object_id, seq);
        CREATE INDEX IF NOT EXISTS held_scope_kind ON held(scope, kind, created_at, id);
        CREATE INDEX IF NOT EXISTS held_scope_b ON held(scope, b);",
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn table_exists(tx: &Transaction<'_>, table: &str) -> Result<bool, StoreError> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn has_column(tx: &Transaction<'_>, table: &str, column: &str) -> Result<bool, StoreError> {
    let mut info = tx.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = info.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Adds `column` unless present; an older build's rows keep their values.
fn add_column(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), StoreError> {
    if !has_column(tx, table, column)? {
        tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {declaration}"))?;
    }
    Ok(())
}

fn get_meta(connection: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(connection
        .prepare_cached("SELECT value FROM meta WHERE key = ?")?
        .query_row([key], |row| row.get(0))
        .optional()?)
}

fn put_meta(connection: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    connection
        .prepare_cached("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")?
        .execute([key, value])?;
    Ok(())
}

fn delete_meta(connection: &Connection, key: &str) -> Result<(), StoreError> {
    connection
        .prepare_cached("DELETE FROM meta WHERE key = ?")?
        .execute([key])?;
    Ok(())
}

/// Inserts changes not held yet, numbering them after the newest; returns how many were new.
fn insert_changes(tx: &Transaction<'_>, records: &[ChangeRecord]) -> Result<usize, StoreError> {
    let mut seq: i64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM changes", [], |row| {
        row.get(0)
    })?;
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO changes (id, object_id, seq, bytes, json, b, rebased) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )?;
    let mut inserted = 0;
    for record in records {
        let added = insert.execute(params![
            record.id,
            record.object_id,
            seq + 1,
            record.bytes,
            record.json,
            record.b,
            record.rebased
        ])?;
        seq += added as i64;
        inserted += added;
    }
    Ok(inserted)
}

fn select_changes(
    connection: &Connection,
    object_id: &str,
) -> Result<Vec<ChangeRecord>, StoreError> {
    let mut select = connection.prepare_cached(
        "SELECT id, bytes, json, b, rebased FROM changes WHERE object_id = ? ORDER BY seq",
    )?;
    let rows = select.query_map([object_id], |row| {
        Ok(ChangeRecord {
            id: row.get(0)?,
            object_id: object_id.to_owned(),
            bytes: row.get(1)?,
            json: row.get(2)?,
            b: row.get(3)?,
            rebased: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn select_bases(connection: &Connection, object_id: &str) -> Result<Vec<BaseRecord>, StoreError> {
    let mut select = connection.prepare_cached(
        "SELECT hash, epoch, created_at, prev_base, bytes FROM bases WHERE object_id = ? ORDER BY epoch, created_at, hash",
    )?;
    let rows = select.query_map([object_id], |row| {
        Ok(BaseRecord {
            object_id: object_id.to_owned(),
            hash: row.get(0)?,
            epoch: row.get(1)?,
            created_at: row.get(2)?,
            prev_base: row.get(3)?,
            bytes: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn checkpoint_row(
    object_id: String,
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(CheckpointRecord, String)> {
    Ok((
        CheckpointRecord {
            object_id,
            bytes: row.get(0)?,
            hash: row.get(1)?,
            heads: Vec::new(),
            covered: row.get(3)?,
        },
        row.get(2)?,
    ))
}

fn with_heads(
    (mut record, heads): (CheckpointRecord, String),
) -> Result<CheckpointRecord, StoreError> {
    record.heads = serde_json::from_str(&heads)
        .map_err(|e| StoreError::Corrupt(format!("checkpoint heads: {e}")))?;
    Ok(record)
}

fn select_checkpoint(
    connection: &Connection,
    object_id: &str,
) -> Result<Option<CheckpointRecord>, StoreError> {
    connection
        .prepare_cached("SELECT bytes, hash, heads, covered FROM checkpoints WHERE object_id = ?")?
        .query_row([object_id], |row| checkpoint_row(object_id.to_owned(), row))
        .optional()?
        .map(with_heads)
        .transpose()
}

const CURRENT_BASES: &str = "SELECT object_id, hash, created_at FROM bases AS b WHERE NOT EXISTS (
    SELECT 1 FROM bases AS o WHERE o.object_id = b.object_id AND (o.epoch > b.epoch OR (o.epoch = b.epoch AND
        (o.created_at > b.created_at OR (o.created_at = b.created_at AND o.hash > b.hash)))))";

const INSERT_HELD: &str =
    "INSERT OR IGNORE INTO held (scope, id, created_at, kind, b) VALUES (?, ?, ?, ?, ?)";

impl ChangeStore for SqliteChangeStore {
    fn add_changes(&self, records: Vec<ChangeRecord>) -> StoreFuture<'_, usize> {
        if records.is_empty() {
            return Box::pin(async { Ok(0) });
        }
        self.write(move |tx| insert_changes(tx, &records))
    }

    fn import(&self, batch: ImportBatch) -> StoreFuture<'_, usize> {
        if batch.is_empty() {
            return Box::pin(async { Ok(0) });
        }
        self.write(move |tx| {
            let added = insert_changes(tx, &batch.changes)?;
            for key in &batch.published {
                put_meta(tx, &format!("{}{key}", keys::PUBLISHED), "1")?;
                delete_meta(tx, &format!("{}{key}", keys::PENDING))?;
            }
            let mut insert = tx.prepare_cached(INSERT_HELD)?;
            for (scope, event) in &batch.held {
                insert.execute(params![
                    scope,
                    event.id,
                    event.created_at,
                    event.kind,
                    event.b
                ])?;
            }
            Ok(added)
        })
    }

    fn add_local_change(
        &self,
        record: ChangeRecord,
        pending: PendingPublish,
    ) -> StoreFuture<'_, ()> {
        self.write(move |tx| {
            insert_changes(tx, std::slice::from_ref(&record))?;
            put_meta(
                tx,
                &format!("{}{}", keys::PENDING, pending.key),
                &encode_pending(&pending)?,
            )
        })
    }

    fn add_rebased(&self, orphan_id: String, record: ChangeRecord) -> StoreFuture<'_, ()> {
        self.write(move |tx| {
            insert_changes(tx, std::slice::from_ref(&record))?;
            tx.prepare_cached("UPDATE changes SET rebased = 1 WHERE id = ?")?
                .execute([orphan_id])?;
            Ok(())
        })
    }

    fn mark_rebased(&self, ids: Vec<String>) -> StoreFuture<'_, ()> {
        self.write(move |tx| {
            let mut update = tx.prepare_cached("UPDATE changes SET rebased = 1 WHERE id = ?")?;
            for id in ids {
                update.execute([id])?;
            }
            Ok(())
        })
    }

    fn changes_for(&self, object_id: String) -> StoreFuture<'_, Vec<ChangeRecord>> {
        self.run(move |connection| select_changes(connection, &object_id))
    }

    fn unpublished_changes(
        &self,
        object_ids: Vec<String>,
        key_prefix: String,
    ) -> StoreFuture<'_, Vec<ChangeRecord>> {
        self.run(move |connection| {
            let tx = connection.transaction()?;
            let marked = format!("{}{key_prefix}", keys::PUBLISHED);
            let mut select = tx.prepare_cached(
                "SELECT id, bytes, json, b, rebased FROM changes AS c WHERE object_id = ?
                    AND NOT EXISTS (SELECT 1 FROM meta WHERE key = ? || c.id) ORDER BY seq",
            )?;
            let mut out = Vec::new();
            for object_id in &object_ids {
                let rows = select.query_map(params![object_id, marked], |row| {
                    Ok(ChangeRecord {
                        id: row.get(0)?,
                        object_id: object_id.clone(),
                        bytes: row.get(1)?,
                        json: row.get(2)?,
                        b: row.get(3)?,
                        rebased: row.get(4)?,
                    })
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        })
    }

    fn object_ids(&self) -> StoreFuture<'_, Vec<String>> {
        self.run(|connection| {
            let mut select = connection.prepare_cached(
                "SELECT object_id FROM (
                    SELECT object_id, MIN(seq) AS first FROM changes GROUP BY object_id
                    UNION SELECT object_id, NULL FROM checkpoints WHERE object_id NOT IN (SELECT object_id FROM changes)
                    UNION SELECT object_id, NULL FROM bases WHERE object_id NOT IN (SELECT object_id FROM changes)
                ) ORDER BY first IS NULL, first, object_id",
            )?;
            let ids = select.query_map([], |row| row.get(0))?;
            Ok(ids.collect::<Result<_, _>>()?)
        })
    }

    fn change_counts(&self) -> StoreFuture<'_, HashMap<String, u64>> {
        self.run(|connection| {
            let mut select = connection
                .prepare_cached("SELECT object_id, COUNT(*) FROM changes GROUP BY object_id")?;
            let counts = select.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(counts.collect::<Result<_, _>>()?)
        })
    }

    fn object_inputs(&self, object_id: String) -> StoreFuture<'_, ObjectInputs> {
        self.run(move |connection| {
            let tx = connection.transaction()?;
            let all = select_changes(&tx, &object_id)?;
            let bases = select_bases(&tx, &object_id)?;
            let checkpoint = select_checkpoint(&tx, &object_id)?;
            let count = all.len() as u64;
            let current_base = BaseRecord::current(&bases).map(|base| CurrentBase {
                hash: base.hash.clone(),
                created_at: base.created_at,
            });
            let changes = all.into_iter().filter(|change| !change.rebased).collect();
            Ok(ObjectInputs {
                changes,
                bases,
                checkpoint,
                count,
                current_base,
            })
        })
    }

    fn all_change_histories(&self) -> StoreFuture<'_, Vec<ObjectHistory>> {
        self.run(|connection| {
            let tx = connection.transaction()?;
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
            {
                let mut select =
                    tx.prepare_cached("SELECT object_id, bytes FROM changes ORDER BY seq")?;
                let mut rows = select.query([])?;
                while let Some(row) = rows.next()? {
                    let at = slot(&mut histories, row.get(0)?);
                    histories[at].changes.push(row.get(1)?);
                }
            }
            let mut select =
                tx.prepare_cached("SELECT object_id, bytes FROM checkpoints ORDER BY object_id")?;
            let mut rows = select.query([])?;
            while let Some(row) = rows.next()? {
                let at = slot(&mut histories, row.get(0)?);
                histories[at].checkpoint = Some(row.get(1)?);
            }
            Ok(histories)
        })
    }

    fn put_base(&self, record: BaseRecord, streams: Vec<String>) -> StoreFuture<'_, bool> {
        self.write(move |tx| {
            let mut by_stream = decode_streams(get_meta(tx, keys::BASE_STREAMS)?.as_deref())?;
            if advance_streams(&mut by_stream, &streams, record.created_at) {
                let encoded = serde_json::to_string(&by_stream).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                put_meta(tx, keys::BASE_STREAMS, &encoded)?;
            }
            let added = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO bases (object_id, hash, epoch, created_at, prev_base, bytes) VALUES (?, ?, ?, ?, ?, ?)",
                )?
                .execute(params![
                    record.object_id,
                    record.hash,
                    record.epoch,
                    record.created_at,
                    record.prev_base,
                    record.bytes
                ])?;
            if added == 0 {
                return Ok(false);
            }
            tx.prepare_cached(
                "DELETE FROM bases WHERE object_id = ?1 AND epoch <= (SELECT MAX(epoch) FROM bases WHERE object_id = ?1) - ?2",
            )?
            .execute(params![record.object_id, BASE_CHAIN_KEPT])?;
            Ok(true)
        })
    }

    fn bases(&self, object_id: String) -> StoreFuture<'_, Vec<BaseRecord>> {
        self.run(move |connection| select_bases(connection, &object_id))
    }

    fn current_base(&self, object_id: String) -> StoreFuture<'_, Option<CurrentBase>> {
        self.run(move |connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT hash, created_at FROM bases WHERE object_id = ? ORDER BY epoch DESC, created_at DESC, hash DESC LIMIT 1",
                )?
                .query_row([object_id], |row| Ok(CurrentBase { hash: row.get(0)?, created_at: row.get(1)? }))
                .optional()?)
        })
    }

    fn current_bases(&self) -> StoreFuture<'_, HashMap<String, CurrentBase>> {
        self.run(|connection| {
            let mut select = connection.prepare_cached(CURRENT_BASES)?;
            let rows = select.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    CurrentBase {
                        hash: row.get(1)?,
                        created_at: row.get(2)?,
                    },
                ))
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn current_base_hashes(&self) -> StoreFuture<'_, Vec<String>> {
        self.run(|connection| {
            let mut select = connection
                .prepare_cached(&format!("SELECT hash FROM ({CURRENT_BASES}) ORDER BY hash"))?;
            let rows = select.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn objects_without_base(&self) -> StoreFuture<'_, Vec<String>> {
        self.run(|connection| {
            let mut select = connection.prepare_cached(
                "SELECT DISTINCT object_id FROM changes WHERE object_id NOT IN (SELECT object_id FROM bases) ORDER BY object_id",
            )?;
            let rows = select.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn newest_base_by_stream(&self) -> StoreFuture<'_, HashMap<String, i64>> {
        self.run(|connection| decode_streams(get_meta(connection, keys::BASE_STREAMS)?.as_deref()))
    }

    fn checkpoint(&self, object_id: String) -> StoreFuture<'_, Option<CheckpointRecord>> {
        self.run(move |connection| select_checkpoint(connection, &object_id))
    }

    fn put_checkpoint(
        &self,
        engine: &roostr_engine::Engine,
        record: CheckpointRecord,
    ) -> StoreFuture<'_, bool> {
        let engine = engine.clone();
        self.write(move |tx| {
            if let Some(held) = select_checkpoint(tx, &record.object_id)?
                // The native engine call completes synchronously on this thread.
                && !futures::executor::block_on(record.supersedes(&held, &engine))?
            {
                return Ok(false);
            }
            let heads = serde_json::to_string(&record.heads).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            tx.prepare_cached("INSERT OR REPLACE INTO checkpoints (object_id, bytes, hash, heads, covered) VALUES (?, ?, ?, ?, ?)")?
                .execute(params![record.object_id, record.bytes, record.hash, heads, record.covered])?;
            Ok(true)
        })
    }

    fn all_checkpoints(&self) -> StoreFuture<'_, HashMap<String, CheckpointRecord>> {
        self.run(|connection| {
            let mut select = connection
                .prepare_cached("SELECT bytes, hash, heads, covered, object_id FROM checkpoints")?;
            let rows = select.query_map([], |row| checkpoint_row(row.get(4)?, row))?;
            let mut out = HashMap::new();
            for row in rows {
                let record = with_heads(row?)?;
                out.insert(record.object_id.clone(), record);
            }
            Ok(out)
        })
    }

    fn forget_checkpoint_floors(&self) -> StoreFuture<'_, ()> {
        self.write(|tx| {
            let Some(raw) = get_meta(tx, keys::CHECKPOINT_FLOORS)? else {
                return Ok(());
            };
            delete_meta(tx, keys::CHECKPOINT_FLOORS)?;
            if floors_shortened_walk(&raw) {
                delete_meta(tx, keys::BOOTSTRAPPED)?;
            }
            Ok(())
        })
    }

    fn meta(&self, key: String) -> StoreFuture<'_, Option<String>> {
        self.run(move |connection| get_meta(connection, &key))
    }

    fn set_meta(&self, key: String, value: Option<String>) -> StoreFuture<'_, ()> {
        self.run(move |connection| match value {
            Some(value) => put_meta(connection, &key, &value),
            None => delete_meta(connection, &key),
        })
    }

    fn cursor(&self) -> StoreFuture<'_, i64> {
        self.run(|connection| {
            Ok(get_meta(connection, keys::CURSOR)?
                .and_then(|raw| raw.parse().ok())
                .unwrap_or(0))
        })
    }

    fn set_cursor(
        &self,
        cursor: i64,
        replay_groups: Option<Vec<(String, i64)>>,
    ) -> StoreFuture<'_, ()> {
        self.write(move |tx| {
            put_meta(tx, keys::CURSOR, &cursor.to_string())?;
            if let Some(groups) = replay_groups {
                put_meta(tx, keys::REPLAY_GROUPS, &encode_groups(&groups))?;
            }
            Ok(())
        })
    }

    fn replay_groups(&self) -> StoreFuture<'_, Vec<(String, i64)>> {
        self.run(
            |connection| match get_meta(connection, keys::REPLAY_GROUPS)? {
                Some(raw) => decode_groups(&raw),
                None => Ok(Vec::new()),
            },
        )
    }

    fn bootstrapped(&self) -> StoreFuture<'_, bool> {
        self.run(|connection| Ok(get_meta(connection, keys::BOOTSTRAPPED)?.as_deref() == Some("1")))
    }

    fn set_bootstrapped(&self) -> StoreFuture<'_, ()> {
        self.run(|connection| put_meta(connection, keys::BOOTSTRAPPED, "1"))
    }

    fn bootstrap_floor(&self) -> StoreFuture<'_, Option<i64>> {
        self.run(|connection| {
            Ok(get_meta(connection, keys::BOOTSTRAP_FLOOR)?.and_then(|raw| raw.parse().ok()))
        })
    }

    fn set_bootstrap_floor(&self, floor: Option<i64>) -> StoreFuture<'_, ()> {
        self.run(move |connection| match floor {
            Some(floor) => put_meta(connection, keys::BOOTSTRAP_FLOOR, &floor.to_string()),
            None => delete_meta(connection, keys::BOOTSTRAP_FLOOR),
        })
    }

    fn pending_publishes(&self) -> StoreFuture<'_, Vec<PendingPublish>> {
        self.run(|connection| {
            // ';' sorts right after ':': the key range of every `pending:` row.
            let mut select = connection.prepare_cached(
                "SELECT value FROM meta WHERE key >= 'pending:' AND key < 'pending;' ORDER BY key",
            )?;
            let rows = select.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|raw| decode_pending(&raw?)).collect()
        })
    }

    fn pending(&self, key: String) -> StoreFuture<'_, Option<PendingPublish>> {
        self.run(move |connection| {
            get_meta(connection, &format!("{}{key}", keys::PENDING))?
                .as_deref()
                .map(decode_pending)
                .transpose()
        })
    }

    fn save_pending(&self, item: PendingPublish) -> StoreFuture<'_, ()> {
        self.run(move |connection| {
            put_meta(
                connection,
                &format!("{}{}", keys::PENDING, item.key),
                &encode_pending(&item)?,
            )
        })
    }

    fn pending_change_ids(&self) -> StoreFuture<'_, HashSet<String>> {
        self.run(|connection| {
            let mut select = connection.prepare_cached(
                "SELECT key FROM meta WHERE key >= 'pending:' AND key < 'pending;'",
            )?;
            let rows = select.query_map([], |row| row.get::<_, String>(0))?;
            let mut ids = HashSet::new();
            for key in rows {
                let key = key?;
                ids.insert(pending_change_id(&key[keys::PENDING.len()..]).to_owned());
            }
            Ok(ids)
        })
    }

    fn is_published(&self, key: String) -> StoreFuture<'_, bool> {
        self.run(move |connection| {
            Ok(get_meta(connection, &format!("{}{key}", keys::PUBLISHED))?.is_some())
        })
    }

    fn mark_published(&self, key: String) -> StoreFuture<'_, ()> {
        self.write(move |tx| {
            put_meta(tx, &format!("{}{key}", keys::PUBLISHED), "1")?;
            delete_meta(tx, &format!("{}{key}", keys::PENDING))
        })
    }

    fn record_held(&self, scope: String, events: Vec<HeldEvent>) -> StoreFuture<'_, ()> {
        if events.is_empty() {
            return Box::pin(async { Ok(()) });
        }
        self.write(move |tx| {
            let mut insert = tx.prepare_cached(INSERT_HELD)?;
            for event in events {
                insert.execute(params![
                    scope,
                    event.id,
                    event.created_at,
                    event.kind,
                    event.b
                ])?;
            }
            Ok(())
        })
    }

    fn held_events(&self, scope: String, query: HeldQuery) -> StoreFuture<'_, Vec<HeldItem>> {
        if query.kinds.is_empty() || query.b.as_ref().is_some_and(Vec::is_empty) {
            return Box::pin(async { Ok(Vec::new()) });
        }
        self.run(move |connection| {
            let mut sql =
                String::from("SELECT created_at, id FROM held WHERE scope = ? AND kind IN (");
            let mut args: Vec<rusqlite::types::Value> = vec![scope.into()];
            push_list(
                &mut sql,
                &mut args,
                query.kinds.iter().map(|kind| i64::from(*kind).into()),
            );
            if let Some(since) = query.since {
                sql.push_str(" AND created_at >= ?");
                args.push(since.into());
            }
            if let Some(until) = query.until {
                sql.push_str(" AND created_at <= ?");
                args.push(until.into());
            }
            if let Some(bases) = query.b {
                sql.push_str(" AND b IN (");
                push_list(&mut sql, &mut args, bases.into_iter().map(Into::into));
            }
            sql.push_str(" ORDER BY created_at, id");
            let mut select = connection.prepare_cached(&sql)?;
            let rows = select.query_map(params_from_iter(args), |row| {
                Ok(HeldItem {
                    created_at: row.get(0)?,
                    id: row.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    fn replay_inputs(&self) -> StoreFuture<'_, HashMap<String, ReplayInput>> {
        self.run(|connection| {
            let tx = connection.transaction()?;
            let checkpoints = {
                let mut select = tx.prepare_cached("SELECT object_id, hash FROM checkpoints")?;
                select.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?
            };
            let mut bases: HashMap<String, Vec<String>> = HashMap::new();
            {
                let mut select = tx.prepare_cached("SELECT object_id, hash FROM bases")?;
                let mut rows = select.query([])?;
                while let Some(row) = rows.next()? {
                    bases.entry(row.get(0)?).or_default().push(row.get(1)?);
                }
            }
            let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
            {
                let mut select = tx.prepare_cached(
                    "SELECT object_id, id FROM changes INDEXED BY changes_object_id WHERE rebased = 0 ORDER BY object_id, seq",
                )?;
                let mut rows = select.query([])?;
                while let Some(row) = rows.next()? {
                    ids.entry(row.get(0)?).or_default().push(row.get(1)?);
                }
            }
            Ok(replay_inputs_from(ids, checkpoints, bases))
        })
    }

    fn cached_replays(
        &self,
        object_ids: Vec<String>,
        engine_version: String,
    ) -> StoreFuture<'_, HashMap<String, CachedReplay>> {
        self.run(move |connection| {
            let mut select = connection.prepare_cached(
                "SELECT change_count, change_digest, checkpoint_hash, base_hashes, state FROM replay_cache WHERE object_id = ? AND engine_version = ?",
            )?;
            let mut rows = HashMap::new();
            for object_id in object_ids {
                let row = select
                    .query_row(params![object_id, engine_version], |row| {
                        Ok(CachedReplay {
                            object_id: object_id.clone(),
                            input: ReplayInput {
                                change_count: row.get(0)?,
                                change_digest: row.get(1)?,
                                checkpoint_hash: row.get(2)?,
                                base_hashes: row.get(3)?,
                            },
                            state: row.get(4)?,
                        })
                    })
                    .optional()?;
                if let Some(row) = row {
                    rows.insert(object_id, row);
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
        if rows.is_empty() {
            return Box::pin(async { Ok(()) });
        }
        self.write(move |tx| {
            let mut upsert = tx.prepare_cached(
                "INSERT OR REPLACE INTO replay_cache (object_id, engine_version, change_count, change_digest, checkpoint_hash, state, base_hashes) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for row in rows {
                upsert.execute(params![
                    row.object_id,
                    engine_version,
                    row.input.change_count,
                    row.input.change_digest,
                    row.input.checkpoint_hash,
                    row.state,
                    row.input.base_hashes
                ])?;
            }
            Ok(())
        })
    }

    fn clear_cached_replays(&self) -> StoreFuture<'_, ()> {
        self.run(|connection| {
            connection.execute("DELETE FROM replay_cache", [])?;
            Ok(())
        })
    }

    fn clear(&self) -> StoreFuture<'_, ()> {
        self.write(|tx| {
            tx.execute_batch(
                "DELETE FROM changes; DELETE FROM meta; DELETE FROM checkpoints; DELETE FROM bases; DELETE FROM held; DELETE FROM replay_cache;",
            )?;
            Ok(())
        })
    }

    fn close(&self) -> StoreFuture<'_, ()> {
        let connection = self.connection.clone();
        Box::pin(blocking::unblock(move || {
            let taken = connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            match taken {
                Some(connection) => connection.close().map_err(|(_, e)| e.into()),
                None => Ok(()),
            }
        }))
    }
}

fn push_list(
    sql: &mut String,
    args: &mut Vec<rusqlite::types::Value>,
    values: impl Iterator<Item = rusqlite::types::Value>,
) {
    for (index, value) in values.enumerate() {
        sql.push_str(if index == 0 { "?" } else { ", ?" });
        args.push(value);
    }
    sql.push(')');
}

//! Database handle: opens a MyHomeLib collection SQLite file, registers the custom UDFs and
//! collation the original schema depends on, and initializes the schema when needed.
//!
//! Why register UDFs/collation at all? The original app creates its tables with triggers that call
//! `MHL_UPPER`, `MHL_FULLNAME` and `MHL_TRIGGERS_ON`, and columns/indexes declared with
//! `COLLATE MHL_SYSTEM_NOCASE`. Existing user `.db` files already contain those triggers, so a
//! Rust process must provide the same functions or any write (or even DDL) fails. Registering them
//! here makes both brand-new collections *and* existing databases behave exactly like the original.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::Connection;
use tracing::info;

use crate::core::search;
use crate::error::Result;

/// The original collection schema, embedded verbatim. `--@@` lines are plain SQL comments.
const SCHEMA: &str = include_str!("../../schema/create_collection_db.sql");

pub struct Db {
    /// Crate-visible so the sibling `repository` module can issue queries.
    pub(crate) conn: Connection,
    /// Mirrors the original `FTriggersEnabled`. When false, `MHL_TRIGGERS_ON()` returns 0 so the
    /// schema's search-column triggers become no-ops (a bulk-import speedup used in later phases).
    triggers_enabled: Arc<AtomicBool>,
}

impl Db {
    /// Open (creating if missing) a collection database at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;

        // The schema references MHL_SYSTEM_NOCASE in column/index definitions, so the collation and
        // UDFs must exist before any DDL that mentions them.
        register_mhl_affinities(&conn);

        let db = Self {
            conn,
            triggers_enabled: Arc::new(AtomicBool::new(true)),
        };
        register_triggers_on_fn(&db);
        db.ensure_schema()?;
        Ok(db)
    }

    /// The underlying connection (for raw queries / advanced use).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Toggle the schema's search-column triggers. Off = faster bulk writes (the DAO still computes
    /// `Search*` columns itself, so correctness is unaffected).
    pub fn set_triggers_enabled(&self, on: bool) {
        self.triggers_enabled.store(on, Ordering::SeqCst);
    }

    /// Idempotent and non-destructive: only runs the create script when the collection is not yet
    /// initialized (the `Books` table is absent). An existing populated DB is never dropped.
    fn ensure_schema(&self) -> Result<()> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Books'",
            [],
            |r| r.get(0),
        )?;

        if exists > 0 {
            info!("existing MyHomeLib collection detected — leaving schema untouched");
            return Ok(());
        }

        info!("initializing new collection from embedded schema");
        // Safe here because the DB is empty: the script's `PRAGMA page_size` and
        // `DROP TABLE IF EXISTS` statements only make sense on a fresh file.
        self.conn.execute_batch(SCHEMA)?;
        Ok(())
    }
}

/// Register the collation + pure UDFs (no shared state). Must run before DDL that references
/// `MHL_SYSTEM_NOCASE`.
fn register_mhl_affinities(conn: &Connection) {
    // Unicode case-insensitive collation. SQLite's built-in NOCASE is ASCII-only; this handles the
    // Cyrillic identifiers the app deals with.
    conn.create_collation("MHL_SYSTEM_NOCASE", |a, b| a.to_lowercase().cmp(&b.to_lowercase()))
        .expect("register MHL_SYSTEM_NOCASE");

    // MHL_UPPER / MHL_LOWER — NULL-propagating (matches SQLite function semantics and the original's
    // use on nullable columns like Books.Lang).
    conn.create_scalar_function(
        "MHL_UPPER",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| match ctx.get::<Option<String>>(0)? {
            Some(s) => Ok(Value::Text(search::upper(&s))),
            None => Ok(Value::Null),
        },
    )
    .expect("register MHL_UPPER");

    conn.create_scalar_function(
        "MHL_LOWER",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| match ctx.get::<Option<String>>(0)? {
            Some(s) => Ok(Value::Text(search::lower(&s))),
            None => Ok(Value::Null),
        },
    )
    .expect("register MHL_LOWER");

    // MHL_FULLNAME — SQLite overloads by arity: 3 args (preserved case) and 4 args (uppercased).
    // NULL name parts are treated as empty strings, matching the original's text16 wrapper.
    conn.create_scalar_function(
        "MHL_FULLNAME",
        3,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let last = ctx.get::<Option<String>>(0)?.unwrap_or_default();
            let first = ctx.get::<Option<String>>(1)?;
            let middle = ctx.get::<Option<String>>(2)?;
            Ok(Value::Text(search::format_name(&last, first.as_deref(), middle.as_deref())))
        },
    )
    .expect("register MHL_FULLNAME/3");

    conn.create_scalar_function(
        "MHL_FULLNAME",
        4,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let last = ctx.get::<Option<String>>(0)?.unwrap_or_default();
            let first = ctx.get::<Option<String>>(1)?;
            let middle = ctx.get::<Option<String>>(2)?;
            // The 4th arg is the original's format flag (only ever passed as 1 by the triggers); accepted
            // and currently ignored.
            let _flag: Option<i64> = ctx.get(3)?;
            Ok(Value::Text(search::upper(&search::format_name(
                &last,
                first.as_deref(),
                middle.as_deref(),
            ))))
        },
    )
    .expect("register MHL_FULLNAME/4");

    // NOTE: MHL_TRIGGERS_ON needs the shared enable-flag and is registered in register_triggers_on_fn.
}

/// Register `MHL_TRIGGERS_ON()` bound to this handle's live trigger flag.
fn register_triggers_on_fn(db: &Db) {
    let flag = Arc::clone(&db.triggers_enabled);
    db.conn
        .create_scalar_function("MHL_TRIGGERS_ON", 0, FunctionFlags::SQLITE_UTF8, move |_ctx| {
            Ok(Value::Integer(if flag.load(Ordering::SeqCst) { 1 } else { 0 }))
        })
        .expect("register MHL_TRIGGERS_ON");
}

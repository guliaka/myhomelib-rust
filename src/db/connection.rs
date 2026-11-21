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
use crate::error::{AppError, Result};

use super::props;

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
        // A background import job writes through its own connection while the GUI connection stays
        // open; wait for a competing lock instead of failing straight away with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(10))?;

        // The schema references MHL_SYSTEM_NOCASE in column/index definitions, so the collation and
        // UDFs must exist before any DDL that mentions them.
        register_mhl_affinities(&conn);

        let mut db = Self {
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
    fn ensure_schema(&mut self) -> Result<()> {
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
        // A new collection starts with the built-in FB2 genre tree (the original's CreateCollection
        // loads it too). Existing collections are never touched by this path.
        self.seed_builtin_genres()?;
        // Stamp the collection metadata exactly like the original's `CreateCollection`: the schema
        // version is what `IsValidCollection` checks before the original agrees to attach a file.
        self.set_prop_str(props::PROP_SCHEMA_VERSION, props::DATABASE_VERSION)?;
        self.set_prop_int(props::PROP_CODE, props::CT_PRIVATE_FB)?;
        self.set_prop_str(props::PROP_CREATIONDATE, &props::now_string())?;
        Ok(())
    }

    // ------------------------------------------------------------------ transactions

    /// Start an explicit write transaction (the original's `BeginBulkOperation`). Pair with
    /// [`Self::commit`] / [`Self::rollback`]. Used by the bulk importers, which insert through `&self`
    /// methods so a whole archive costs one commit instead of one per book.
    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }

    // ------------------------------------------------------------------ collection properties

    /// Read a collection property from the `Settings` table as text (integers are rendered in decimal).
    /// `None` when the property is absent or NULL.
    pub fn get_prop_str(&self, prop_id: i64) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let v: Option<Value> = self
            .conn
            .query_row("SELECT SettingValue FROM Settings WHERE SettingID = ?1", [prop_id], |r| r.get(0))
            .optional()?;
        Ok(match v {
            Some(Value::Text(s)) => Some(s),
            Some(Value::Integer(i)) => Some(i.to_string()),
            Some(Value::Real(f)) => Some(f.to_string()),
            Some(Value::Blob(b)) => Some(String::from_utf8_lossy(&b).into_owned()),
            Some(Value::Null) | None => None,
        })
    }

    pub fn get_prop_int(&self, prop_id: i64) -> Result<Option<i64>> {
        Ok(self.get_prop_str(prop_id)?.and_then(|s| s.trim().parse().ok()))
    }

    /// Write a string property (delete + insert, like the original's `SetProperty`).
    pub fn set_prop_str(&self, prop_id: i64, value: &str) -> Result<()> {
        self.conn.execute("DELETE FROM Settings WHERE SettingID = ?1", [prop_id])?;
        self.conn.execute(
            "INSERT INTO Settings (SettingID, SettingValue) VALUES (?1, ?2)",
            rusqlite::params![prop_id, value],
        )?;
        Ok(())
    }

    pub fn set_prop_int(&self, prop_id: i64, value: i64) -> Result<()> {
        self.conn.execute("DELETE FROM Settings WHERE SettingID = ?1", [prop_id])?;
        self.conn.execute(
            "INSERT INTO Settings (SettingID, SettingValue) VALUES (?1, ?2)",
            rusqlite::params![prop_id, value],
        )?;
        Ok(())
    }

    /// The collection type code (`CT_*`); private FB2 when the property is missing.
    pub fn collection_code(&self) -> i64 {
        self.get_prop_int(props::PROP_CODE).ok().flatten().unwrap_or(props::CT_PRIVATE_FB)
    }

    /// Create a brand-new collection file at `path` with the given type code and (optionally) a custom
    /// genre list in the original's `.glst` format. Fails if the file already exists — a collection is
    /// never silently overwritten.
    pub fn create_collection<P: AsRef<Path>>(path: P, code: i64, genres_file: Option<&Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            return Err(AppError::Other(format!("collection file already exists: {}", path.display())));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut db = Self::open(path)?;
        db.set_prop_int(props::PROP_CODE, code)?;
        if let Some(gf) = genres_file {
            let text = std::fs::read_to_string(gf)
                .map_err(|e| AppError::Other(format!("read genre file {}: {e}", gf.display())))?;
            let rows = crate::genres::parse_genre_lines(&text);
            db.replace_genres(&rows)?;
        }
        Ok(db)
    }

    /// Port of the original's `IsValidCollection`: `Some(type code)` when `path` is a collection this
    /// app can attach. A file with the collection tables but no schema-version stamp (written by an
    /// earlier build of this port) is accepted and stamped, so it can be attached too.
    pub fn probe_collection<P: AsRef<Path>>(path: P) -> Option<i64> {
        let path = path.as_ref();
        if !path.is_file() {
            return None;
        }
        // Read-only + no schema init: probing must never create or alter a foreign file.
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let has_books: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('Books','Settings')",
                [],
                |r| r.get(0),
            )
            .ok()?;
        if has_books < 2 {
            return None;
        }
        let version: Option<String> = conn
            .query_row(
                "SELECT CAST(SettingValue AS TEXT) FROM Settings WHERE SettingID = ?1",
                [props::PROP_SCHEMA_VERSION],
                |r| r.get(0),
            )
            .ok();
        drop(conn);
        match version {
            Some(v) if v == props::DATABASE_VERSION => {}
            Some(_) => return None, // a different (incompatible) schema generation
            None => {
                let db = Self::open(path).ok()?;
                db.set_prop_str(props::PROP_SCHEMA_VERSION, props::DATABASE_VERSION).ok()?;
            }
        }
        let db = Self::open(path).ok()?;
        Some(db.collection_code())
    }
}

/// Register the collation + pure UDFs (no shared state). Must run before DDL that references
/// `MHL_SYSTEM_NOCASE`.
pub(crate) fn register_mhl_affinities(conn: &Connection) {
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

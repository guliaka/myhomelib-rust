//! System database — the registry of all collections (the original's `TSystemData` /
//! `unit_SystemDatabase_SQLite.pas`). One extra SQLite file (default `MyHomeLib.system.db` next to
//! the executable, overridable via `MYHOMELIB_SYSTEM_DB`) holds a `Bases` row per collection plus the
//! cross-collection `Books`/`Groups`/`BookGroups` user-data tables, exactly as the original's
//! `CreateSystemDB_SQLite.sql` does — so an existing original system DB opens unchanged and a DB we
//! create is usable by the original app too.
//!
//! The `Bases` trigger (`TRBases_BD`) cascades deletes into the user-data tables, matching the
//! original. Collection maintenance (create/repair/compact/delete) goes through [`crate::db::Db`]
//! + [`SystemDb`]: the registry points at the `.db` file; the file itself is a normal collection.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::connection::register_mhl_affinities;
use crate::error::{AppError, Result};

/// The original's system-DB create script, embedded verbatim (BOM/`--@@` separators stripped).
const SYSTEM_SCHEMA: &str = include_str!("../schema/create_system_db.sql");

/// One `Bases` row — the original's `TCollectionInfo`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectionInfo {
    /// `Bases.DatabaseID` (0 = not yet stored).
    pub id: i64,
    /// `BaseName` — the user-visible collection name (unique).
    pub name: String,
    /// `RootFolder` — where the collection's book files live.
    pub root_folder: String,
    /// `DBFileName` — path to the collection `.db` file.
    pub db_file_name: String,
    /// `Notes`.
    pub notes: String,
    /// `LibUser` / `LibPassword` — Librusec credentials for this collection.
    pub lib_user: String,
    pub lib_password: String,
    /// `DataVersion` — collection schema version.
    pub data_version: i64,
    /// `Code` — the original's `COLLECTION_TYPE` bitmask (local/online, FB2/non-FB2).
    pub code: i64,
    /// `URL` — Librusec mirror URL (online collections).
    pub url: String,
    /// `ConnectionScript` — Librusec connection script (online collections).
    pub connection_script: String,
    /// `CreationDate` — stored as the original's `yyyy-mm-dd hh:nn:ss` text.
    pub creation_date: String,
}

impl CollectionInfo {
    /// The original's local/online bit (`CT_LOCATION_MASK = $08000000`).
    pub fn is_online(&self) -> bool {
        self.code & 0x0800_0000 != 0
    }

    /// True when the registered `.db` file exists on disk.
    pub fn file_exists(&self) -> bool {
        Path::new(&self.db_file_name).is_file()
    }
}

/// Handle to the system database.
pub struct SystemDb {
    conn: Connection,
}

impl SystemDb {
    /// Open (creating if missing) the system DB at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        // The system schema declares MHL_SYSTEM_NOCASE collations, so the same affinities must be
        // registered before any DDL — an existing original system DB may also reference the UDFs.
        register_mhl_affinities(&conn);
        let db = Self { conn };
        db.ensure_schema()?;
        Ok(db)
    }

    /// Default system-DB location: `$MYHOMELIB_SYSTEM_DB` if set, else `MyHomeLib.system.db`
    /// next to the executable (the original keeps its data beside the app), else in the cwd.
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var_os("MYHOMELIB_SYSTEM_DB") {
            return PathBuf::from(p);
        }
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("MyHomeLib.system.db")))
            .unwrap_or_else(|| PathBuf::from("MyHomeLib.system.db"))
    }

    /// Idempotent: create the schema only when the `Bases` table is absent.
    fn ensure_schema(&self) -> Result<()> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Bases'",
            [],
            |r| r.get(0),
        )?;
        if exists == 0 {
            self.conn.execute_batch(SYSTEM_SCHEMA)?;
        }
        Ok(())
    }

    /// All registered collections, oldest first (the original's collection-info iterator).
    pub fn list(&self) -> Result<Vec<CollectionInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT DatabaseID, BaseName, RootFolder, DBFileName, DataVersion, Code,
                    LibUser, LibPassword, Notes, CreationDate, URL, ConnectionScript
             FROM Bases ORDER BY DatabaseID",
        )?;
        let rows = stmt
            .query_map([], row_to_info)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Fetch one collection by id.
    pub fn get(&self, id: i64) -> Result<Option<CollectionInfo>> {
        self.conn
            .query_row(
                "SELECT DatabaseID, BaseName, RootFolder, DBFileName, DataVersion, Code,
                        LibUser, LibPassword, Notes, CreationDate, URL, ConnectionScript
                 FROM Bases WHERE DatabaseID = ?1",
                [id],
                row_to_info,
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Find a collection by its exact name (unique, case-insensitive per the schema collation).
    pub fn find_by_name(&self, name: &str) -> Result<Option<CollectionInfo>> {
        self.conn
            .query_row(
                "SELECT DatabaseID, BaseName, RootFolder, DBFileName, DataVersion, Code,
                        LibUser, LibPassword, Notes, CreationDate, URL, ConnectionScript
                 FROM Bases WHERE BaseName = ?1",
                [name],
                row_to_info,
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Register a new collection; returns its `DatabaseID`. `creation_date` defaults to now
    /// (local time, `yyyy-mm-dd hh:mm:ss` — the original's format).
    pub fn add(&self, info: &CollectionInfo) -> Result<i64> {
        if self.find_by_name(&info.name)?.is_some() {
            return Err(AppError::msg(format!("collection “{}” already exists", info.name)));
        }
        let created = if info.creation_date.is_empty() {
            now_stamp()
        } else {
            info.creation_date.clone()
        };
        self.conn.execute(
            "INSERT INTO Bases
               (BaseName, RootFolder, DBFileName, CreationDate, DataVersion, Code,
                LibUser, LibPassword, Notes, URL, ConnectionScript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                info.name,
                info.root_folder,
                info.db_file_name,
                created,
                info.data_version,
                info.code,
                info.lib_user,
                info.lib_password,
                info.notes,
                info.url,
                info.connection_script,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update an existing collection's editable fields (the original's property dialog).
    pub fn update(&self, info: &CollectionInfo) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE Bases SET
               BaseName = ?2, RootFolder = ?3, DBFileName = ?4, Notes = ?5,
               LibUser = ?6, LibPassword = ?7, DataVersion = ?8, Code = ?9,
               URL = ?10, ConnectionScript = ?11
             WHERE DatabaseID = ?1",
            params![
                info.id,
                info.name,
                info.root_folder,
                info.db_file_name,
                info.notes,
                info.lib_user,
                info.lib_password,
                info.data_version,
                info.code,
                info.url,
                info.connection_script,
            ],
        )?;
        if n == 0 {
            return Err(AppError::msg(format!("collection #{} not found", info.id)));
        }
        Ok(())
    }

    /// Remove a collection from the registry (cascades its user-data rows via the schema trigger).
    /// Does NOT touch the `.db` file itself — the caller decides whether to purge files.
    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM Bases WHERE DatabaseID = ?1", [id])?;
        Ok(())
    }

    /// The first registered collection whose `.db` file still exists (the original's
    /// `FindFirstExistingCollectionID` — what the app auto-opens at startup).
    pub fn first_existing(&self) -> Result<Option<CollectionInfo>> {
        for c in self.list()? {
            if c.file_exists() {
                return Ok(Some(c));
            }
        }
        Ok(None)
    }

    // -- groups (the original's cross-collection reading groups, stored in the system DB) ------

    /// All groups (id, name), oldest first.
    pub fn list_groups(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT GroupID, GroupName FROM Groups ORDER BY GroupID")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find or create a group by name; returns its id (the original's `ImportUserData`).
    pub fn upsert_group(&self, name: &str) -> Result<i64> {
        match self.find_group_by_name(name)? {
            Some((id, _)) => Ok(id),
            None => self.add_group(name, true),
        }
    }

    /// Put a book into a group for a given collection. Ensures the system-DB `Books` row exists
    /// first (the original inserts it in `CopyBookToGroup`), then the `BookGroups` link.
    /// Idempotent — re-adding is a no-op.
    pub fn add_group_book(
        &self,
        group_id: i64,
        book_id: i64,
        database_id: i64,
        libid: &str,
        title: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO Books \
             (BookID, DatabaseID, LibID, Title, UpdateDate, LibRate, FileName, InsideNo, IsLocal, IsDeleted, Rate, Progress) \
             VALUES (?1, ?2, ?3, ?4, '1970-01-01 00:00:00', 0, '', 0, 0, 0, 0, 0)",
            params![book_id, database_id, libid, title],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO BookGroups (GroupID, BookID, DatabaseID) VALUES (?1, ?2, ?3)",
            params![group_id, book_id, database_id],
        )?;
        Ok(())
    }

    /// Create a group with an explicit `AllowDelete` flag. The original's built-ins
    /// («Избранное», «К прочтению») are `AllowDelete = 0` and can't be deleted.
    pub fn add_group(&self, name: &str, allow_delete: bool) -> Result<i64> {
        if self.find_group_by_name(name)?.is_some() {
            return Err(AppError::msg(format!("group “{name}” already exists")));
        }
        self.conn.execute(
            "INSERT INTO Groups (GroupName, AllowDelete) VALUES (?1, ?2)",
            params![name, allow_delete as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Find a group by exact name.
    pub fn find_group_by_name(&self, name: &str) -> Result<Option<(i64, bool)>> {
        // (GroupID, AllowDelete)
        self.conn
            .query_row(
                "SELECT GroupID, AllowDelete FROM Groups WHERE GroupName = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Rename a group.
    pub fn rename_group(&self, group_id: i64, new_name: &str) -> Result<()> {
        let n = self
            .conn
            .execute("UPDATE Groups SET GroupName = ?2 WHERE GroupID = ?1", params![group_id, new_name])?;
        if n == 0 {
            return Err(AppError::msg(format!("group #{group_id} not found")));
        }
        Ok(())
    }

    /// Delete a group and all its memberships. Refuses protected groups (`AllowDelete = 0`,
    /// the original's InternalClearGroup/DELETE only removes deletable ones).
    pub fn delete_group(&self, group_id: i64) -> Result<()> {
        let deletable: Option<i64> = self
            .conn
            .query_row("SELECT AllowDelete FROM Groups WHERE GroupID = ?1", [group_id], |r| r.get(0))
            .optional()?;
        match deletable {
            Some(1) => {
                self.conn.execute("DELETE FROM BookGroups WHERE GroupID = ?1", [group_id])?;
                self.conn.execute("DELETE FROM Groups WHERE GroupID = ?1", [group_id])?;
                Ok(())
            }
            Some(_) => Err(AppError::msg("эту группу нельзя удалить")),
            None => Err(AppError::msg(format!("group #{group_id} not found"))),
        }
    }

    /// Remove all book memberships from a group (the original's ClearGroup) — the group stays.
    pub fn clear_group(&self, group_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM BookGroups WHERE GroupID = ?1", [group_id])?;
        Ok(())
    }

    /// Remove one book from a group (for this collection).
    pub fn remove_group_book(&self, group_id: i64, book_id: i64, database_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM BookGroups WHERE GroupID = ?1 AND BookID = ?2 AND DatabaseID = ?3",
            params![group_id, book_id, database_id],
        )?;
        Ok(())
    }

    /// The original seeds two protected groups on a fresh system DB («Избранное» / «К прочтению»).
    /// Idempotent: only runs when the Groups table is empty.
    pub fn ensure_default_groups(&self) -> Result<()> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM Groups", [], |r| r.get(0))?;
        if n == 0 {
            self.add_group("Избранное", false)?;
            self.add_group("К прочтению", false)?;
        }
        Ok(())
    }

    /// Groups with their member books for one collection: `(group_id, group_name, book_id, libid)`
    /// (the original's group + book iterators restricted to `DatabaseID`).
    pub fn list_group_books(&self, database_id: i64) -> Result<Vec<(i64, String, i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.GroupID, g.GroupName, b.BookID, b.LibID \
             FROM Groups g \
             JOIN BookGroups bg ON bg.GroupID = g.GroupID \
             JOIN Books b ON b.BookID = bg.BookID AND b.DatabaseID = bg.DatabaseID \
             WHERE bg.DatabaseID = ?1 ORDER BY g.GroupID, b.BookID",
        )?;
        let rows = stmt
            .query_map([database_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete a collection's `.db` file and its `<db>.ini` settings. The registry row must already
    /// be gone (or removed by the caller in the same operation).
    pub fn purge_files(db_file_name: &str) -> Result<()> {
        let db_path = Path::new(db_file_name);
        std::fs::remove_file(db_path).map_err(|e| AppError::msg(format!("delete {db_file_name}: {e}")))?;
        let ini = crate::settings::Settings::path_for(db_path);
        if ini.is_file() {
            let _ = std::fs::remove_file(&ini);
        }
        Ok(())
    }
}

fn row_to_info(r: &rusqlite::Row<'_>) -> rusqlite::Result<CollectionInfo> {
    Ok(CollectionInfo {
        id: r.get(0)?,
        name: r.get(1)?,
        root_folder: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        db_file_name: r.get(3)?,
        data_version: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        code: r.get(5)?,
        lib_user: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        lib_password: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        notes: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        creation_date: r.get(9)?,
        url: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        connection_script: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
    })
}

/// Local-time `yyyy-mm-dd hh:mm:ss` (the original's `NowDateTimeToStr`).
fn now_stamp() -> String {
    let now = chrono::Local::now();
    now.format("%Y-%m-%d %H:%M:%S").to_string()
}

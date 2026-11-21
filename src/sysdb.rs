//! The system ("user") database — the port of the original's `TSystemData_SQLite`
//! (`Program/DAO/SQLite/unit_SystemDatabase_SQLite.pas`, file `user.dbs2`).
//!
//! It is the registry of collections (`Bases`) plus the cross-collection user data: book groups
//! (`Groups` / `BookGroups`, with the two built-in groups «Избранное» and «К прочтению») and the
//! copies of grouped books (`Books`). The schema is the original's, embedded verbatim, so an existing
//! `user.dbs2` opens unchanged.
//!
//! Paths: the original stores collection paths relative to its data folder when possible. Here the
//! base is the folder holding the system database — relative paths are resolved against it on read,
//! and a path under it is stored relative on write (so a portable install keeps working when moved).

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::{connection::register_mhl_affinities, props, Db};
use crate::error::{AppError, Result};

/// The original's `SYSTEM_DATABASE_FILENAME`.
pub const SYSTEM_DATABASE_FILENAME: &str = "user.dbs2";
/// The original's `COLLECTION_EXTENSION`.
pub const COLLECTION_EXTENSION: &str = "hlc2";

pub const FAVORITES_GROUP_NAME: &str = "Избранное";
pub const TO_READ_GROUP_NAME: &str = "К прочтению";

const SCHEMA: &str = include_str!("../schema/create_system_db.sql");

/// One registered collection (a `Bases` row, the original's `TCollectionInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionInfo {
    pub id: i64,
    pub name: String,
    /// Absolute folder holding the collection's book files / archives.
    pub root_folder: PathBuf,
    /// Absolute path of the collection database file.
    pub db_file: PathBuf,
    /// Collection type code (`props::CT_*`).
    pub code: i64,
    pub data_version: Option<i64>,
    pub notes: String,
    pub user: String,
    pub password: String,
    pub url: String,
    pub script: String,
}

/// A book group (a `Groups` row).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    /// False for the built-in groups, which can be cleared but not deleted.
    pub allow_delete: bool,
}

pub struct SystemDb {
    conn: Connection,
    /// Folder of the system database — the base for relative collection paths.
    base: PathBuf,
}

impl SystemDb {
    /// Open the system database at `path`, creating it (schema + the two built-in groups) if missing.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        register_mhl_affinities(&conn);

        let base = std::path::absolute(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let sys = Self { conn, base };

        let has_bases: i64 = sys.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Bases'",
            [],
            |r| r.get(0),
        )?;
        if has_bases == 0 {
            // Fresh file only: the script starts with DROP TABLE IF EXISTS.
            sys.conn.execute_batch(&format!("BEGIN;\n{SCHEMA}\nCOMMIT;"))?;
            sys.add_group(FAVORITES_GROUP_NAME, false)?;
            sys.add_group(TO_READ_GROUP_NAME, false)?;
        }
        Ok(sys)
    }

    /// Folder against which relative collection paths are resolved.
    pub fn base_dir(&self) -> &Path {
        &self.base
    }

    fn expand(&self, stored: &str) -> PathBuf {
        // Stored by the original on Windows, so accept either separator.
        let native = if cfg!(windows) { stored.to_string() } else { stored.replace('\\', "/") };
        let p = PathBuf::from(native);
        if p.is_absolute() || is_windows_absolute(stored) {
            p
        } else {
            self.base.join(p)
        }
    }

    fn to_stored(&self, path: &Path) -> String {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        match abs.strip_prefix(&self.base) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
            _ => abs.to_string_lossy().into_owned(),
        }
    }

    // ------------------------------------------------------------------ collections

    pub fn collections(&self) -> Result<Vec<CollectionInfo>> {
        let mut st = self.conn.prepare(&format!("{SELECT_BASE} ORDER BY bs.BaseName"))?;
        let rows = st.query_map([], |r| self.row_to_info(r))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn collection(&self, id: i64) -> Result<Option<CollectionInfo>> {
        Ok(self
            .conn
            .query_row(&format!("{SELECT_BASE} WHERE bs.DatabaseID = ?1"), [id], |r| self.row_to_info(r))
            .optional()?)
    }

    fn row_to_info(&self, r: &rusqlite::Row) -> rusqlite::Result<CollectionInfo> {
        let text = |i: usize| -> rusqlite::Result<String> {
            Ok(match r.get_ref(i)? {
                rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                    String::from_utf8_lossy(t).into_owned()
                }
                rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                rusqlite::types::ValueRef::Real(f) => f.to_string(),
                rusqlite::types::ValueRef::Null => String::new(),
            })
        };
        Ok(CollectionInfo {
            id: r.get(0)?,
            name: text(1)?,
            root_folder: self.expand(&text(2)?),
            db_file: self.expand(&text(3)?),
            code: r.get(4)?,
            data_version: r.get(5)?,
            notes: text(6)?,
            user: text(7)?,
            password: text(8)?,
            url: text(9)?,
            script: text(10)?,
        })
    }

    /// The original's `HasCollectionWithProp(PROP_DISPLAYNAME, …)`.
    pub fn has_collection_named(&self, name: &str, ignore_id: Option<i64>) -> Result<bool> {
        Ok(self.collections()?.iter().any(|c| c.name == name && Some(c.id) != ignore_id))
    }

    /// The original's `HasCollectionWithProp(PROP_DATAFILE, …)`.
    pub fn has_collection_file(&self, db_file: &Path, ignore_id: Option<i64>) -> Result<bool> {
        let want = std::path::absolute(db_file).unwrap_or_else(|_| db_file.to_path_buf());
        Ok(self.collections()?.iter().any(|c| c.db_file == want && Some(c.id) != ignore_id))
    }

    fn check_new(&self, name: &str, db_file: &Path) -> Result<()> {
        if name.trim().is_empty() {
            return Err(AppError::Other("не задано название коллекции".into()));
        }
        if self.has_collection_named(name, None)? {
            return Err(AppError::Other(format!("коллекция с названием «{name}» уже существует")));
        }
        if self.has_collection_file(db_file, None)? {
            return Err(AppError::Other(format!("файл {} уже подключён как коллекция", db_file.display())));
        }
        Ok(())
    }

    fn insert_base(&self, name: &str, root: &Path, db_file: &Path, code: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO Bases (BaseName, RootFolder, DBFileName, CreationDate, Code) VALUES (?1,?2,?3,?4,?5)",
            params![name, self.to_stored(root), self.to_stored(db_file), props::now_string(), code],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Create a new, empty collection file and register it (the original's `CreateCollection`).
    /// If registration fails the freshly created file is removed again.
    pub fn create_collection(
        &self,
        name: &str,
        root: &Path,
        db_file: &Path,
        code: i64,
        genres_file: Option<&Path>,
    ) -> Result<i64> {
        self.check_new(name, db_file)?;
        drop(Db::create_collection(db_file, code, genres_file)?);
        match self.insert_base(name, root, db_file, code) {
            Ok(id) => Ok(id),
            Err(e) => {
                let _ = std::fs::remove_file(db_file);
                Err(e)
            }
        }
    }

    /// Attach an existing collection file (the original's `RegisterCollection`). The collection type
    /// is read from the file itself.
    pub fn register_collection(&self, db_file: &Path, name: &str, root: &Path) -> Result<i64> {
        self.check_new(name, db_file)?;
        let code = Db::probe_collection(db_file).ok_or_else(|| {
            AppError::Other(format!("Файл {} не является коллекцией MyHomeLib", db_file.display()))
        })?;
        self.insert_base(name, root, db_file, code)
    }

    /// Unregister a collection; the `Bases` delete trigger also drops its grouped-book copies. With
    /// `remove_from_disk` the collection database file is deleted too (book files are never touched).
    pub fn delete_collection(&self, id: i64, remove_from_disk: bool) -> Result<()> {
        let info = self.collection(id)?;
        self.conn.execute("DELETE FROM Bases WHERE DatabaseID = ?1", [id])?;
        if remove_from_disk {
            if let Some(info) = info {
                if info.db_file.is_file() {
                    std::fs::remove_file(&info.db_file)?;
                }
            }
        }
        Ok(())
    }

    /// Update the editable properties of a registered collection (the collection-properties dialog).
    #[allow(clippy::too_many_arguments)]
    pub fn update_collection(
        &self,
        id: i64,
        name: &str,
        root: &Path,
        notes: &str,
        url: &str,
        user: &str,
        password: &str,
        script: &str,
    ) -> Result<()> {
        if name.trim().is_empty() {
            return Err(AppError::Other("не задано название коллекции".into()));
        }
        if self.has_collection_named(name, Some(id))? {
            return Err(AppError::Other(format!("коллекция с названием «{name}» уже существует")));
        }
        self.conn.execute(
            "UPDATE Bases SET BaseName=?2, RootFolder=?3, Notes=?4, URL=?5, LibUser=?6, LibPassword=?7, \
             ConnectionScript=?8 WHERE DatabaseID=?1",
            params![id, name, self.to_stored(root), notes, url, user, password, script],
        )?;
        Ok(())
    }

    /// Mirror the data version stored in the collection into the registry (shown in the list and
    /// used by the online-update check).
    pub fn set_data_version(&self, id: i64, version: Option<i64>) -> Result<()> {
        self.conn.execute("UPDATE Bases SET DataVersion=?2 WHERE DatabaseID=?1", params![id, version])?;
        Ok(())
    }

    // ------------------------------------------------------------------ groups

    pub fn groups(&self) -> Result<Vec<Group>> {
        let mut st = self.conn.prepare("SELECT GroupID, GroupName, AllowDelete FROM Groups ORDER BY GroupID")?;
        let rows = st.query_map([], |r| {
            Ok(Group { id: r.get(0)?, name: r.get(1)?, allow_delete: r.get::<_, i64>(2)? != 0 })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Add a group. `Ok(false)` when a group with that name already exists (the original's `AddGroup`).
    pub fn add_group(&self, name: &str, allow_delete: bool) -> Result<bool> {
        let name = name.trim();
        if name.is_empty() || self.find_group(name)?.is_some() {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO Groups (GroupName, AllowDelete) VALUES (?1, ?2)",
            params![name, allow_delete as i64],
        )?;
        Ok(true)
    }

    fn find_group(&self, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT GroupID FROM Groups WHERE GroupName = ?1", [name], |r| r.get(0))
            .optional()?)
    }

    /// Rename a group. `Ok(false)` when the new name is empty or already taken.
    pub fn rename_group(&self, id: i64, new_name: &str) -> Result<bool> {
        let new_name = new_name.trim();
        if new_name.is_empty() || self.find_group(new_name)?.is_some_and(|other| other != id) {
            return Ok(false);
        }
        self.conn.execute("UPDATE Groups SET GroupName=?2 WHERE GroupID=?1", params![id, new_name])?;
        Ok(true)
    }

    /// Remove every book from a group, then drop grouped-book copies no group references any more.
    pub fn clear_group(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM BookGroups WHERE GroupID = ?1", [id])?;
        self.remove_unused_books()
    }

    /// Delete a group and its memberships. Built-in groups are only cleared. `Ok(true)` if deleted.
    pub fn delete_group(&self, id: i64) -> Result<bool> {
        self.clear_group(id)?;
        let n = self.conn.execute("DELETE FROM Groups WHERE GroupID = ?1 AND AllowDelete = 1", [id])?;
        Ok(n > 0)
    }

    /// The original's `RemoveUnusedBooks`.
    fn remove_unused_books(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM Books WHERE NOT EXISTS \
             (SELECT 1 FROM BookGroups bg WHERE bg.BookID = Books.BookID AND bg.DatabaseID = Books.DatabaseID)",
            [],
        )?;
        Ok(())
    }
}

const SELECT_BASE: &str = "SELECT bs.DatabaseID, bs.BaseName, bs.RootFolder, bs.DBFileName, bs.Code, \
     bs.DataVersion, bs.Notes, bs.LibUser, bs.LibPassword, bs.URL, bs.ConnectionScript FROM Bases bs";

/// `C:\…` or `\\server\share\…` — absolute on Windows even when this build runs elsewhere.
fn is_windows_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || s.starts_with("\\\\")
}

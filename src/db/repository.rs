//! Data-access methods on [`Db`]: CRUD for Books/Authors/Series/Genres, full search across the
//! uppercased `Search*` columns, and pagination. All writes compute the `Search*` columns in-app
//! (via [`crate::core::search`]) so they are correct whether or not the schema's triggers fire.

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, OptionalExtension, ToSql};

use super::connection::Db;
use crate::core::{search, Author, Book, Genre, Series};
use crate::error::Result;

/// The `Books` columns in a fixed order that [`row_to_book`] reads by index. `alias` prefixes each
/// column (empty for single-table queries, "b" for joined search queries).
fn book_cols(alias: &str) -> String {
    let p = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    format!(
        "{p}BookID, {p}LibID, {p}Title, {p}SeriesID, {p}SeqNumber, {p}UpdateDate, {p}LibRate, \
         {p}Lang, {p}Folder, {p}FileName, {p}InsideNo, {p}Ext, {p}BookSize, {p}IsLocal, \
         {p}IsDeleted, {p}KeyWords, {p}Rate, {p}Progress, {p}Annotation, {p}Review"
    )
}

fn row_to_book(r: &rusqlite::Row) -> rusqlite::Result<Book> {
    Ok(Book {
        book_id: r.get(0)?,
        lib_id: r.get(1)?,
        title: r.get(2)?,
        series_id: r.get(3)?,
        seq_number: r.get(4)?,
        update_date: r.get(5)?,
        lib_rate: r.get(6)?,
        lang: r.get(7)?,
        folder: r.get(8)?,
        file_name: r.get(9)?,
        inside_no: r.get(10)?,
        ext: r.get(11)?,
        book_size: r.get(12)?,
        is_local: r.get::<_, i32>(13)? != 0,
        is_deleted: r.get::<_, i32>(14)? != 0,
        key_words: r.get(15)?,
        rate: r.get(16)?,
        progress: r.get(17)?,
        annotation: r.get(18)?,
        review: r.get(19)?,
    })
}

/// Filters for [`Db::search_books`]. All `*_like` fields are matched case-insensitively against the
/// uppercased `Search*` columns (the value is wrapped in `%…%`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookQuery {
    pub title_like: Option<String>,
    pub author_name_like: Option<String>,
    pub lang: Option<String>,
    pub ext: Option<String>,
    /// Only books that have a physical file on disk.
    pub only_local: bool,
    /// When false (default), soft-deleted books are excluded.
    pub include_deleted: bool,
}

/// A tree-node scope for listing books (mirrors [`crate::gui::state::Selection`]). `All` = the whole
/// collection. This is what the virtualized book list pages through with `LIMIT/OFFSET`.
#[derive(Debug, Clone, PartialEq)]
pub enum BookScope {
    All,
    Author(i64),
    Genre(String),
    Series(i64),
    /// A folder; `None` = the collection root.
    Folder(Option<String>),
}

impl Db {
    // ------------------------------------------------------------------ Authors

    /// Insert or return the existing author. The original has no unique constraint on authors, so
    /// dedup happens here on (Last, First, Middle). Returns the `AuthorID`.
    pub fn upsert_author(&mut self, last: &str, first: Option<&str>, middle: Option<&str>) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT AuthorID FROM Authors \
                 WHERE LastName=?1 AND IFNULL(FirstName,'')=IFNULL(?2,'') AND IFNULL(MiddleName,'')=IFNULL(?3,'')",
                params![last, first, middle],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }

        let search_name = search::upper(&search::format_name(last, first, middle));
        self.conn.execute(
            "INSERT INTO Authors (LastName, FirstName, MiddleName, SearchName) VALUES (?1,?2,?3,?4)",
            params![last, first, middle, search_name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_author(&self, id: i64) -> Result<Option<Author>> {
        let out = self
            .conn
            .query_row(
                "SELECT AuthorID, LastName, FirstName, MiddleName FROM Authors WHERE AuthorID=?1",
                params![id],
                |r| {
                    Ok(Author {
                        author_id: r.get(0)?,
                        last_name: r.get(1)?,
                        first_name: r.get(2)?,
                        middle_name: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(out)
    }

    // ------------------------------------------------------------------ Series

    /// Insert or return the existing series (dedup on case-insensitive title). Returns `SeriesID`.
    pub fn upsert_series(&mut self, title: &str) -> Result<i64> {
        let key = search::upper(title);
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT SeriesID FROM Series WHERE SearchSeriesTitle=?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO Series (SeriesTitle, SearchSeriesTitle) VALUES (?1,?2)",
            params![title, key],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_series(&self, id: i64) -> Result<Option<Series>> {
        let out = self
            .conn
            .query_row(
                "SELECT SeriesID, SeriesTitle FROM Series WHERE SeriesID=?1",
                params![id],
                |r| Ok(Series { series_id: r.get(0)?, series_title: r.get(1)? }),
            )
            .optional()?;
        Ok(out)
    }

    // ------------------------------------------------------------------ Genres

    /// Insert or replace a genre row (keyed by `genre_code`).
    pub fn upsert_genre(&mut self, code: &str, parent: Option<&str>, fb2: Option<&str>, alias: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO Genres (GenreCode, ParentCode, FB2Code, GenreAlias) VALUES (?1,?2,?3,?4)",
            params![code, parent, fb2, alias],
        )?;
        Ok(())
    }

    pub fn list_genres(&self) -> Result<Vec<Genre>> {
        let mut stmt = self.conn.prepare("SELECT GenreCode, ParentCode, FB2Code, GenreAlias FROM Genres ORDER BY GenreAlias")?;
        let rows = stmt.query_map([], |r| {
            Ok(Genre {
                genre_code: r.get(0)?,
                parent_code: r.get(1)?,
                fb2_code: r.get(2)?,
                genre_alias: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------ Books

    /// Insert-or-replace a set of genre rows (idempotent "update": adds missing rows, refreshes
    /// existing ones by `genre_code`). Returns the number of rows written.
    pub fn upsert_genres(&mut self, rows: &[Genre]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        for g in rows {
            tx.execute(
                "INSERT OR REPLACE INTO Genres (GenreCode, ParentCode, FB2Code, GenreAlias) VALUES (?1,?2,?3,?4)",
                params![g.genre_code, g.parent_code, g.fb2_code, g.genre_alias],
            )?;
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// Replace the whole genre catalog from `rows` (mirrors the original `ReloadGenres`): clear
    /// `Genres`, load the new set, then drop dangling `Genre_List` links whose code matches neither a
    /// remaining `GenreCode` nor an `FB2Code`. Returns the number of rows written.
    pub fn replace_genres(&mut self, rows: &[Genre]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM Genres", [])?;
        for g in rows {
            tx.execute(
                "INSERT INTO Genres (GenreCode, ParentCode, FB2Code, GenreAlias) VALUES (?1,?2,?3,?4)",
                params![g.genre_code, g.parent_code, g.fb2_code, g.genre_alias],
            )?;
        }
        tx.execute(
            "DELETE FROM Genre_List WHERE GenreCode NOT IN (SELECT GenreCode FROM Genres) \
             AND GenreCode NOT IN (SELECT FB2Code FROM Genres WHERE FB2Code IS NOT NULL)",
            [],
        )?;
        tx.commit()?;
        Ok(rows.len())
    }

    /// Seed the embedded built-in FB2 genre tree (idempotent upsert). Returns rows written.
    pub fn seed_builtin_genres(&mut self) -> Result<usize> {
        let rows = crate::genres::parse_genre_lines(crate::genres::BUILTIN_GENRES);
        self.upsert_genres(&rows)
    }

    /// Ensure a genre exists for `code` (matched by `GenreCode` or `FB2Code`); insert it if missing.
    /// Used by import `--register-genres` to grow the catalog from genres found in imported books.
    /// Returns `true` when a new row was inserted.
    pub fn register_genre_code(&mut self, code: &str) -> Result<bool> {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM Genres WHERE GenreCode = ?1 OR FB2Code = ?1",
            params![trimmed],
            |r| r.get(0),
        )?;
        if exists > 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO Genres (GenreCode, ParentCode, FB2Code, GenreAlias) VALUES (?1, NULL, ?1, ?1)",
            params![trimmed],
        )?;
        Ok(true)
    }

    /// Insert a book and its author/genre links atomically. Computes the seven `Search*` columns.
    /// Returns the new `BookID`.
    pub fn insert_book(&mut self, b: &Book, author_ids: &[i64], genre_codes: &[&str]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = insert_book_on(&tx, b, author_ids, genre_codes)?;
        tx.commit()?;
        Ok(id)
    }

    /// Same as [`Self::insert_book`] but without its own transaction — for bulk importers that wrap
    /// many inserts in one [`Db::begin`] / [`Db::commit`] pair (one commit per archive, not per book).
    pub fn insert_book_no_tx(&self, b: &Book, author_ids: &[i64], genre_codes: &[&str]) -> Result<i64> {
        insert_book_on(&self.conn, b, author_ids, genre_codes)
    }

    /// Insert many books in a single transaction — the fast path for large imports/seeding (one commit,
    /// not one per row). Computes the `Search*` columns; does **not** create author/genre links. Returns
    /// the number of rows inserted.
    pub fn insert_books_bulk(&mut self, books: &[Book]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        for b in books {
            let s_title = search::upper(&b.title);
            let s_lang = b.lang.as_deref().map(search::upper);
            let s_folder = b.folder.as_deref().map(search::upper);
            let s_file = search::upper(&b.file_name);
            let s_ext = b.ext.as_deref().map(search::upper);
            let s_kw = b.key_words.as_deref().map(search::upper);
            let s_ann = b.annotation.as_deref().map(search::upper);
            tx.execute(
                "INSERT INTO Books (LibID, Title, SeriesID, SeqNumber, UpdateDate, LibRate, Lang, Folder, \
                 FileName, InsideNo, Ext, BookSize, IsLocal, IsDeleted, KeyWords, Rate, Progress, Annotation, Review, \
                 SearchTitle, SearchLang, SearchFolder, SearchFileName, SearchExt, SearchKeyWords, SearchAnnotation) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
                params![
                    b.lib_id, b.title, b.series_id, b.seq_number, b.update_date, b.lib_rate, b.lang,
                    b.folder, b.file_name, b.inside_no, b.ext, b.book_size, b.is_local as i32,
                    b.is_deleted as i32, b.key_words, b.rate, b.progress, b.annotation, b.review,
                    s_title, s_lang, s_folder, s_file, s_ext, s_kw, s_ann,
                ],
            )?;
        }
        tx.commit()?;
        Ok(books.len())
    }

    pub fn get_book(&self, id: i64) -> Result<Option<Book>> {
        let out = self
            .conn
            .query_row(
                &format!("SELECT {} FROM Books WHERE BookID=?1", book_cols("")),
                params![id],
                row_to_book,
            )
            .optional()?;
        Ok(out)
    }

    /// Find a book by its source-file identity (folder + file name + position in container). Used
    /// for import dedupe — the original keys local books on their file, not on title/author.
    pub fn find_book_by_file(&self, folder: Option<&str>, file_name: &str, inside_no: i32) -> Result<Option<i64>> {
        let out = self
            .conn
            .query_row(
                "SELECT BookID FROM Books \
                 WHERE IFNULL(Folder,'')=IFNULL(?1,'') AND FileName=?2 AND InsideNo=?3",
                params![folder, file_name, inside_no],
                |r| r.get(0),
            )
            .optional()?;
        Ok(out)
    }

    /// Authors linked to a book.
    pub fn book_authors(&self, book_id: i64) -> Result<Vec<Author>> {
        let mut stmt = self.conn.prepare(
            "SELECT au.AuthorID, au.LastName, au.FirstName, au.MiddleName \
             FROM Author_List al JOIN Authors au ON au.AuthorID = al.AuthorID \
             WHERE al.BookID=?1 ORDER BY au.LastName",
        )?;
        let rows = stmt.query_map(params![book_id], |r| {
            Ok(Author {
                author_id: r.get(0)?,
                last_name: r.get(1)?,
                first_name: r.get(2)?,
                middle_name: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Genre codes linked to a book.
    pub fn book_genres(&self, book_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT GenreCode FROM Genre_List WHERE BookID=?1 ORDER BY GenreCode")?;
        let rows = stmt.query_map(params![book_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Genre labels for a set of books in ONE round-trip: `book_id -> "GenreA, GenreB"`. The grid's
    /// Жанр column uses this so the visible slice costs one indexed query, not one per row.
    pub fn book_genres_batch(&self, book_ids: &[i64]) -> Result<HashMap<i64, String>> {
        let mut out = HashMap::new();
        if book_ids.is_empty() {
            return Ok(out);
        }
        let ph: Vec<String> = (0..book_ids.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            "SELECT BookID, GROUP_CONCAT(GenreCode, ', ') FROM Genre_List \
             WHERE BookID IN ({}) GROUP BY BookID",
            ph.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(book_ids.iter().copied()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (id, g) = row?;
            if let Some(g) = g {
                out.insert(id, g);
            }
        }
        Ok(out)
    }

    /// Paginated search across the `Search*` columns. See [`BookQuery`].
    pub fn search_books(&self, q: &BookQuery, limit: i64, offset: i64) -> Result<Vec<Book>> {
        let (sql, params) = build_search_sql(q, limit, offset)?;
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_book)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Total number of books matching `q` (ignoring pagination).
    pub fn count_books(&self, q: &BookQuery) -> Result<i64> {
        let (where_sql, wparams) = search_where(q)?;
        let sql = format!("SELECT COUNT(*) FROM Books b {where_sql}");
        Ok(self.conn.query_row(&sql, params_from_iter(wparams), |r| r.get(0))?)
    }

    /// Simple collection statistics for the CLI / a future stats view.
    pub fn stats(&self) -> Result<CollectionStats> {
        let q = BookQuery::default();
        Ok(CollectionStats {
            books: self.count_books(&q)?,
            authors: self.scalar_count("SELECT COUNT(*) FROM Authors")?,
            series: self.scalar_count("SELECT COUNT(*) FROM Series")?,
            genres: self.scalar_count("SELECT COUNT(*) FROM Genres")?,
        })
    }

    /// Book counts per language code (non-deleted), most frequent first.
    pub fn counts_by_lang(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT Lang, COUNT(*) FROM Books WHERE IsDeleted = 0 AND Lang IS NOT NULL \
             GROUP BY Lang ORDER BY 2 DESC, 1 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Book counts per file extension (non-deleted), most frequent first.
    pub fn counts_by_ext(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT LTRIM(Ext, '.'), COUNT(*) FROM Books WHERE IsDeleted = 0 AND Ext IS NOT NULL \
             GROUP BY 1 ORDER BY 2 DESC, 1 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Distinct authors linked to non-deleted books.
    pub fn count_authors_in_use(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT al.AuthorID) FROM Author_List al \
             JOIN Books b ON b.BookID = al.BookID WHERE b.IsDeleted = 0",
            [],
            |r| r.get(0),
        )?)
    }

    /// Distinct series attached to non-deleted books.
    pub fn count_series_in_use(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT SeriesID) FROM Books WHERE IsDeleted = 0 AND SeriesID IS NOT NULL",
            [],
            |r| r.get(0),
        )?)
    }

    /// Distinct genres linked to non-deleted books (from the `Genre_List` links — the `Genres` catalog
    /// is only seeded by the built-in FB2 genre list, which a fresh import may not have populated).
    pub fn count_genres_in_use(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT gl.GenreCode) FROM Genre_List gl \
             JOIN Books b ON b.BookID = gl.BookID WHERE b.IsDeleted = 0",
            [],
            |r| r.get(0),
        )?)
    }

    fn scalar_count(&self, sql: &str) -> Result<i64> {
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }

    // ------------------------------------------------------------------ GUI tree navigation

    /// Authors that have books: `(AuthorID, display name, book count)`, sorted by name.
    pub fn list_author_nodes(&self) -> Result<Vec<(i64, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT au.AuthorID, MHL_FULLNAME(au.LastName, au.FirstName, au.MiddleName, 0), COUNT(al.BookID) \
             FROM Authors au JOIN Author_List al ON al.AuthorID = au.AuthorID \
             GROUP BY au.AuthorID ORDER BY 2 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Series that have books: `(SeriesID, title, book count)`, sorted by title.
    pub fn list_series_nodes(&self) -> Result<Vec<(i64, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.SeriesID, s.SeriesTitle, COUNT(*) \
             FROM Books b JOIN Series s ON s.SeriesID = b.SeriesID \
             WHERE b.IsDeleted = 0 GROUP BY b.SeriesID ORDER BY 2 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Genres in use: `(code, display name, book count)`, sorted by name. Derived from the `Genre_List`
    /// links (always present); the `Genres` catalog is left-joined for a human alias — matching either
    /// the dotted `GenreCode` or the raw FB2 code stored on the link — falling back to the raw code when
    /// no catalog row exists yet.
    pub fn list_genre_nodes(&self) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT gl.GenreCode, IFNULL(g.GenreAlias, gl.GenreCode), COUNT(*) \
             FROM Genre_List gl LEFT JOIN Genres g ON (g.GenreCode = gl.GenreCode OR g.FB2Code = gl.GenreCode) \
             GROUP BY gl.GenreCode ORDER BY 2 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Folders that have books: `(folder, book count)`; `None` = collection root.
    pub fn list_folder_nodes(&self) -> Result<Vec<(Option<String>, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT Folder, COUNT(*) FROM Books WHERE IsDeleted = 0 \
             GROUP BY Folder ORDER BY 1 COLLATE MHL_SYSTEM_NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Paginated books for a tree [`BookScope`] (or all when `All`). `limit < 0` = no limit. Ordered per
    /// scope — series by sequence number, everything else by title — matching the original's node lists.
    pub fn list_scope_books(&self, scope: &BookScope, limit: i64, offset: i64) -> Result<Vec<Book>> {
        let (where_sql, order_by, mut params) = scope_clause(scope);
        let n = params.len();
        let pagination = if limit < 0 {
            String::new()
        } else {
            format!(" LIMIT ?{} OFFSET ?{}", n + 1, n + 2)
        };
        if limit >= 0 {
            params.push(Box::new(limit));
            params.push(Box::new(offset));
        }
        let sql = format!(
            "SELECT {} FROM Books b WHERE {} {}{}",
            book_cols("b"), where_sql, order_by, pagination
        );
        self.query_books(&sql, params)
    }

    /// Total (non-deleted) books in a tree [`BookScope`]. Drives the virtualized list's row count.
    pub fn count_scope_books(&self, scope: &BookScope) -> Result<i64> {
        let (where_sql, _, params) = scope_clause(scope);
        let sql = format!("SELECT COUNT(*) FROM Books b WHERE {}", where_sql);
        Ok(self.conn.query_row(&sql, params_from_iter(params), |r| r.get(0))?)
    }

    /// Run a `SELECT … FROM Books b …` query that maps rows via [`row_to_book`].
    fn query_books(&self, sql: &str, params: Vec<Box<dyn ToSql>>) -> Result<Vec<Book>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_book)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Soft-delete a book (sets `IsDeleted = 1`), matching the original's soft-delete model.
    pub fn soft_delete_book(&mut self, book_id: i64) -> Result<()> {
        let n = self
            .conn
            .execute("UPDATE Books SET IsDeleted = 1 WHERE BookID = ?1", params![book_id])?;
        if n == 0 {
            return Err(crate::error::AppError::Other(format!("no book with id {book_id}")));
        }
        Ok(())
    }
}

/// Insert one book row + its links on `conn` (no transaction management). Statements are cached, so
/// the bulk importers pay the prepare cost once.
fn insert_book_on(conn: &rusqlite::Connection, b: &Book, author_ids: &[i64], genre_codes: &[&str]) -> Result<i64> {
    let s_title = search::upper(&b.title);
    let s_lang = b.lang.as_deref().map(search::upper);
    let s_folder = b.folder.as_deref().map(search::upper);
    let s_file = search::upper(&b.file_name);
    let s_ext = b.ext.as_deref().map(search::upper);
    let s_kw = b.key_words.as_deref().map(search::upper);
    let s_ann = b.annotation.as_deref().map(search::upper);

    conn.prepare_cached(
        "INSERT INTO Books (\
            LibID, Title, SeriesID, SeqNumber, UpdateDate, LibRate, Lang, Folder, FileName, \
            InsideNo, Ext, BookSize, IsLocal, IsDeleted, KeyWords, Rate, Progress, Annotation, Review, \
            SearchTitle, SearchLang, SearchFolder, SearchFileName, SearchExt, SearchKeyWords, SearchAnnotation \
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
    )?
    .execute(params![
        b.lib_id, b.title, b.series_id, b.seq_number, b.update_date, b.lib_rate, b.lang,
        b.folder, b.file_name, b.inside_no, b.ext, b.book_size, b.is_local as i32,
        b.is_deleted as i32, b.key_words, b.rate, b.progress, b.annotation, b.review,
        s_title, s_lang, s_folder, s_file, s_ext, s_kw, s_ann,
    ])?;
    let id = conn.last_insert_rowid();

    let mut al = conn.prepare_cached("INSERT OR IGNORE INTO Author_List (AuthorID, BookID) VALUES (?1,?2)")?;
    for &aid in author_ids {
        al.execute(params![aid, id])?;
    }
    let mut gl = conn.prepare_cached("INSERT OR IGNORE INTO Genre_List (GenreCode, BookID) VALUES (?1,?2)")?;
    for gc in genre_codes {
        gl.execute(params![gc, id])?;
    }
    Ok(id)
}

/// Build the WHERE clause + params for a [`BookQuery`]. Shared by search and count.
fn search_where(q: &BookQuery) -> Result<(String, Vec<Box<dyn ToSql>>)> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();

    if !q.include_deleted {
        clauses.push("b.IsDeleted = 0".to_string());
    }
    if q.only_local {
        clauses.push("b.IsLocal = 1".to_string());
    }
    if let Some(t) = &q.title_like {
        params.push(Box::new(format!("%{}%", search::upper(t))));
        clauses.push(format!("b.SearchTitle LIKE ?{}", params.len()));
    }
    if let Some(l) = &q.lang {
        params.push(Box::new(search::upper(l)));
        clauses.push(format!("b.SearchLang = ?{}", params.len()));
    }
    if let Some(e) = &q.ext {
        // The original stores extensions with the leading dot (".fb2"); collections written by earlier
        // builds of this port have them without it. Match either, whatever the user typed.
        let bare = search::upper(e.trim().trim_start_matches('.'));
        params.push(Box::new(bare.clone()));
        params.push(Box::new(format!(".{bare}")));
        clauses.push(format!("b.SearchExt IN (?{}, ?{})", params.len() - 1, params.len()));
    }
    if let Some(a) = &q.author_name_like {
        params.push(Box::new(format!("%{}%", search::upper(a))));
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM Author_List al \
             JOIN Authors au ON au.AuthorID = al.AuthorID \
             WHERE al.BookID = b.BookID AND au.SearchName LIKE ?{})",
            params.len()
        ));
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    Ok((where_sql, params))
}

/// Build the `WHERE` + `ORDER BY` + params for a [`BookScope`] list. Shared by [`Db::list_scope_books`]
/// and [`Db::count_scope_books`]. Series orders by sequence number; everything else by title — matching
/// the original's per-node book lists.
fn scope_clause(scope: &BookScope) -> (String, String, Vec<Box<dyn ToSql>>) {
    let by_title = "ORDER BY b.Title COLLATE MHL_SYSTEM_NOCASE";
    match scope {
        BookScope::All => ("b.IsDeleted = 0".into(), by_title.into(), vec![]),
        BookScope::Author(id) => (
            "b.IsDeleted = 0 AND EXISTS (SELECT 1 FROM Author_List al \
             WHERE al.BookID = b.BookID AND al.AuthorID = ?1)"
                .into(),
            by_title.into(),
            vec![Box::new(*id)],
        ),
        BookScope::Genre(code) => (
            "b.IsDeleted = 0 AND EXISTS (SELECT 1 FROM Genre_List gl \
             WHERE gl.BookID = b.BookID AND gl.GenreCode = ?1)"
                .into(),
            by_title.into(),
            vec![Box::new(code.clone())],
        ),
        BookScope::Series(id) => (
            "b.IsDeleted = 0 AND b.SeriesID = ?1".into(),
            "ORDER BY b.SeqNumber, b.Title COLLATE MHL_SYSTEM_NOCASE".into(),
            vec![Box::new(*id)],
        ),
        BookScope::Folder(f) => (
            "b.IsDeleted = 0 AND IFNULL(b.Folder,'') = IFNULL(?1,'')".into(),
            by_title.into(),
            vec![Box::new(f.clone())],
        ),
    }
}

/// Assemble the full paginated SELECT for [`Db::search_books`].
fn build_search_sql(q: &BookQuery, limit: i64, offset: i64) -> Result<(String, Vec<Box<dyn ToSql>>)> {
    let (where_sql, mut params) = search_where(q)?;
    let n = params.len();
    let pagination = if limit < 0 {
        String::new()
    } else {
        format!(" LIMIT ?{} OFFSET ?{}", n + 1, n + 2)
    };
    if limit >= 0 {
        params.push(Box::new(limit));
        params.push(Box::new(offset));
    }

    let sql = format!(
        "SELECT {} FROM Books b {} ORDER BY b.Title COLLATE MHL_SYSTEM_NOCASE{}",
        book_cols("b"), where_sql, pagination
    );
    Ok((sql, params))
}

/// Aggregate counts over a collection.
#[derive(Debug, Clone, Copy)]
pub struct CollectionStats {
    pub books: i64,
    pub authors: i64,
    pub series: i64,
    pub genres: i64,
}

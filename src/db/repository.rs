//! Data-access methods on [`Db`]: CRUD for Books/Authors/Series/Genres, full search across the
//! uppercased `Search*` columns, and pagination. All writes compute the `Search*` columns in-app
//! (via [`crate::core::search`]) so they are correct whether or not the schema's triggers fire.

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
#[derive(Debug, Clone, Default)]
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

impl Db {
    // ------------------------------------------------------------------ Authors

    /// Insert or return the existing author. The original has no unique constraint on authors, so
    /// dedup happens here on (Last, First, Middle). Returns the `AuthorID`.
    pub fn upsert_author(&self, last: &str, first: Option<&str>, middle: Option<&str>) -> Result<i64> {
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
    pub fn upsert_series(&self, title: &str) -> Result<i64> {
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
    pub fn upsert_genre(&self, code: &str, parent: Option<&str>, fb2: Option<&str>, alias: &str) -> Result<()> {
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

    /// Insert a book and its author/genre links atomically. Computes the seven `Search*` columns.
    /// Returns the new `BookID`.
    pub fn insert_book(&self, b: &Book, author_ids: &[i64], genre_codes: &[&str]) -> Result<i64> {
        let s_title = search::upper(&b.title);
        let s_lang = b.lang.as_deref().map(search::upper);
        let s_folder = b.folder.as_deref().map(search::upper);
        let s_file = search::upper(&b.file_name);
        let s_ext = b.ext.as_deref().map(search::upper);
        let s_kw = b.key_words.as_deref().map(search::upper);
        let s_ann = b.annotation.as_deref().map(search::upper);

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO Books (\
                LibID, Title, SeriesID, SeqNumber, UpdateDate, LibRate, Lang, Folder, FileName, \
                InsideNo, Ext, BookSize, IsLocal, IsDeleted, KeyWords, Rate, Progress, Annotation, Review, \
                SearchTitle, SearchLang, SearchFolder, SearchFileName, SearchExt, SearchKeyWords, SearchAnnotation \
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
            params![
                b.lib_id, b.title, b.series_id, b.seq_number, b.update_date, b.lib_rate, b.lang,
                b.folder, b.file_name, b.inside_no, b.ext, b.book_size, b.is_local as i32,
                b.is_deleted as i32, b.key_words, b.rate, b.progress, b.annotation, b.review,
                s_title, s_lang, s_folder, s_file, s_ext, s_kw, s_ann,
            ],
        )?;
        let id = tx.last_insert_rowid();

        for &aid in author_ids {
            tx.execute("INSERT OR IGNORE INTO Author_List (AuthorID, BookID) VALUES (?1,?2)", params![aid, id])?;
        }
        for gc in genre_codes {
            tx.execute("INSERT OR IGNORE INTO Genre_List (GenreCode, BookID) VALUES (?1,?2)", params![gc, id])?;
        }

        tx.commit()?;
        Ok(id)
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

    fn scalar_count(&self, sql: &str) -> Result<i64> {
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }
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
        params.push(Box::new(search::upper(e)));
        clauses.push(format!("b.SearchExt = ?{}", params.len()));
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

//! Mirrors the `Books` table (the central entity). See `schema/create_collection_db.sql`.
//!
//! The seven uppercased `Search*` columns (`SearchTitle`, `SearchLang`, `SearchFolder`,
//! `SearchFileName`, `SearchExt`, `SearchKeyWords`, `SearchAnnotation`) are maintained by the
//! schema's triggers through the registered `MHL_UPPER` UDF. They are intentionally *not* fields
//! on this struct: they are derived, and the DAO writes them explicitly (see
//! [`crate::db::repository::Db::insert_book`]) so behavior is correct whether or not triggers fire.

#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    pub book_id: i64,
    /// Stable external identifier (e.g. the Librusec/Flibusta book id for remote books).
    pub lib_id: String,
    pub title: String,
    pub series_id: Option<i64>,
    /// Position within the series (1-based), if part of a series.
    pub seq_number: Option<i32>,
    /// Last-modified timestamp. The original stores a formatted date string (`VARCHAR(23)`);
    /// kept as-is here for exact DB compatibility.
    pub update_date: String,
    /// Rating coming from the source library (0 = none).
    pub lib_rate: i32,
    /// ISO language code, e.g. "ru", "en".
    pub lang: Option<String>,
    /// Folder on disk relative to the collection root (local books).
    pub folder: Option<String>,
    pub file_name: String,
    /// Index of this book inside a multi-book container such as an FBD archive.
    pub inside_no: i32,
    /// File extension, e.g. "fb2".
    pub ext: Option<String>,
    /// File size in bytes.
    pub book_size: Option<i64>,
    /// 1 = a physical file exists on disk; 0 = metadata-only (e.g. remote).
    pub is_local: bool,
    /// Soft-delete flag. The original keeps deleted rows and filters them out of views.
    pub is_deleted: bool,
    pub key_words: Option<String>,
    /// User rating (0..5 in the original UI).
    pub rate: i32,
    /// Reading progress percentage (0..100).
    pub progress: i32,
    pub annotation: Option<String>,
    /// Free-form user review (may be binary/UTF-8 blob in the original).
    pub review: Option<Vec<u8>>,
}

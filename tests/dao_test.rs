//! Integration tests for the Phase 1 data layer: schema init, UDF-driven search columns,
//! author/series dedup, book insert + links, and paginated search.

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};
use rusqlite::params;

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    (dir, path)
}

fn make_book(title: &str, lang: &str) -> Book {
    Book {
        book_id: 0,
        lib_id: format!("lib-{title}"),
        title: title.to_string(),
        series_id: None,
        seq_number: None,
        update_date: "2026-11-20 00:00:00".to_string(),
        lib_rate: 0,
        lang: Some(lang.to_string()),
        folder: Some("x".to_string()),
        file_name: format!("{title}.fb2"),
        inside_no: 0,
        ext: Some("fb2".to_string()),
        book_size: Some(100),
        is_local: true,
        is_deleted: false,
        key_words: None,
        rate: 0,
        progress: 0,
        annotation: None,
        review: None,
    }
}

#[test]
fn open_initializes_schema_and_is_idempotent() {
    let (_d, path) = temp_db();
    Db::open(&path).unwrap(); // creates
    let db2 = Db::open(&path).unwrap(); // reopens — must not drop existing data
    let n: i64 = db2
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Books'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn upsert_author_dedups_and_sets_search_name() {
    let (_d, path) = temp_db();
    let db = Db::open(&path).unwrap();
    let id1 = db.upsert_author("Толстой", Some("Лев"), Some("Николаевич")).unwrap();
    let id2 = db.upsert_author("Толстой", Some("Лев"), Some("Николаевич")).unwrap();
    assert_eq!(id1, id2);

    let a = db.get_author(id1).unwrap().unwrap();
    assert_eq!(a.search_name(), "ТОЛСТОЙ ЛЕВ НИКОЛАЕВИЧ");

    // The stored SearchName (written in-app and reinforced by the trigger) must match.
    let stored: String = db
        .connection()
        .query_row("SELECT SearchName FROM Authors WHERE AuthorID=?1", params![id1], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, "ТОЛСТОЙ ЛЕВ НИКОЛАЕВИЧ");
}

#[test]
fn insert_book_populates_search_columns_and_links() {
    let (_d, path) = temp_db();
    let db = Db::open(&path).unwrap();
    let a = db.upsert_author("Толстой", Some("Лев"), None).unwrap();
    let b = make_book("Война и мир", "ru");
    let id = db.insert_book(&b, &[a], &["prose.classic"]).unwrap();

    let got = db.get_book(id).unwrap().unwrap();
    assert_eq!(got.title, "Война и мир");

    let st: String = db
        .connection()
        .query_row("SELECT SearchTitle FROM Books WHERE BookID=?1", params![id], |r| r.get(0))
        .unwrap();
    assert_eq!(st, "ВОЙНА И МИР");

    assert_eq!(db.book_authors(id).unwrap().len(), 1);
    assert_eq!(db.book_genres(id).unwrap(), vec!["prose.classic".to_string()]);
}

#[test]
fn search_by_title_and_author_with_pagination() {
    let (_d, path) = temp_db();
    let db = Db::open(&path).unwrap();
    let a_tol = db.upsert_author("Толстой", Some("Лев"), None).unwrap();
    let a_dost = db.upsert_author("Достоевский", Some("Фёдор"), None).unwrap();

    for (i, title) in ["Война и мир", "Анна Каренина", "Преступление и наказание"].into_iter().enumerate() {
        let b = make_book(title, "ru");
        let a = if i < 2 { a_tol } else { a_dost };
        db.insert_book(&b, &[a], &[]).unwrap();
    }

    // By author (case-insensitive on SearchName).
    let q = BookQuery { author_name_like: Some("Толстой".into()), ..Default::default() };
    assert_eq!(db.count_books(&q).unwrap(), 2);

    // Pagination returns distinct single-row pages.
    let page0 = db.search_books(&q, 1, 0).unwrap();
    let page1 = db.search_books(&q, 1, 1).unwrap();
    assert_eq!(page0.len(), 1);
    assert_eq!(page1.len(), 1);
    assert_ne!(page0[0].book_id, page1[0].book_id);

    // By title substring.
    let qt = BookQuery { title_like: Some("Каренина".into()), ..Default::default() };
    let r = db.search_books(&qt, 10, 0).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].title, "Анна Каренина");
}

#[test]
fn soft_deleted_excluded_by_default() {
    let (_d, path) = temp_db();
    let db = Db::open(&path).unwrap();
    let a = db.upsert_author("X", None, None).unwrap();

    let visible = make_book("Visible", "en");
    let id1 = db.insert_book(&visible, &[a], &[]).unwrap();

    let mut hidden = make_book("Hidden", "en");
    hidden.is_deleted = true;
    db.insert_book(&hidden, &[a], &[]).unwrap();

    let all = db.search_books(&BookQuery::default(), 100, 0).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].book_id, id1);

    // But included when asked.
    let q = BookQuery { include_deleted: true, ..Default::default() };
    assert_eq!(db.count_books(&q).unwrap(), 2);
}

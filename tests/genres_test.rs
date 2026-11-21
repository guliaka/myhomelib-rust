//! Genre catalog tests: file-format parsing, built-in seeding on new collections, alias resolution,
//! replace-from-file with dangling-link cleanup, and the import "register genre" path.

use std::sync::atomic::{AtomicUsize, Ordering};

use myhomelib::core::Book;
use myhomelib::db::Db;
use myhomelib::genres::parse_genre_lines;

static CTR: AtomicUsize = AtomicUsize::new(0);

fn temp_db() -> (Db, std::path::PathBuf) {
    let n = CTR.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mhl_genres_test_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("g.db");
    (Db::open(&path).unwrap(), dir)
}

fn book(title: &str, _genre: &str) -> Book {
    Book {
        book_id: 0,
        lib_id: String::new(),
        title: title.into(),
        series_id: None,
        seq_number: None,
        update_date: "2026-01-01 00:00:00".into(),
        lib_rate: 0,
        lang: Some("ru".into()),
        folder: None,
        file_name: format!("{title}.fb2"),
        inside_no: 0,
        ext: Some("fb2".into()),
        book_size: Some(1000),
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
fn parse_genre_lines_format() {
    let text = "# a comment\n\
                0.1 lit;Художественная литература\n\
                0.1.1 prose;Проза\n\
                0.1.1.1 prose_classic;Классическая проза\n\
                nodot (skipped — no dot in code)\n\
                \n\
                0.2 nonlit";
    let rows = parse_genre_lines(text);
    assert_eq!(rows.len(), 4, "comment + no-dot line are skipped");

    let g0 = &rows[0];
    assert_eq!(g0.genre_code, "0.1");
    assert_eq!(g0.parent_code.as_deref(), Some("0"));
    assert_eq!(g0.fb2_code.as_deref(), Some("lit"));
    assert_eq!(g0.genre_alias, "Художественная литература");

    let g2 = &rows[2];
    assert_eq!(g2.genre_code, "0.1.1.1");
    assert_eq!(g2.parent_code.as_deref(), Some("0.1.1"));
    assert_eq!(g2.fb2_code.as_deref(), Some("prose_classic"));
    assert_eq!(g2.genre_alias, "Классическая проза");

    // A line with no fb2 code: the whole remainder is the alias.
    let g3 = &rows[3];
    assert_eq!(g3.genre_code, "0.2");
    assert_eq!(g3.fb2_code.as_deref(), None);
    assert_eq!(g3.genre_alias, "nonlit");
}

#[test]
fn new_collection_is_seeded_with_builtin_genres() {
    let (db, dir) = temp_db();
    let genres = db.list_genres().unwrap();
    assert!(genres.len() > 50, "built-in tree should be seeded, got {}", genres.len());

    // A known leaf: fb2 code "sf" -> "Научная фантастика".
    let sf = genres.iter().find(|g| g.fb2_code.as_deref() == Some("sf")).unwrap();
    assert_eq!(sf.genre_code, "0.1.1.5.2");
    assert_eq!(sf.genre_alias, "Научная фантастика");
    assert_eq!(sf.parent_code.as_deref(), Some("0.1.1.5"));

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn genre_node_alias_resolves_after_seed() {
    let (mut db, dir) = temp_db();
    // Link a book by the raw FB2 code; the seeded catalog should supply the human alias.
    let author = db.upsert_author("Толстой", Some("Лев"), None).unwrap();
    let b = book("Война и мир", "prose_classic");
    db.insert_book(&b, &[author], &["prose_classic"]).unwrap();

    let nodes = db.list_genre_nodes().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].0, "prose_classic"); // code stored on the link
    assert_eq!(nodes[0].1, "Классическая проза"); // alias resolved via FB2Code join

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_genres_from_file_and_cleanup_dangling() {
    let (mut db, dir) = temp_db();
    // A book linked to a genre that will NOT survive the replacement.
    let author = db.upsert_author("X", None, None).unwrap();
    let b = book("Old", "obsolete_code");
    db.insert_book(&b, &[author], &["obsolete_code"]).unwrap();

    // Replace with a tiny custom catalog (one genre).
    let file = dir.join("custom_genres.txt");
    std::fs::write(&file, "# custom\n1.1 alpha;Alpha branch\n1.1.1 beta;Beta leaf\n").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let rows = parse_genre_lines(&text);
    assert_eq!(rows.len(), 2);
    let n = db.replace_genres(&rows).unwrap();
    assert_eq!(n, 2);

    // Catalog now has exactly the two custom rows.
    let genres = db.list_genres().unwrap();
    assert_eq!(genres.len(), 2);
    assert!(genres.iter().any(|g| g.genre_code == "1.1"));
    assert!(genres.iter().any(|g| g.fb2_code.as_deref() == Some("beta")));

    // The dangling link (obsolete_code no longer in the catalog) was removed.
    let nodes = db.list_genre_nodes().unwrap();
    assert!(nodes.is_empty(), "dangling genre link should be cleaned up");

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn register_genre_code_is_idempotent() {
    let (mut db, dir) = temp_db();
    // A built-in code already exists -> no new row.
    assert!(!db.register_genre_code("sf").unwrap());
    // A brand-new code is inserted once, then stable.
    assert!(db.register_genre_code("my_custom_genre").unwrap());
    assert!(!db.register_genre_code("my_custom_genre").unwrap());

    let genres = db.list_genres().unwrap();
    let custom = genres.iter().find(|g| g.genre_code == "my_custom_genre").unwrap();
    assert_eq!(custom.fb2_code.as_deref(), Some("my_custom_genre"));
    assert_eq!(custom.genre_alias, "my_custom_genre");

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

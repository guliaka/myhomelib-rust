//! Phase 3 acceptance: folder/FBD import (metadata extraction + source-file dedupe) and metadata
//! export that round-trips through the FB2 engine.

use base64::Engine;
use myhomelib::db::{Db, BookQuery};
use myhomelib::export::Exporter;
use myhomelib::fb2::fbd::Fbd;
use myhomelib::fb2::parse_fb2;
use myhomelib::import::Importer;

const COVER: &[u8] = b"\x89PNG-fake-cover-bytes";

fn sample_fb2(cover_b64: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2" xmlns:xlink="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <id>import-test-1</id>
      <genre>prose_classic</genre>
      <author><last-name>Толстой</last-name><first-name>Лев</first-name></author>
      <book-title>Война и мир</book-title>
      <lang>ru</lang>
      <series><name>Война и мир</name><number>1</number></series>
      <annotation>Роман о войне.</annotation>
    </title-info>
  </description>
  <body>
    <section><p>Текст книги.</p></section>
  </body>
  <binary id="cover">{cover_b64}</binary>
</FictionBook>"##
    )
}

fn b64() -> String {
    base64::engine::general_purpose::STANDARD.encode(COVER)
}

const DOSTOEVSKY: &str = r##"<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2">
  <description>
    <title-info>
      <id>import-test-2</id>
      <genre>prose_classic</genre>
      <author><last-name>Достоевский</last-name><first-name>Фёдор</first-name></author>
      <book-title>Преступление и наказание</book-title>
      <lang>ru</lang>
      <annotation>Роман о преступлении.</annotation>
    </title-info>
  </description>
  <body><section><p>Текст.</p></section></body>
</FictionBook>"##;

#[test]
fn import_folder_dedupe_and_export_roundtrip() {
    let dir = std::env::temp_dir().join(format!("mhl_import_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // One standalone FB2 in the root, one book inside an FBD in a subfolder.
    std::fs::write(dir.join("voyna.fb2"), sample_fb2(&b64())).unwrap();
    let sub = dir.join("russian");
    std::fs::create_dir_all(&sub).unwrap();
    let book2 = parse_fb2(DOSTOEVSKY.as_bytes()).unwrap();
    Fbd::write(sub.join("crime.fbd").as_path(), &book2, None).unwrap();

    let db_path = dir.join("collection.db");
    let mut db = Db::open(&db_path).unwrap();

    // First import: both books are new.
    let mut imp = Importer::new(&mut db, &dir);
    let report = imp.import_dir(&dir).unwrap();
    assert_eq!(report.imported, 2, "expected 2 imported, got {report:?}");
    assert_eq!(report.existing, 0);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

    // Verify the standalone FB2 book landed with correct metadata.
    let books = db.search_books(&BookQuery::default(), -1, 0).unwrap();
    assert_eq!(books.len(), 2);
    let voyna = books
        .iter()
        .find(|b| b.title == "Война и мир")
        .expect("voyna present");
    assert_eq!(voyna.lang.as_deref(), Some("ru"));
    assert_eq!(voyna.ext.as_deref(), Some("fb2"));
    assert!(voyna.is_local);
    let authors = db.book_authors(voyna.book_id).unwrap();
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].last_name, "Толстой");
    assert_eq!(db.book_genres(voyna.book_id).unwrap(), vec!["prose_classic".to_string()]);
    assert!(voyna.series_id.is_some(), "series should be linked");

    // Second import of the same dir: everything deduped as existing, no duplicates.
    let mut imp2 = Importer::new(&mut db, &dir);
    let report2 = imp2.import_dir(&dir).unwrap();
    assert_eq!(report2.imported, 0, "re-import should add nothing");
    assert_eq!(report2.existing, 2);
    assert_eq!(db.count_books(&BookQuery::default()).unwrap(), 2, "no duplicates");

    // Export the standalone book back to FB2 and verify metadata round-trips.
    let exporter = Exporter::new(&db);
    let out_path = dir.join("out").join("voyna-export.fb2");
    exporter.export_book(voyna.book_id, &out_path).unwrap();
    let re_parsed = parse_fb2(&std::fs::read(&out_path).unwrap()).unwrap();
    assert_eq!(re_parsed.book_title().as_deref(), Some("Война и мир"));
    assert_eq!(re_parsed.lang().as_deref(), Some("ru"));
    let ra = re_parsed.authors();
    assert_eq!(ra.len(), 1);
    assert_eq!(ra[0].last_name.as_deref(), Some("Толстой"));
    assert!(re_parsed.annotation().unwrap().contains("Роман о войне."));

    let _ = std::fs::remove_dir_all(&dir);
}

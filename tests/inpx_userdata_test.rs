//! INPX import/export + user-data (rate/progress/review + groups) round-trip tests.

use std::io::Write;
use std::path::Path;

use myhomelib::core::Book;
use myhomelib::db::Db;
use myhomelib::inpx::{export_inpx, import_inpx, InpxHeader};
use myhomelib::systemdb::SystemDb;
use myhomelib::userdata::{
    export_groups_from_system, import_groups_to_system, GroupData, UserData,
};
use tempfile::TempDir;

/// A book with sensible defaults; caller sets title/libid/lang and overrides as needed.
fn mkbook(title: &str, lib_id: &str, lang: &str) -> Book {
    Book {
        book_id: 0,
        lib_id: lib_id.into(),
        title: title.into(),
        series_id: None,
        seq_number: None,
        update_date: "2026-11-21 12:00:00".into(),
        lib_rate: 0,
        lang: Some(lang.into()),
        folder: Some("lib.zip".into()),
        file_name: format!("{title}.fb2"),
        inside_no: 0,
        ext: Some("fb2".into()),
        book_size: Some(100_000),
        is_local: true,
        is_deleted: false,
        key_words: None,
        rate: 0,
        progress: 0,
        annotation: None,
        review: None,
    }
}

fn seeded_db(path: &Path) -> Db {
    let mut db = Db::open(path).unwrap();
    let a = db.upsert_author("Толстой", Some("Лев"), Some("Николаевич")).unwrap();
    db.upsert_genre("prose.classic", None, Some("classic"), "Классическая проза").unwrap();
    let series = db.upsert_series("Война и мир").unwrap();
    let mut b = mkbook("Война и мир", "lib-1", "ru");
    b.series_id = Some(series);
    b.seq_number = Some(1);
    db.insert_book(&b, &[a], &["prose.classic"]).unwrap();
    let mut b2 = mkbook("1984", "lib-2", "en");
    b2.key_words = Some("dystopia".into());
    let a_orwell = db.upsert_author("Orwell", Some("George"), None).unwrap();
    db.insert_book(&b2, &[a_orwell], &["prose.classic"]).unwrap();
    db
}

#[test]
fn inpx_export_then_import_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let mut src = seeded_db(&tmp.path().join("src.db"));

    // Give one book user data too, to prove INPX export doesn't confuse it with catalog data.
    src.connection().execute("UPDATE Books SET Rate = 5, Progress = 40 WHERE LibID = 'lib-1'", []).unwrap();

    let inpx = tmp.path().join("export.inpx");
    let header = InpxHeader {
        name: "Тестовая".into(),
        file_name: "test.db".into(),
        content_type: 0x0001_0000,
        notes: "заметка".into(),
        url: "http://lib.example".into(),
        script: "GET http://lib.example".into(),
    };
    let n = export_inpx(&src, &inpx, &header, "20261121").unwrap();
    assert_eq!(n, 2);

    // Import into a fresh collection.
    let mut dst = Db::open(tmp.path().join("dst.db")).unwrap();
    let report = import_inpx(&mut dst, &inpx).unwrap();
    assert_eq!(report.imported, 2);
    assert_eq!(report.existing, 0);
    assert!(report.errors.is_empty());
    assert_eq!(report.header.unwrap().name, "Тестовая");
    assert_eq!(report.version.unwrap(), 20261121);

    // Fields survive: authors, genres (as FB2 codes), series, lang, size, libid, keywords.
    let q = myhomelib::db::BookQuery { title_like: Some("Война".into()), ..Default::default() };
    let books = dst.search_books(&q, 10, 0).unwrap();
    assert_eq!(books.len(), 1);
    let b = &books[0];
    assert_eq!(b.lang.as_deref(), Some("ru"));
    assert_eq!(b.lib_id, "lib-1");
    assert_eq!(b.book_size, Some(100_000));
    assert_eq!(b.seq_number, Some(1));
    let authors = dst.book_authors(b.book_id).unwrap();
    assert_eq!(authors[0].last_name, "Толстой");
    assert_eq!(authors[0].first_name.as_deref(), Some("Лев"));
    let genres = dst.book_genres(b.book_id).unwrap();
    assert_eq!(genres, ["classic"]); // exported as FB2 code, re-imported as genre code
    let series_title = dst.get_series(b.series_id.unwrap()).unwrap().unwrap().series_title;
    assert_eq!(series_title, "Война и мир");

    // Re-import is a no-op (dedupe on source-file identity).
    let report = import_inpx(&mut dst, &inpx).unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.existing, 2);
}

#[test]
fn inpx_import_librus_ec_sample() {
    // A hand-built INPX in lib.rus.ec's shape: structure.info + 12345-67890.inp with INSNO/FOLDER.
    let tmp = TempDir::new().unwrap();
    let inpx_path = tmp.path().join("lib.inpx");
    let f = std::fs::File::create(&inpx_path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions = Default::default();

    zip.start_file("structure.info", opts).unwrap();
    zip.write_all(b"AUTHOR;GENRE;TITLE;SERIES;SERNO;FILE;SIZE;LIBID;DEL;EXT;DATE;INSNO;FOLDER;LANG;KEYWORDS;").unwrap();
    zip.start_file("12345.inp", opts).unwrap();
    let rec = [

        "Иванов,Иван:Петров,Пётр:",
        "sf_fantasy:",
        "Книга один",
        "",
        "",
        "12345.fb2",
        "123456",
        "12345",
        "0",
        "fb2",
        "2026-01-15",
        "0",
        "lib.zip",
        "ru",
        "",
    ].join("\u{4}");
    zip.write_all(format!("{rec}\r\n").as_bytes()).unwrap();
    zip.start_file("collection.info", opts).unwrap();
    zip.write_all("Библиотека\r\ntest.db\r\n65552\r\n\r\nhttp://lib\r\nGET http://lib".as_bytes()).unwrap();
    zip.finish().unwrap();

    let mut db = Db::open(tmp.path().join("c.db")).unwrap();
    let report = import_inpx(&mut db, &inpx_path).unwrap();
    assert_eq!(report.imported, 1);
    assert!(report.errors.is_empty());
    let h = report.header.unwrap();
    assert_eq!(h.name, "Библиотека");
    assert_eq!(h.url, "http://lib");

    let b = db.search_books(&Default::default(), 10, 0).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].folder.as_deref(), Some("lib.zip"));
    assert_eq!(b[0].file_name, "12345.fb2");
    let authors = db.book_authors(b[0].book_id).unwrap();
    assert_eq!(authors.len(), 2);
    assert_eq!(authors[0].last_name, "Иванов");
    assert_eq!(authors[1].last_name, "Петров");
}

#[test]
fn user_data_xml_roundtrip() {
    let data = UserData {
        extras: vec![myhomelib::userdata::BookExtra {
            book_id: Some(5),
            libid: Some("lib-5".into()),
            rate: 4,
            progress: 30,
            review: Some("отлично, местами <![CDATA[ ... ]]> слёзы".into()),
        }],
        groups: vec![GroupData {
            group_id: Some(1),
            name: "Избранное".into(),
            books: vec![(Some(5), Some("lib-5".into())), (None, Some("lib-7".into()))],
        }],
    };
    let xml = data.to_xml();
    let parsed = UserData::from_xml(&xml).unwrap();
    assert_eq!(parsed, data);
    assert!(xml.contains("<Review><![CDATA["));
}

#[test]
fn user_data_file_zlib_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("user.mhl");
    let data = UserData {
        extras: vec![myhomelib::userdata::BookExtra {
            book_id: None,
            libid: Some("lib-1".into()),
            rate: 5,
            progress: 42,
            review: Some("перечитать".into()),
        }],
        groups: vec![],
    };
    data.save(&path).unwrap();
    // The original saves zlib (0x78 magic), not zip/gzip.
    let magic = std::fs::read(&path).unwrap()[0];
    assert_eq!(magic, 0x78);
    let loaded = UserData::load(&path).unwrap();
    assert_eq!(loaded, data);

    // Plain uncompressed XML is also accepted (our own to_xml output).
    let xml_path = tmp.path().join("user.xml");
    std::fs::write(&xml_path, data.to_xml()).unwrap();
    assert_eq!(UserData::load(&xml_path).unwrap(), data);
}

#[test]
fn user_data_collection_roundtrip_and_groups() {
    let tmp = TempDir::new().unwrap();
    let mut src = seeded_db(&tmp.path().join("src.db"));
    // Rate/progress/review on one book.
    src.connection()
        .execute("UPDATE Books SET Rate = 5, Progress = 40, Review = 'шедевр' WHERE LibID = 'lib-1'", [])
        .unwrap();

    // Register the collection so groups can be attached to it.
    let sys = SystemDb::open(tmp.path().join("s.db")).unwrap();
    let id = sys
        .add(&myhomelib::systemdb::CollectionInfo {
            name: "Библиотека".into(),
            db_file_name: tmp.path().join("src.db").to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap();
    // Put lib-1 into a group (resolve its collection-local BookID first).
    let book_id: i64 = src
        .connection()
        .query_row("SELECT BookID FROM Books WHERE LibID = 'lib-1'", [], |r| r.get(0))
        .unwrap();
    let group_id = sys.upsert_group("Прочитать").unwrap();
    sys.add_group_book(group_id, book_id, id, "lib-1", "Война и мир").unwrap();
    sys.add_group_book(group_id, book_id, id, "lib-1", "Война и мир").unwrap(); // idempotent
    assert_eq!(sys.list_group_books(id).unwrap().len(), 1);

    // Export user data (extras from collection + groups from system DB).
    let mut data = UserData::from_collection(&src).unwrap();
    export_groups_from_system(&mut data, &sys, id).unwrap();
    assert_eq!(data.extras.len(), 1);
    assert_eq!(data.extras[0].libid.as_deref(), Some("lib-1"));
    assert_eq!(data.extras[0].review.as_deref(), Some("шедевр"));
    assert_eq!(data.groups.len(), 1);
    assert_eq!(data.groups[0].name, "Прочитать");
    assert_eq!(data.groups[0].books.len(), 1);

    // Import into a FRESH collection with the same LibIDs and a FRESH system DB.
    let dst = seeded_db(&tmp.path().join("dst.db"));
    let sys2 = SystemDb::open(tmp.path().join("s2.db")).unwrap();
    let id2 = sys2
        .add(&myhomelib::systemdb::CollectionInfo {
            name: "Библиотека 2".into(),
            db_file_name: tmp.path().join("dst.db").to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap();
    let mut dst = dst;
    let (applied, missing) = data.apply_to_collection(&mut dst).unwrap();
    assert_eq!(applied, 1);
    assert_eq!(missing, 0);
    let r = dst
        .connection()
        .query_row("SELECT Rate, Progress, Review FROM Books WHERE LibID = 'lib-1'", [], |r| {
            // Review is a BLOB column — read bytes, compare as UTF-8.
            let review: Option<Vec<u8>> = r.get(2)?;
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, i32>(1)?,
                review.map(|b| String::from_utf8_lossy(&b).into_owned()),
            ))
        })
        .unwrap();
    assert_eq!(r, (5, 40, Some("шедевр".to_string())));

    let (linked, unresolved) = import_groups_to_system(&data.groups, &sys2, &dst, id2).unwrap();
    assert_eq!(linked, 1);
    assert_eq!(unresolved, 0);
    let books = sys2.list_group_books(id2).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].1, "Прочитать");
    assert_eq!(books[0].3, "lib-1");
}

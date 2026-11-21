//! INPX → local collection import, and the system database (collection registry + groups).

use std::io::Write;
use std::path::{Path, PathBuf};

use myhomelib::db::{props, BookQuery, Db};
use myhomelib::inpx::{import_inpx, read_header, InpxOptions};
use myhomelib::jobs::{Job, JobProgress};
use myhomelib::sysdb::{SystemDb, FAVORITES_GROUP_NAME, TO_READ_GROUP_NAME};
use zip::write::SimpleFileOptions;

const D: char = '\u{4}';

/// AUTHOR;GENRE;TITLE;SERIES;SERNO;FILE;SIZE;LIBID;DEL;EXT;DATE;LANG;LIBRATE;KEYWORDS
#[allow(clippy::too_many_arguments)]
fn rec(authors: &str, genres: &str, title: &str, series: &str, serno: &str, file: &str, del: &str, lang: &str) -> String {
    [authors, genres, title, series, serno, file, "1000", file, del, "fb2", "2012-01-15", lang, "3", ""]
        .join(&D.to_string())
        + &D.to_string()
}

fn write_inpx(path: &Path, entries: &[(&str, String)]) {
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in entries {
        zw.start_file(*name, SimpleFileOptions::default()).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

fn sample_inpx(dir: &Path) -> PathBuf {
    let inp1 = [
        rec("Толстой,Лев,Николаевич:", "prose_classic:", "Война и мир", "Эпопея", "1", "100", "0", "ru"),
        rec("Толстой,Лев,Николаевич:", "prose_classic:", "Анна Каренина", "", "", "101", "0", "ru"),
        rec("Ильф,Илья,:Петров,Евгений,:", "humor:no_such_genre:", "Двенадцать стульев", "Остап Бендер", "1", "102", "0", "ru"),
    ]
    .join("\r\n");
    let inp2 = [
        rec("Orwell,George,:", "sf:", "1984", "", "", "200", "1", "en"),
        "garbage without delimiters".to_string(),
        rec("", "", "", "", "", "201", "0", ""),
    ]
    .join("\r\n");
    let extra = rec("Online,Only,:", "sf:", "Online only", "", "", "900", "0", "ru");
    let path = dir.join("test.inpx");
    write_inpx(
        &path,
        &[
            ("fb2-000100-000102.inp", inp1),
            ("fb2-000200-000201.inp", inp2),
            ("extra.inp", extra),
            ("collection.info", "Test Lib\r\ntestlib\r\n65536\r\nThe notes\r\nhttp://example.org/\r\nGET %URL%\r\n".to_string()),
            ("version.info", "20120115\r\n".to_string()),
            ("structure.info", "AUTHOR;GENRE;TITLE;SERIES;SERNO;FILE;SIZE;LIBID;DEL;EXT;DATE;LANG;LIBRATE;KEYWORDS;".to_string()),
        ],
    );
    path
}

#[test]
fn inpx_import_builds_a_local_collection() {
    let dir = tempfile::tempdir().unwrap();
    let inpx = sample_inpx(dir.path());
    let db = Db::open(dir.path().join("c.hlc2")).unwrap();

    let p = JobProgress::new();
    let report = import_inpx(&db, &inpx, &InpxOptions::default(), &p).unwrap();
    // 3 + 2 real records; the garbage line has no file name (ignored); extra.inp is skipped for a
    // local collection.
    assert_eq!(report.added, 5, "{report:?}");
    assert_eq!(report.existing, 0);
    assert_eq!(report.inp_files, 2);
    assert!(!report.cancelled);
    assert_eq!(p.percent(), 100.0);

    let conn = db.connection();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM Books", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 5);
    // Tolstoy is one author row shared by two books.
    let authors: i64 = conn.query_row("SELECT COUNT(*) FROM Authors", [], |r| r.get(0)).unwrap();
    assert_eq!(authors, 5); // Толстой, Ильф, Петров, Orwell, Неизвестный

    // Folder = archive name, InsideNo = line index, Ext with the dot, date in the original format.
    let (folder, inside, ext, date, local, deleted, title): (String, i64, String, String, i64, i64, String) = conn
        .query_row(
            "SELECT Folder, InsideNo, Ext, UpdateDate, IsLocal, IsDeleted, Title FROM Books WHERE FileName='102'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .unwrap();
    assert_eq!(folder, "fb2-000100-000102.zip");
    assert_eq!(inside, 2);
    assert_eq!(ext, ".fb2");
    assert_eq!(date, "2012-01-15");
    assert_eq!((local, deleted), (1, 0));
    assert_eq!(title, "Двенадцать стульев");

    // Two authors linked; series created with its number.
    let links: i64 = conn
        .query_row("SELECT COUNT(*) FROM Author_List al JOIN Books b ON b.BookID=al.BookID WHERE b.FileName='102'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 2);
    let (series, seq): (String, i64) = conn
        .query_row(
            "SELECT s.SeriesTitle, b.SeqNumber FROM Books b JOIN Series s ON s.SeriesID=b.SeriesID WHERE b.FileName='100'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((series.as_str(), seq), ("Эпопея", 1));
    let no_series: Option<i64> = conn.query_row("SELECT SeriesID FROM Books WHERE FileName='101'", [], |r| r.get(0)).unwrap();
    assert_eq!(no_series, None);

    // FB2 genre code translated to the catalog's universal code; an unknown code is filed under 0.0.
    let classic: String = conn.query_row("SELECT GenreCode FROM Genres WHERE FB2Code='prose_classic'", [], |r| r.get(0)).unwrap();
    let g100: String = conn
        .query_row("SELECT gl.GenreCode FROM Genre_List gl JOIN Books b ON b.BookID=gl.BookID WHERE b.FileName='100'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(g100, classic);
    let g102: Vec<String> = {
        let mut st = conn
            .prepare("SELECT gl.GenreCode FROM Genre_List gl JOIN Books b ON b.BookID=gl.BookID WHERE b.FileName='102' ORDER BY 1")
            .unwrap();
        let v = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        v
    };
    let humor: String = conn.query_row("SELECT GenreCode FROM Genres WHERE FB2Code='humor'", [], |r| r.get(0)).unwrap();
    assert_eq!(g102, vec!["0.0".to_string(), humor]);
    // Empty Lang / KeyWords are stored as '' like the original does, not NULL.
    let (lang, kw): (String, String) =
        conn.query_row("SELECT Lang, KeyWords FROM Books WHERE FileName='201'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((lang.as_str(), kw.as_str()), ("", ""));

    // The record with nothing but a file: normalized author/title/genre.
    let (t201, g201): (String, String) = conn
        .query_row(
            "SELECT b.Title, gl.GenreCode FROM Books b JOIN Genre_List gl ON gl.BookID=b.BookID WHERE b.FileName='201'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(t201, "Без названия");
    assert_eq!(g201, "0.0");
    // DEL=1 → soft-deleted, so the default views hide it.
    let del: i64 = conn.query_row("SELECT IsDeleted FROM Books WHERE FileName='200'", [], |r| r.get(0)).unwrap();
    assert_eq!(del, 1);

    // Search columns are populated (triggers were off during the import) — the DAO search works.
    let q = BookQuery { author_name_like: Some("толстой".into()), ..Default::default() };
    assert_eq!(db.count_books(&q).unwrap(), 2);
    let q = BookQuery { title_like: Some("стульев".into()), ..Default::default() };
    assert_eq!(db.count_books(&q).unwrap(), 1);

    // Collection properties from collection.info / version.info.
    assert_eq!(db.get_prop_str(props::PROP_NOTES).unwrap().as_deref(), Some("The notes"));
    assert_eq!(db.get_prop_str(props::PROP_URL).unwrap().as_deref(), Some("http://example.org/"));
    assert_eq!(db.get_prop_int(props::PROP_DATAVERSION).unwrap(), Some(20120115));

    // Re-import is idempotent.
    let again = import_inpx(&db, &inpx, &InpxOptions::default(), &JobProgress::new()).unwrap();
    assert_eq!((again.added, again.existing), (0, 5));
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM Books", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 5);

    let h = read_header(&inpx).unwrap().unwrap();
    assert_eq!(h.name, "Test Lib");
    assert_eq!(h.content_type, props::CT_EXTERNAL_LOCAL_FB);
}

#[test]
fn inpx_import_runs_as_a_background_job_next_to_an_open_connection() {
    let dir = tempfile::tempdir().unwrap();
    let inpx = sample_inpx(dir.path());
    let db_path = dir.path().join("c.hlc2");
    // The "GUI" connection stays open while the job writes through its own.
    let gui = Db::open(&db_path).unwrap();

    let (jp, ji) = (db_path.clone(), inpx.clone());
    let mut job = Job::spawn("import", move |p| {
        let db = Db::open(&jp)?;
        Ok(import_inpx(&db, &ji, &InpxOptions::default(), p)?.summary())
    });
    job.join();
    assert_eq!(job.progress.result(), Some(Ok("Добавлено 5 книг".to_string())));
    assert_eq!(gui.count_books(&BookQuery::default()).unwrap(), 4); // 5 minus the DEL=1 one

    // A job cancelled before it starts imports nothing and says so.
    let db2 = Db::open(dir.path().join("d.hlc2")).unwrap();
    let p = JobProgress::new();
    p.cancel();
    let r = import_inpx(&db2, &inpx, &InpxOptions::default(), &p).unwrap();
    assert!(r.cancelled);
    assert_eq!(r.added, 0);
}

#[test]
fn inpx_online_collection_generates_locations_and_reads_extra() {
    let dir = tempfile::tempdir().unwrap();
    let inpx = sample_inpx(dir.path());
    let db = Db::create_collection(dir.path().join("o.hlc2"), props::CT_EXTERNAL_ONLINE_FB, None).unwrap();
    let report = import_inpx(&db, &inpx, &InpxOptions::default(), &JobProgress::new()).unwrap();
    assert_eq!(report.added, 6); // extra.inp included for online collections
    let (folder, local): (String, i64) = db
        .connection()
        .query_row("SELECT Folder, IsLocal FROM Books WHERE FileName='100'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(folder, "Т\\Толстой Лев Николаевич\\100 Война и мир.fb2.zip");
    assert_eq!(local, 0);
}

#[test]
fn bad_inpx_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.inpx");
    std::fs::write(&bad, b"this is not a zip").unwrap();
    let db = Db::open(dir.path().join("c.hlc2")).unwrap();
    assert!(import_inpx(&db, &bad, &InpxOptions::default(), &JobProgress::new()).is_err());
    assert!(import_inpx(&db, &dir.path().join("missing.inpx"), &InpxOptions::default(), &JobProgress::new()).is_err());
}

#[test]
fn system_db_registers_collections_and_groups() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("data").join("user.dbs2");
    let sys = SystemDb::open(&sys_path).unwrap();

    // Built-in groups exist and cannot be deleted.
    let groups = sys.groups().unwrap();
    assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec![FAVORITES_GROUP_NAME, TO_READ_GROUP_NAME]);
    assert_eq!(groups[0].id, 1); // the original's FAVORITES_GROUP_ID
    assert!(!sys.delete_group(groups[0].id).unwrap());
    assert!(sys.add_group("Фантастика", true).unwrap());
    assert!(!sys.add_group("Фантастика", true).unwrap());
    let g = sys.groups().unwrap().into_iter().find(|g| g.name == "Фантастика").unwrap();
    assert!(!sys.rename_group(g.id, FAVORITES_GROUP_NAME).unwrap());
    assert!(sys.rename_group(g.id, "НФ").unwrap());
    assert!(sys.delete_group(g.id).unwrap());
    assert_eq!(sys.groups().unwrap().len(), 2);

    // Create a collection: file appears, is a valid collection of the requested type, is listed.
    let root = dir.path().join("books");
    let file = dir.path().join("data").join("lib.hlc2");
    let id = sys.create_collection("Моя библиотека", &root, &file, props::CT_EXTERNAL_LOCAL_FB, None).unwrap();
    assert!(file.is_file());
    assert_eq!(Db::probe_collection(&file), Some(props::CT_EXTERNAL_LOCAL_FB));
    let info = sys.collection(id).unwrap().unwrap();
    assert_eq!(info.name, "Моя библиотека");
    assert_eq!(info.db_file, file);
    assert_eq!(info.root_folder, root);
    assert_eq!(info.code, props::CT_EXTERNAL_LOCAL_FB);

    // Duplicates are refused; nothing is overwritten.
    assert!(sys.create_collection("Моя библиотека", &root, &dir.path().join("x.hlc2"), 0, None).is_err());
    assert!(sys.create_collection("Другая", &root, &file, 0, None).is_err());
    assert!(!dir.path().join("x.hlc2").exists());

    // Register an existing file; a non-collection file is refused.
    let other = dir.path().join("other.hlc2");
    drop(Db::open(&other).unwrap());
    let id2 = sys.register_collection(&other, "Вторая", dir.path()).unwrap();
    let junk = dir.path().join("junk.hlc2");
    std::fs::write(&junk, b"not sqlite").unwrap();
    assert!(sys.register_collection(&junk, "Мусор", dir.path()).is_err());
    assert_eq!(sys.collections().unwrap().len(), 2);

    sys.update_collection(id2, "Вторая (переим.)", dir.path(), "заметка", "", "u", "p", "").unwrap();
    assert!(sys.update_collection(id2, "Моя библиотека", dir.path(), "", "", "", "", "").is_err());

    // Survives a reopen; paths under the system DB folder are stored relative.
    drop(sys);
    let sys = SystemDb::open(&sys_path).unwrap();
    let all = sys.collections().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all.iter().find(|c| c.id == id2).unwrap().notes, "заметка");
    let stored: String = rusqlite::Connection::open(&sys_path)
        .map(|c| {
            // MHL_SYSTEM_NOCASE isn't registered on this raw connection; read a column that needs no collation.
            c.query_row("SELECT CAST(DBFileName AS BLOB) FROM Bases WHERE DatabaseID=?1", [id], |r| r.get::<_, Vec<u8>>(0))
                .map(|b| String::from_utf8(b).unwrap())
                .unwrap()
        })
        .unwrap();
    assert_eq!(stored, "lib.hlc2");

    // Delete: unregister only, then unregister + remove the file.
    sys.delete_collection(id2, false).unwrap();
    assert!(other.is_file());
    sys.delete_collection(id, true).unwrap();
    assert!(!file.exists());
    assert!(sys.collections().unwrap().is_empty());
}

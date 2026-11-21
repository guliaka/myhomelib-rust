//! GUI acceptance tests: navigation, multi-field search, statistics, settings persistence (all pure
//! Rust), plus a headless egui render smoke test. The windowed eframe runner is behind the `gui`
//! feature and isn't exercised here (no display).

use myhomelib::core::Book;
use myhomelib::db::Db;
use myhomelib::gui::state::{State, Selection, TABS};
use myhomelib::gui::view;
use myhomelib::librusec::catalog::{Catalog, CatalogBook};

fn book(title: &str, lang: &str, folder: Option<&str>) -> Book {
    Book {
        book_id: 0,
        lib_id: String::new(),
        title: title.into(),
        series_id: None,
        seq_number: None,
        update_date: "2026-01-01 00:00:00".into(),
        lib_rate: 0,
        lang: Some(lang.into()),
        folder: folder.map(|s| s.into()),
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

struct Fixture {
    db: Db,
    dir: std::path::PathBuf,
    db_path: std::path::PathBuf,
    tolstoy: i64,
    dostoevsky: i64,
    wiwar: i64,
}

static CTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fresh collection with two authors, one series, and three books across folders.
fn make_fixture() -> Fixture {
    let n = CTR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mhl_gui_test_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let db_path = dir.join("gui.db");
    let mut db = Db::open(db_path.clone()).unwrap();
    let tolstoy = db.upsert_author("Толстой", Some("Лев"), None).unwrap();
    let dostoevsky = db.upsert_author("Достоевский", Some("Фёдор"), None).unwrap();
    let wiwar = db.upsert_series("Война и мир").unwrap();

    let mut b1 = book("Война и мир", "ru", None);
    b1.series_id = Some(wiwar);
    b1.seq_number = Some(1);
    db.insert_book(&b1, &[tolstoy], &["prose_classic"]).unwrap();

    let b2 = book("Анна Каренина", "ru", Some("russian"));
    db.insert_book(&b2, &[tolstoy], &["prose_classic"]).unwrap();

    let b3 = book("Преступление и наказание", "ru", Some("russian"));
    db.insert_book(&b3, &[dostoevsky], &["prose_classic"]).unwrap();

    Fixture { db, dir, db_path, tolstoy, dostoevsky, wiwar }
}

fn titles(state: &mut State) -> Vec<String> {
    let n = state.list_total as i64;
    let mut v: Vec<String> = state.books_range(0, n).iter().map(|b| b.title.clone()).collect();
    v.sort();
    v
}

#[test]
fn navigation_by_author_genre_series_folder() {
    let Fixture { db, dir, db_path, tolstoy, dostoevsky, wiwar } = make_fixture();
    let mut state = State::new(db, db_path.clone());

    // All books.
    assert_eq!(state.selection, Selection::All);
    assert_eq!(titles(&mut state), vec!["Анна Каренина", "Война и мир", "Преступление и наказание"]);
    assert_eq!(state.total_books(), 3);

    // Authors tab: two nodes with correct counts.
    state.set_tab(0);
    let authors = state.tree_nodes();
    assert_eq!(authors.len(), 2);
    let tolstoy_node = authors.iter().find(|n| matches!(&n.selection, Selection::Author(id) if *id == tolstoy)).unwrap();
    assert_eq!(tolstoy_node.count, 2);

    // Select the Толстой author → his two books only.
    state.select(Selection::Author(tolstoy));
    assert_eq!(titles(&mut state), vec!["Анна Каренина", "Война и мир"]);

    // Series tab (1): one series with a single book.
    state.set_tab(1);
    let series = state.tree_nodes();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].count, 1);
    state.select(Selection::Series(wiwar));
    assert_eq!(titles(&mut state), vec!["Война и мир"]);

    // Genres tab (2): one genre in use, all three books share it.
    state.set_tab(2);
    let genres = state.tree_nodes();
    assert_eq!(genres.len(), 1);
    assert!(matches!(&genres[0].selection, Selection::Genre(c) if c == "prose_classic"));
    assert_eq!(genres[0].count, 3);
    state.select(Selection::Genre("prose_classic".into()));
    assert_eq!(state.list_total, 3);

    // Groups/Folders tab (4): root (1) + "russian" (2).
    state.set_tab(4);
    let folders = state.tree_nodes();
    assert_eq!(folders.len(), 2);
    state.select(Selection::Folder(Some("russian".into())));
    assert_eq!(titles(&mut state), vec!["Анна Каренина", "Преступление и наказание"]);

    // The other author's node resolves correctly too.
    state.select(Selection::Author(dostoevsky));
    assert_eq!(titles(&mut state), vec!["Преступление и наказание"]);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_filters_by_field() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());

    // Author filter narrows to Толстой's two books.
    state.filters.author = "Толстой".into();
    state.reload();
    assert_eq!(titles(&mut state), vec!["Анна Каренина", "Война и мир"]);

    // Combined author + title → the single matching book.
    state.filters.title = "Анна".into();
    state.reload();
    assert_eq!(titles(&mut state), vec!["Анна Каренина"]);

    // Language filter (exact) — all three are ru.
    state.clear_search();
    state.filters.lang = "ru".into();
    state.reload();
    assert_eq!(state.list_total, 3);

    // Extension filter (exact) — all fb2.
    state.clear_search();
    state.filters.ext = "fb2".into();
    state.reload();
    assert_eq!(state.list_total, 3);

    // Local-only — all local.
    state.clear_search();
    state.filters.only_local = true;
    state.reload();
    assert_eq!(state.list_total, 3);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stats_breakdowns() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let state = State::new(db, db_path.clone());
    let s = state.stats_data();
    assert_eq!(s.books, 3);
    assert_eq!(s.authors, 2); // Толстой, Достоевский
    assert_eq!(s.series, 1); // one series catalog row
    assert_eq!(s.genres, 1); // prose_classic (upserted by insert_book)
    assert_eq!(s.by_lang, vec![("ru".to_string(), 3)]);
    assert_eq!(s.by_ext, vec![("fb2".to_string(), 3)]);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_roundtrip_ini() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());

    // Set a reader association + active tab; both persist to `<db>.ini`.
    state.reader_ext = "fb2".into();
    state.reader_cmd = "/opt/reader %f".into();
    state.apply_reader();
    state.set_tab(2);

    let ini_path = myhomelib::settings::Settings::path_for(&db_path);
    assert!(ini_path.exists(), "settings should be written to <db>.ini");
    let reloaded = myhomelib::settings::Settings::load(&db_path);
    assert_eq!(reloaded.reader_for("fb2").as_deref(), Some("/opt/reader %f"));
    assert_eq!(reloaded.active_tab(), 2);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_smoke_headless() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());

    let ctx = egui::Context::default();
    // Frame 1: default. Frame 2: a book selected (detail panel). Frame 3: stats dialog open.
    let mut icons = view::IconCache::default();
    for frame in 0..3 {
        if frame == 0 && state.list_total > 0 {
            if let Some(b) = state.books_range(0, 1).into_iter().next() {
                state.select_book_id(b.book_id);
            }
        }
        if frame == 2 {
            state.show_stats = true;
        }
        let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons)); // no panic
    }

    // Walk every page-control tab and render it headlessly — catches a panic in any per-tab layout
    // (menu/toolbar/tab-bar/nav/search/downloads) that the single default-frame loop above doesn't reach.
    for t in 0..TABS.len() {
        state.set_tab(t);
        let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons)); // no panic
    }

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn online_search_queue_and_config() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());

    // A background search "completes" by writing a catalog into the shared slot.
    let cat = Catalog {
        base_url: Some("http://x/download/".into()),
        books: vec![
            CatalogBook {
                title: "Война и мир".into(),
                authors: vec![("Толстой".into(), "Лев".into(), String::new())],
                genres: vec!["prose_classic".into()],
                series: None,
                libid: Some("lib001".into()),
                lang: Some("ru".into()),
                ext: Some("fb2".into()),
                url: Some("tw/voyna.fb2.zip".into()),
            },
            CatalogBook {
                title: "Преступление и наказание".into(),
                authors: vec![("Достоевский".into(), "Фёдор".into(), String::new())],
                genres: vec!["sf".into()],
                series: None,
                libid: Some("lib002".into()),
                lang: Some("ru".into()),
                ext: Some("fb2".into()),
                url: Some("ds/pn.fb2.zip".into()),
            },
        ],
    };
    *state.online_slot.lock().unwrap() = Some(Ok(cat));
    state.online_query = "война".into();
    state.poll_online();
    assert_eq!(state.online_results, vec![0], "only book 0 matches “война”");
    assert!(state.online_catalog.is_some());

    // Queue the matching book; its download URL resolves onto the catalog base.
    let id = state.queue_download(0).unwrap();
    assert_eq!(state.downloads.len(), 1);
    assert_eq!(state.downloads.get(id).unwrap().url, "http://x/download/tw/voyna.fb2.zip");

    // Librusec config round-trips through `<db>.ini` (including a multi-line script).
    state.lib_user = "alice".into();
    state.lib_url = "http://lib.rus.ec".into();
    state.lib_script = "GET %URL%/login\nCHECK".into();
    state.save_lib_config();
    let reloaded = myhomelib::settings::Settings::load(&db_path);
    assert_eq!(reloaded.librusec_user(), "alice");
    assert_eq!(reloaded.librusec_url(), "http://lib.rus.ec");
    assert_eq!(reloaded.librusec_script(), "GET %URL%/login\nCHECK");

    // Render the Online panel headlessly with a result + an in-flight download (progress bar path).
    state.downloads.set_progress(id, 50, Some(100));
    state.show_online = true;
    let ctx = egui::Context::default();
    let mut icons = view::IconCache::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons)); // no panic

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

const BULGAKOV_FB2: &str = r##"<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2">
  <description>
    <title-info>
      <id>gui-import-1</id>
      <genre>prose_classic</genre>
      <author><last-name>Булгаков</last-name><first-name>Михаил</first-name></author>
      <book-title>Мастер и Маргарита</book-title>
      <lang>ru</lang>
    </title-info>
  </description>
  <body><section><p>Текст.</p></section></body>
</FictionBook>"##;

#[test]
fn import_dir_refreshes_state() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());
    assert_eq!(state.total_books(), 3);

    // A folder with one new book (distinct author/title) to import.
    let import_dir = dir.join("inbound");
    std::fs::create_dir_all(&import_dir).unwrap();
    std::fs::write(import_dir.join("master.fb2"), BULGAKOV_FB2).unwrap();

    // Import it through State (the headless equivalent of the Import button's work).
    let report = state.import_dir(&import_dir).unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.existing, 0);
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

    // The view refreshed: total went up and the new book is listed.
    assert_eq!(state.total_books(), 4);
    let all = titles(&mut state);
    assert!(all.contains(&"Мастер и Маргарита".to_string()), "new book listed: {all:?}");

    // The status bar reports the import.
    assert!(state.status.contains("Imported 1 book(s)"), "status: {}", state.status);

    // Re-importing the same folder dedupes — nothing new, total unchanged.
    let report2 = state.import_dir(&import_dir).unwrap();
    assert_eq!(report2.imported, 0);
    assert_eq!(report2.existing, 1);
    assert_eq!(state.total_books(), 4);

    // The new author shows up in the rebuilt Authors tree (Толстой, Достоевский, Булгаков).
    state.set_tab(0);
    let authors = state.tree_nodes();
    assert_eq!(authors.len(), 3, "author tree: {authors:?}");

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Background INPX import driven the way the window drives it: start the job, keep rendering the
/// progress page while it runs, acknowledge it, and see the views refreshed.
#[test]
fn inpx_import_job_shows_progress_and_refreshes_views() {
    use std::io::Write;
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::new(db, db_path.clone());
    assert_eq!(state.total_books(), 3);

    let d = '\u{4}';
    let line = |author: &str, title: &str, file: &str| {
        [author, "prose_classic:", title, "", "", file, "10", file, "0", "fb2", "2020-01-01", "ru", "", ""].join(&d.to_string())
    };
    let inp = [line("Булгаков,Михаил,:", "Мастер и Маргарита", "500"), line("Чехов,Антон,:", "Вишнёвый сад", "501")].join("\r\n");
    let inpx = dir.join("lib.inpx");
    let mut zw = zip::ZipWriter::new(std::fs::File::create(&inpx).unwrap());
    zw.start_file("fb2-500-501.inp", zip::write::SimpleFileOptions::default()).unwrap();
    zw.write_all(inp.as_bytes()).unwrap();
    zw.finish().unwrap();

    assert!(state.start_inpx_import(&inpx));
    assert!(state.job_active());
    // A second job is refused while one is active.
    assert!(!state.start_inpx_import(&inpx));

    // Render the progress page until the job finishes (bounded wait).
    let ctx = egui::Context::default();
    let mut icons = view::IconCache::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));
        if state.job.as_ref().unwrap().is_done() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "import job did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // One more frame with the finished job (Close button + summary) must render too.
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));

    state.finish_job();
    assert!(!state.job_active());
    assert_eq!(state.status, "Добавлено 2 книг");
    assert_eq!(state.total_books(), 5);
    state.set_tab(0);
    assert_eq!(state.tree_nodes().len(), 4, "authors: Толстой, Достоевский, Булгаков, Чехов");
    // Back on the normal window.
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));

    // Folder import through the same job path.
    let inbound = dir.join("inbound2");
    std::fs::create_dir_all(&inbound).unwrap();
    std::fs::write(inbound.join("master.fb2"), BULGAKOV_FB2).unwrap();
    assert!(state.start_folder_import(&inbound));
    state.job.as_mut().unwrap().join();
    state.finish_job();
    assert_eq!(state.total_books(), 6);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Multiple collections: the wizard creates a collection, the switcher opens another one, the
/// properties dialog renames it, and delete swaps to a remaining collection. All through the
/// `State` methods the GUI dialogs call, with an isolated `user.dbs2` (never the real one).
#[test]
fn collection_manager_switch_props_delete() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let sysdb = dir.join("user.dbs2");
    let mut state = State::with_sysdb(db, db_path.clone(), sysdb.clone());

    // Not registered yet — the switcher shows nothing, the id is None.
    assert!(state.collections().is_empty());
    assert!(state.current_collection_id().is_none());
    assert!(state.register_current_collection("Основная"));
    assert_eq!(state.current_collection_name().as_deref(), Some("Основная"));

    // Wizard: create a second, empty collection; it becomes the open one.
    state.new_col_name = "Вторая".into();
    state.new_col_file = dir.join("second.hlc2").to_string_lossy().into_owned();
    state.new_col_root = dir.join("books").to_string_lossy().into_owned();
    assert!(state.create_collection_from_wizard());
    assert_eq!(state.current_collection_name().as_deref(), Some("Вторая"));
    assert_eq!(state.total_books(), 0, "the new collection is empty");
    assert!(state.new_col_file.trim().is_empty() || state.show_new_collection == false);

    // The old collection still has its books; switch back by id and see them.
    let main_id = state
        .collections()
        .iter()
        .find(|c| c.name == "Основная")
        .expect("main registered")
        .id;
    assert!(state.switch_collection(main_id));
    assert_eq!(state.current_collection_name().as_deref(), Some("Основная"));
    assert_eq!(state.total_books(), 3, "books survived the switch");
    assert_eq!(state.sysdb_path, sysdb, "system DB path is preserved across switches");

    // Properties: rename via the dialog buffers.
    let second_id = state
        .collections()
        .iter()
        .find(|c| c.name == "Вторая")
        .expect("second registered")
        .id;
    assert!(state.load_collection_props(second_id));
    assert_eq!(state.props_name, "Вторая");
    state.props_name = "Вторая (переименована)".into();
    state.props_notes = "заметка".into();
    assert!(state.save_collection_props());
    assert!(state
        .collections()
        .iter()
        .any(|c| c.name == "Вторая (переименована)" && c.notes == "заметка"));

    // Delete the open collection (registry entry only): switches to the remaining one.
    assert!(state.delete_collection(main_id, false));
    assert_eq!(state.current_collection_name().as_deref(), Some("Вторая (переименована)"));
    assert!(state.db_path.exists(), "registry-only delete keeps the file");
    assert_eq!(state.collections().len(), 1);

    // Delete the last one too: registry empty, status says there's nothing to switch to.
    let last_id = state.collections()[0].id;
    assert!(state.delete_collection(last_id, true));
    assert!(state.collections().is_empty());
    assert!(!dir.join("second.hlc2").exists(), "--remove-file deleted the database file");

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Wizard validation: an empty name or file is refused and nothing is created.
#[test]
fn collection_wizard_requires_name_and_file() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::with_sysdb(db, db_path, dir.join("user.dbs2"));

    state.new_col_name = String::new();
    state.new_col_file = dir.join("x.hlc2").to_string_lossy().into_owned();
    assert!(!state.create_collection_from_wizard());
    assert!(state.collections().is_empty());

    state.new_col_name = "Имя".into();
    state.new_col_file = String::new();
    assert!(!state.create_collection_from_wizard());
    assert!(state.collections().is_empty());

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The collection dialogs render headlessly without panicking (switcher combo, manager, wizard,
/// properties) — same pattern as `render_smoke_headless`.
#[test]
fn render_collection_dialogs_headless() {
    let Fixture { db, dir, db_path, .. } = make_fixture();
    let mut state = State::with_sysdb(db, db_path, dir.join("user.dbs2"));
    assert!(state.register_current_collection("Основная"));
    state.new_col_name = "Вторая".into();
    state.new_col_file = dir.join("second.hlc2").to_string_lossy().into_owned();
    assert!(state.create_collection_from_wizard());

    let ctx = egui::Context::default();
    let mut icons = view::IconCache::default();
    state.show_collections = true;
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));
    state.show_new_collection = true;
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));
    assert!(state.load_collection_props(state.collections()[0].id));
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));
    // A frame with no dialogs open (combo renders with a registered current collection).
    state.show_collections = false;
    state.show_new_collection = false;
    state.show_collection_props = false;
    let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state, &mut icons));

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

//! System-DB registry + multi-collection management + maintenance (compact/repair) tests.

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};
use myhomelib::gui::state::{State, WizardData};
use myhomelib::systemdb::{CollectionInfo, SystemDb};
use tempfile::TempDir;

fn info(name: &str, db_file: &str) -> CollectionInfo {
    CollectionInfo {
        name: name.into(),
        db_file_name: db_file.into(),
        ..Default::default()
    }
}

#[test]
fn system_db_crud_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let sys_path = tmp.path().join("MyHomeLib.system.db");
    let sys = SystemDb::open(&sys_path).unwrap();

    assert!(sys.list().unwrap().is_empty());

    // add
    let c1 = tmp.path().join("books1.db").to_string_lossy().to_string();
    let id1 = sys.add(&info("Домашняя", &c1)).unwrap();
    let c2 = tmp.path().join("books2.db").to_string_lossy().to_string();
    let id2 = sys.add(&info("Загрузки", &c2)).unwrap();
    assert_ne!(id1, id2);

    // duplicate name rejected (BaseName is unique)
    assert!(sys.add(&info("Домашняя", &c2)).is_err());

    // list
    let rows = sys.list().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "Домашняя");
    assert_eq!(rows[1].name, "Загрузки");
    assert!(!rows[0].creation_date.is_empty());

    // get / find_by_name
    assert_eq!(sys.get(id1).unwrap().unwrap().name, "Домашняя");
    assert_eq!(sys.find_by_name("Загрузки").unwrap().unwrap().id, id2);
    assert!(sys.get(999).unwrap().is_none());

    // update
    let mut p = sys.get(id1).unwrap().unwrap();
    p.notes = "основная коллекция".into();
    p.url = "http://flibusta.example".into();
    sys.update(&p).unwrap();
    let p2 = sys.get(id1).unwrap().unwrap();
    assert_eq!(p2.notes, "основная коллекция");
    assert_eq!(p2.url, "http://flibusta.example");

    // delete + idempotent reopen
    sys.delete(id2).unwrap();
    assert_eq!(sys.list().unwrap().len(), 1);
    drop(sys);
    let sys = SystemDb::open(&sys_path).unwrap();
    assert_eq!(sys.list().unwrap().len(), 1);
}

#[test]
fn collection_info_flags() {
    let mut c = info("online", "x.db");
    assert!(!c.is_online());
    c.code = 0x0801_0000; // CT_EXTERNAL_ONLINE_FB
    assert!(c.is_online());
    assert!(!c.file_exists());
}

#[test]
fn maintenance_check_compact_repair() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("coll.db");
    let mut db = Db::open(&db_path).unwrap();

    // A book so there's data (and search columns) to repair.
    let a = db.upsert_author("Толстой", Some("Лев"), Some("Николаевич")).unwrap();
    db.upsert_genre("prose.classic", None, Some("classic"), "Классическая проза").unwrap();
    let b = Book {
        book_id: 0,
        lib_id: "t-1".into(),
        title: "Война и мир".into(),
        series_id: None,
        seq_number: None,
        update_date: "2026-11-21 12:00:00".into(),
        lib_rate: 0,
        lang: Some("ru".into()),
        folder: None,
        file_name: "voyna.fb2".into(),
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
    };
    db.insert_book(&b, &[a], &["prose.classic"]).unwrap();

    // Corrupt the derived search column directly (as another tool with triggers disabled would).
    db.connection()
        .execute("UPDATE Books SET SearchTitle = ''", [])
        .unwrap();

    // repair rebuilds it
    let n = db.repair().unwrap();
    assert!(n >= 2); // 1 book + 1 author
    let title: String = db
        .connection()
        .query_row("SELECT SearchTitle FROM Books LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title.to_uppercase(), "ВОЙНА И МИР".to_uppercase());

    // search works again after repair
    let q = BookQuery {
        title_like: Some("война".into()),
        ..Default::default()
    };
    assert_eq!(db.count_books(&q).unwrap(), 1);

    // compact + integrity check
    db.compact().unwrap();
    db.integrity_check().unwrap();
    assert!(db.file_size_bytes().unwrap() > 0);
}

#[test]
fn state_wizard_create_switch_delete() {
    let tmp = TempDir::new().unwrap();
    let sys_path = tmp.path().join("MyHomeLib.system.db");
    let sys = SystemDb::open(&sys_path).unwrap();

    // Start with an unregistered placeholder collection (as on first launch).
    let first = tmp.path().join("initial.db");
    Db::open(&first).unwrap();
    let mut state = State::new(Db::open(&first).unwrap(), first.to_string_lossy().to_string());
    state.attach_system(sys);

    // Empty wizard input is rejected with an error, dialog stays open.
    state.show_wizard = true;
    state.create_collection();
    assert!(state.show_wizard);
    assert!(!state.collection_error.is_empty());

    // Wizard creates + registers + switches.
    let w1 = tmp.path().join("home.db");
    state.wizard = WizardData {
        name: "Домашняя".into(),
        db_file: w1.to_string_lossy().to_string(),
        root_folder: "".into(),
        notes: "".into(),
        online: false,
    };
    state.create_collection();
    assert!(!state.show_wizard);
    assert!(state.collection_error.is_empty());
    assert_eq!(state.db_path, w1);
    assert_eq!(state.collection_rows.len(), 1);

    // Second collection, then switch between them via the registry.
    let w2 = tmp.path().join("dloads.db");
    state.wizard = WizardData {
        name: "Загрузки".into(),
        db_file: w2.to_string_lossy().to_string(),
        ..Default::default()
    };
    state.create_collection();
    assert_eq!(state.db_path, w2);
    assert_eq!(state.collection_rows.len(), 2);
    state.switch_collection(w1.to_string_lossy().as_ref());
    assert_eq!(state.db_path, w1);

    // Properties round-trip for the open collection.
    state.open_properties();
    assert!(state.show_properties);
    state.props.as_mut().unwrap().notes = "проверка".into();
    state.save_properties();
    assert!(!state.show_properties);
    let home = state.collection_rows.iter().find(|c| c.name == "Домашняя").unwrap();
    assert_eq!(home.notes, "проверка");

    // Remove the non-open collection from the registry (files kept).
    let id2 = state.collection_rows.iter().find(|c| c.name == "Загрузки").unwrap().id;
    state.remove_collection(id2, false);
    assert_eq!(state.collection_rows.len(), 1);
    assert_eq!(state.db_path, w1); // still on the open collection
    assert!(w2.is_file()); // files kept

    // Purge-deleting the OPEN collection would delete the open file with no fallback — must refuse.
    let id1 = state.collection_rows[0].id;
    state.remove_collection(id1, true);
    assert!(!state.collection_error.is_empty());
    assert!(w1.is_file());
    assert_eq!(state.collection_rows.len(), 1);
}

#[test]
fn remove_current_without_purge_falls_back() {
    let tmp = TempDir::new().unwrap();
    let sys = SystemDb::open(tmp.path().join("s.db")).unwrap();
    let a = tmp.path().join("a.db");
    let b = tmp.path().join("b.db");
    Db::open(&a).unwrap();
    Db::open(&b).unwrap();
    sys.add(&info("A", &a.to_string_lossy())).unwrap();
    sys.add(&info("B", &b.to_string_lossy())).unwrap();

    let mut state = State::new(Db::open(&a).unwrap(), a.to_string_lossy().to_string());
    state.attach_system(sys);
    state.refresh_collections();

    // Unregister the open collection without purging: the app falls back to the next existing one.
    let ida = state.collection_rows.iter().find(|c| c.name == "A").unwrap().id;
    state.remove_collection(ida, false);
    assert_eq!(state.db_path, b);
    assert_eq!(state.collection_rows.len(), 1);
}

#[test]
fn purge_files_removes_db_and_ini() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("gone.db");
    Db::open(&db_path).unwrap();
    let ini = myhomelib::settings::Settings::path_for(&db_path);
    std::fs::write(&ini, "[Window]\n").unwrap();

    SystemDb::purge_files(&db_path.to_string_lossy()).unwrap();
    assert!(!db_path.exists());
    assert!(!ini.exists());
}

#[test]
fn group_editor_flow() {
    use myhomelib::core::Book;
    use myhomelib::gui::state::{GroupEditMode, Selection, State, TABS};

    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("coll.db");
    let mut db = Db::open(&db_path).unwrap();
    let a = db.upsert_author("Толстой", Some("Лев"), None).unwrap();
    db.insert_book(
        &Book {
            book_id: 0,
            lib_id: "lib-1".into(),
            title: "Война и мир".into(),
            series_id: None,
            seq_number: None,
            update_date: "2026-11-21 12:00:00".into(),
            lib_rate: 0,
            lang: Some("ru".into()),
            folder: None,
            file_name: "voina.fb2".into(),
            inside_no: 0,
            ext: Some("fb2".into()),
            book_size: Some(1),
            is_local: true,
            is_deleted: false,
            key_words: None,
            rate: 0,
            progress: 0,
            annotation: None,
            review: None,
        },
        &[a],
        &[],
    )
    .unwrap();

    let sys = SystemDb::open(tmp.path().join("s.db")).unwrap();
    sys.add(&info("Библиотека", &db_path.to_string_lossy())).unwrap();
    let mut state = State::new(Db::open(&db_path).unwrap(), db_path.to_string_lossy().to_string());
    state.attach_system(sys);

    // attach_system seeds the original's protected defaults.
    let names: Vec<String> = state.groups_cache.iter().map(|g| g.1.clone()).collect();
    assert!(names.contains(&"Избранное".to_string()));
    assert!(names.contains(&"К прочтению".to_string()));

    // Create a group through the editor dialog flow.
    state.open_group_edit(GroupEditMode::Create);
    state.group_edit.as_mut().unwrap().1 = "Прочитать летом".into();
    state.apply_group_edit();
    assert!(state.group_edit.is_none());
    assert_eq!(state.groups_cache.len(), 3);

    // Add the selected book to it (the «Добавить в группу» picker action).
    let gid = state.groups_cache.iter().find(|g| g.1 == "Прочитать летом").unwrap().0;
    state.select_book_id(1);
    state.add_selected_to_group(gid);
    state.refresh_groups_cache();
    let entry = state.groups_cache.iter().find(|g| g.0 == gid).unwrap();
    assert_eq!(entry.2, 1);

    // The Группы tab shows the group with its count; selecting it lists the member book.
    state.set_tab(TABS.iter().position(|t| *t == "Группы").unwrap());
    let nodes = state.tree_nodes();
    let node = nodes.iter().find(|n| matches!(n.selection, Selection::Group(g) if g == gid)).unwrap();
    assert_eq!(node.count, 1);
    state.select(Selection::Group(gid));
    assert_eq!(state.list_total, 1);
    let books = state.books_range(0, 10);
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Война и мир");

    // Duplicate add is a no-op.
    state.add_selected_to_group(gid);
    assert_eq!(state.list_total, 1);

    // Remove from group: book disappears from the group view.
    state.remove_selected_from_group();
    assert_eq!(state.list_total, 0);

    // Re-add, then clear the group (books stay in the library).
    state.add_selected_to_group(gid);
    state.delete_or_clear_group(gid, true);
    assert_eq!(state.list_total, 0); // cleared group stays selected, now empty
    assert_eq!(state.db.count_books(&Default::default()).unwrap(), 1);

    // Rename.
    state.open_group_edit(GroupEditMode::Rename(gid));
    state.group_edit.as_mut().unwrap().1 = "Архив".into();
    state.apply_group_edit();
    state.refresh_groups_cache();
    assert!(state.groups_cache.iter().any(|g| g.1 == "Архив"));

    // Delete the user group; protected «Избранное» refuses deletion.
    state.delete_or_clear_group(gid, false);
    assert!(state.groups_cache.iter().all(|g| g.0 != gid));
    let fav = state.groups_cache.iter().find(|g| g.1 == "Избранное").unwrap().0;
    state.delete_or_clear_group(fav, false);
    assert!(state.groups_cache.iter().any(|g| g.0 == fav));
    assert!(state.status.contains("нельзя удалить"));
}

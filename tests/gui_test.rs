//! Phase 4 acceptance: the GUI state/navigation logic (pure Rust) and a headless egui render smoke
//! test. The windowed eframe runner is behind the `gui` feature and isn't exercised here (no display).

use myhomelib::core::Book;
use myhomelib::db::Db;
use myhomelib::gui::state::{State, Selection};
use myhomelib::gui::view;

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

    let mut db = Db::open(dir.join("gui.db")).unwrap();
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

    Fixture { db, dir, tolstoy, dostoevsky, wiwar }
}

fn titles(state: &State) -> Vec<String> {
    let mut v: Vec<String> = state.books.iter().map(|b| b.title.clone()).collect();
    v.sort();
    v
}

#[test]
fn navigation_by_author_genre_series_folder() {
    let Fixture { db, dir, tolstoy, dostoevsky, wiwar } = make_fixture();
    let mut state = State::new(db);

    // All books.
    assert_eq!(state.selection, Selection::All);
    assert_eq!(titles(&state), vec!["Анна Каренина", "Война и мир", "Преступление и наказание"]);
    assert_eq!(state.total_books(), 3);

    // Authors tab: two nodes with correct counts.
    state.tab = 0;
    let authors = state.tree_nodes();
    assert_eq!(authors.len(), 2);
    let tolstoy_node = authors.iter().find(|n| matches!(&n.selection, Selection::Author(id) if *id == tolstoy)).unwrap();
    assert_eq!(tolstoy_node.count, 2);

    // Select the Толстой author → his two books only.
    state.select(Selection::Author(tolstoy));
    assert_eq!(titles(&state), vec!["Анна Каренина", "Война и мир"]);

    // Genres tab: one genre in use, all three books share it.
    state.tab = 1;
    let genres = state.tree_nodes();
    assert_eq!(genres.len(), 1);
    assert!(matches!(&genres[0].selection, Selection::Genre(c) if c == "prose_classic"));
    assert_eq!(genres[0].count, 3);
    state.select(Selection::Genre("prose_classic".into()));
    assert_eq!(state.books.len(), 3);

    // Series tab: one series with a single book.
    state.tab = 2;
    let series = state.tree_nodes();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].count, 1);
    state.select(Selection::Series(wiwar));
    assert_eq!(titles(&state), vec!["Война и мир"]);

    // Folders tab: root (1) + "russian" (2).
    state.tab = 3;
    let folders = state.tree_nodes();
    assert_eq!(folders.len(), 2);
    state.select(Selection::Folder(Some("russian".into())));
    assert_eq!(titles(&state), vec!["Анна Каренина", "Преступление и наказание"]);

    // The other author's node resolves correctly too.
    state.select(Selection::Author(dostoevsky));
    assert_eq!(titles(&state), vec!["Преступление и наказание"]);

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_filters_and_delete_removes() {
    let Fixture { db, dir, .. } = make_fixture();
    let mut state = State::new(db);

    // Search narrows the list.
    state.select(Selection::All);
    state.set_search("Анна");
    assert_eq!(titles(&state), vec!["Анна Каренина"]);

    // Clearing search returns to the tree selection (All).
    state.set_search("");
    assert_eq!(state.books.len(), 3);

    // Delete the selected book; it disappears from All and the count drops.
    state.select(Selection::All);
    let idx = state.books.iter().position(|b| b.title == "Анна Каренина").unwrap();
    state.select_book(idx);
    state.delete_selected_book();
    assert_eq!(state.total_books(), 2);
    assert!(!titles(&state).contains(&"Анна Каренина".to_string()));

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_smoke_headless() {
    let Fixture { db, dir, .. } = make_fixture();
    let mut state = State::new(db);

    let ctx = egui::Context::default();
    // Frame 1: default selection. Frame 2: a book selected (exercises the detail panel).
    for frame in 0..2 {
        let _ = ctx.run(egui::RawInput::default(), |ctx| view::render(ctx, &mut state)); // no panic
        if frame == 0 && !state.books.is_empty() {
            state.select_book(0);
        }
    }

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}

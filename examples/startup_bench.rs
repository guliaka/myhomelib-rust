//! Cold-start benchmark for a large library. Measures the real startup path — open DB + build GUI
//! state + first render frame — and shows it stays flat as the collection grows, because the book
//! list is virtualized (`COUNT(*)` + one visible-window fetch, not a full load). The final line times
//! the eager full-load we deliberately no longer do, for contrast.
//!
//! Run: `cargo run --release --example startup_bench [num_books]`

use std::time::Instant;

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};
use myhomelib::gui::state::State;

fn make_book(i: usize) -> Book {
    Book {
        book_id: 0,
        lib_id: format!("lib-{i}"),
        title: format!("Book {i:08}"),
        series_id: if i % 50 == 0 { Some((i / 1000) as i64) } else { None },
        seq_number: None,
        update_date: "2026-01-01 00:00:00".to_string(),
        lib_rate: 0,
        lang: Some(if i % 3 == 0 { "ru".into() } else { "en".into() }),
        folder: Some(format!("folder_{:02}", i % 40)),
        file_name: format!("{i}.fb2"),
        inside_no: 0,
        ext: Some("fb2".to_string()),
        book_size: Some(1_000_000),
        is_local: true,
        is_deleted: false,
        key_words: None,
        rate: 0,
        progress: 0,
        annotation: None,
        review: None,
    }
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let dir = std::env::temp_dir().join(format!("mhl_bench_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("bench.db");

    // 1) Seed a large collection (setup, not counted as startup).
    let t_seed = Instant::now();
    {
        let mut db = Db::open(db_path.clone()).unwrap();
        for s in 0..200i64 {
            let _ = db.upsert_series(&format!("Series {s}"));
        }
        const CHUNK: usize = 250_000;
        let mut done = 0;
        while done < n {
            let end = (done + CHUNK).min(n);
            let chunk: Vec<Book> = (done..end).map(make_book).collect();
            db.insert_books_bulk(&chunk).unwrap();
            done = end;
        }
    }
    println!("seeded {n} books in {:?}   (setup only)", t_seed.elapsed());

    // 2) Cold start: fresh open + build GUI state + first render frame.
    let t_open = Instant::now();
    let db = Db::open(db_path.clone()).unwrap();
    let mut state = State::new(db, db_path.clone());
    println!(
        "cold open + State::new:  {:?}   (list_total={})",
        t_open.elapsed(),
        state.list_total
    );

    // First headless render frame — the virtualized list fetches only the visible window.
    let t_render = Instant::now();
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| myhomelib::gui::view::render(ctx, &mut state));
    println!("first render frame:     {:?}", t_render.elapsed());

    // Tree nodes per tab (aggregate COUNT GROUP BY over the whole collection) — first build of each.
    for (i, name) in ["Authors", "Genres", "Series", "Folders"].iter().enumerate() {
        state.set_tab(i);
        let t = Instant::now();
        let nodes = state.tree_nodes();
        println!("  tree {name:8} ({:5} nodes): {:?}", nodes.len(), t.elapsed());
    }

    // Re-visit the slowest tab — now served from the per-tab cache, not re-queried.
    state.set_tab(3);
    let t = Instant::now();
    let _ = state.tree_nodes();
    println!("  tree Folders re-visit (cached): {:?}", t.elapsed());

    // 3) What we NO LONGER do at startup: load every row into memory. Shown for contrast only.
    let t_eager = Instant::now();
    let db2 = Db::open(db_path.clone()).unwrap();
    let all = db2.search_books(&BookQuery::default(), -1, 0).unwrap();
    println!(
        "eager full load (AVOIDED): {:?}   -> {} rows in memory",
        t_eager.elapsed(),
        all.len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

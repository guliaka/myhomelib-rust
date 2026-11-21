//! Time the first build of each left-panel tree on an existing collection:
//! `cargo run --release --example tree_bench <collection-db>`
use std::time::Instant;

use myhomelib::db::Db;
use myhomelib::gui::state::{State, TABS};

fn main() {
    let path = std::env::args().nth(1).expect("usage: tree_bench <collection-db>");
    let t = Instant::now();
    let db = Db::open(&path).expect("open");
    let mut state = State::new(db, &path);
    println!("open + State::new: {:?} ({} books)", t.elapsed(), state.list_total);
    for tab in [0usize, 1, 2, 4] {
        state.set_tab(tab);
        let t = Instant::now();
        let n = state.tree().len();
        println!("{:<10} {:>7} nodes, first build {:?}", TABS[tab], n, t.elapsed());
        let t = Instant::now();
        let _ = state.tree().len();
        println!("{:<10} re-visit {:?}", "", t.elapsed());
    }
}

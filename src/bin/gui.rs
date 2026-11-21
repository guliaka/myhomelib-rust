//! Native GUI entry point: `cargo run --features gui --bin myhomelib-gui [db-path]`.
//!
//! Only builds with the `gui` feature (see Cargo.toml `required-features`). The DB is opened up front
//! so a missing/corrupt collection fails with a clear message before the window starts.

use std::path::Path;

fn main() {
    let db_path = std::env::args().nth(1).unwrap_or_else(|| "collection.db".to_string());

    if let Err(e) = myhomelib::db::Db::open(&db_path) {
        eprintln!("failed to open {db_path}: {e}");
        std::process::exit(1);
    }

    // Restore the persisted window size from `<db>.ini` (defaults to 1024×700 if unset).
    let [w, h] = myhomelib::settings::Settings::load(Path::new(&db_path)).window_size();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([w, h])
            .with_title("MyHomeLib"),
        ..Default::default()
    };

    // The DB is already verified open above, so this won't fail in practice.
    if let Err(e) = eframe::run_native(
        "MyHomeLib",
        options,
        Box::new(|_cc| Ok(Box::new(myhomelib::gui::app::MyHomeLibApp::new(&db_path).unwrap()))),
    ) {
        eprintln!("GUI error: {e}");
        std::process::exit(1);
    }
}

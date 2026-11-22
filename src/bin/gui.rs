//! Native GUI entry point: `cargo run --features gui --bin myhomelib-gui [db-path]`.
//!
//! Only builds with the `gui` feature (see Cargo.toml `required-features`). With an explicit
//! `db-path` argument that collection opens directly; otherwise the system DB
//! (`MyHomeLib.system.db`, see [`myhomelib::systemdb::SystemDb::default_path`]) decides: the first
//! registered collection whose file still exists (the original's `FindFirstExistingCollectionID`),
//! or a fresh unregistered collection + the new-collection wizard on first launch.

use std::path::{Path, PathBuf};

use myhomelib::db::Db;
use myhomelib::systemdb::SystemDb;

fn main() {
    let system_path = SystemDb::default_path();
    let system = match SystemDb::open(&system_path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!(
                "warning: cannot open system DB at {} — collection registry disabled ({e})",
                system_path.display()
            );
            None
        }
    };

    let mut first_launch = false;
    let db_path: PathBuf = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            // Original startup: open the first existing registered collection.
            match system.as_ref().and_then(|s| s.first_existing().ok().flatten()) {
                Some(c) => PathBuf::from(&c.db_file_name),
                None => {
                    first_launch = true;
                    PathBuf::from("collection.db")
                }
            }
        }
    };

    if let Err(e) = Db::open(&db_path) {
        eprintln!("failed to open {}: {e}", db_path.display());
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

    let sys_for_app = system;
    let open_wizard = first_launch;
    // The DB is already verified open above, so this won't fail in practice.
    if let Err(e) = eframe::run_native(
        "MyHomeLib",
        options,
        Box::new(move |_cc| {
            let db_str = db_path.to_string_lossy().to_string();
            let mut app = myhomelib::gui::app::MyHomeLibApp::new(&db_str)?;
            if let Some(sys) = sys_for_app {
                app.attach_system(sys);
                app.refresh_collections();
            }
            if open_wizard {
                // First launch: no registered collection exists — offer the wizard right away.
                app.open_wizard();
            }
            Ok(Box::new(app))
        }),
    ) {
        eprintln!("GUI error: {e}");
        std::process::exit(1);
    }
}

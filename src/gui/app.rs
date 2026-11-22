//! Native eframe window runner for the GUI. Behind the `gui` feature (pulls winit/GL) so it only
//! builds on machines with a display toolchain — e.g. the Windows target. The view/state logic it
//! drives is pure egui and fully tested headlessly regardless of this module.

use crate::db::Db;
use crate::gui::{state, view};

/// The eframe application: owns a [`State`], a per-context icon cache, and forwards each frame to
/// [`view::render`].
pub struct MyHomeLibApp {
    state: state::State,
    icons: view::IconCache,
}

impl MyHomeLibApp {
    pub fn new(db_path: &str) -> Result<Self, crate::error::AppError> {
        let db = Db::open(db_path)?;
        Ok(Self {
            state: state::State::new(db, db_path),
            icons: view::IconCache::default(),
        })
    }

    /// Attach the collection registry (called by the GUI entry point after opening the system DB).
    pub fn attach_system(&mut self, system: crate::systemdb::SystemDb) {
        self.state.attach_system(system);
    }

    /// Re-read the registry rows (so the manager dialog is fresh at startup).
    pub fn refresh_collections(&mut self) {
        self.state.refresh_collections();
    }

    /// Open the new-collection wizard (first launch with an empty registry).
    pub fn open_wizard(&mut self) {
        self.state.wizard = state::WizardData::default();
        self.state.show_wizard = true;
    }
}

impl eframe::App for MyHomeLibApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        view::render(ctx, &mut self.state, &mut self.icons);
        // eframe repaints on input; no extra timer is needed. (Window-size auto-save is deferred —
        // the persisted size in `<db>.ini` is still read on launch.)
    }
}

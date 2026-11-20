//! GUI layer (Phase 4). [`state`] is pure Rust (no egui) so the navigation logic is fully testable
//! headlessly; [`view`] renders it with egui; [`app`] + the `myhomelib-gui` binary wrap it in a native
//! eframe window (behind the `gui` feature, which pulls winit/GL and only builds on machines with a
//! display toolchain — e.g. the Windows target).

pub mod state;
pub mod view;

#[cfg(feature = "gui")]
pub mod app;

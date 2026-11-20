//! MyHomeLib — home e-book library manager (Rust rewrite of the FreePascal/Lazarus original).
//!
//! Phase 1 exposes the core domain types and the SQLite data layer. The GUI, FB2/FBD engine,
//! import/export, and Librusec client land in later phases (see `PORTING_NOTES.md`).

pub mod core;
pub mod db;
pub mod error;

pub use db::{BookQuery, CollectionStats, Db};

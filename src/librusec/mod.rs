//! Librusec / online-collection client + download queue (Phase 6).
//!
//! Faithful port of the original's `DwnldImpl` subsystem. A per-collection *connection script*
//! (`CHECK`/`REDIR`/`PAUSE`/`GET`/`POST`/`ADD`) drives an HTTP session that fetches a book file,
//! with macro substitution (`%USER%`/`%PASS%`/`%URL%`/`%RESURL%` + book fields), progress, cancel,
//! and skip-if-exists. `catalog` parses the delimited INPX catalog for remote browse/search;
//! `queue` is the download-queue state machine; `command` is the script parser; `client` executes
//! the script over a pluggable HTTP transport (reqwest in production, a mock in tests).

pub mod catalog;
pub mod client;
pub mod command;
pub mod queue;
pub mod service;

//! Core domain types mirroring the original SQLite schema, plus the faithful ports of the
//! original's search-column UDFs. UI-agnostic and unit-testable without a display.

pub mod author;
pub mod book;
pub mod genre;
pub mod search;
pub mod series;

pub use author::Author;
pub use book::Book;
pub use genre::Genre;
pub use series::Series;

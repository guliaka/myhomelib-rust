//! SQLite data layer for a MyHomeLib collection.

pub mod connection;
pub mod props;
pub mod repository;

pub use connection::Db;
pub use repository::{BookQuery, BookScope, CollectionStats};

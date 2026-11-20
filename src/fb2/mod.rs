//! FictionBook 2.1 (FB2) parsing and serialization.
//!
//! The model is a generic element tree ([`model::Fb2`]) that preserves every element, attribute and
//! text run, so parse → serialize → parse round-trips are lossless. Typed accessors on
//! [`model::Fb2`] expose the fields MyHomeLib actually imports (title, authors, genre, annotation,
//! cover). FBD (ZIP) container support lives in [`fbd`].

pub mod fbd;
pub mod model;
pub mod parser;
pub mod writer;

pub use model::{AuthorInfo, Element, Fb2, Node};
pub use parser::parse_fb2;
pub use writer::serialize;

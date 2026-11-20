//! Mirrors the `Genres` table — a hierarchical tree (via `parent_code`) of FB2 genre codes.

#[derive(Debug, Clone, PartialEq)]
pub struct Genre {
    /// Primary key; the stable genre identifier used in `Genre_List`.
    pub genre_code: String,
    /// Parent genre code (`None` = top level).
    pub parent_code: Option<String>,
    /// The corresponding FB2 `<genre>` code, if any.
    pub fb2_code: Option<String>,
    /// Human-readable display name (localized alias).
    pub genre_alias: String,
}

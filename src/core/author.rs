//! Mirrors the `Authors` table. See `schema/create_collection_db.sql`.

use super::search;

#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub author_id: i64,
    pub last_name: String,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
}

impl Author {
    /// Port of `MHL_FULLNAME(LastName, FirstName, MiddleName, 1)` = UPPERCASE(format_name(...)).
    /// This is the value stored in `Authors.SearchName`.
    pub fn search_name(&self) -> String {
        search::upper(&search::format_name(
            &self.last_name,
            self.first_name.as_deref(),
            self.middle_name.as_deref(),
        ))
    }
}

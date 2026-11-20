//! Mirrors the `Series` table.

use super::search;

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub series_id: i64,
    pub series_title: String,
}

impl Series {
    /// Port of `MHL_UPPER(SeriesTitle)` — the value stored in `Series.SearchSeriesTitle`.
    pub fn search_title(&self) -> String {
        search::upper(&self.series_title)
    }
}

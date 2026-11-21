//! GUI application state. Pure Rust (no egui dependency) so the navigation/search/stats logic is fully
//! testable headlessly; [`crate::gui::view`] renders from this and mutates it on user interaction.

use std::path::PathBuf;

use crate::core::{Author, Book};
use crate::db::{BookQuery, Db};
use crate::settings::Settings;

/// Which tree node is selected in the left panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    All,
    Author(i64),
    /// A genre (keyed by its FB2 code).
    Genre(String),
    Series(i64),
    /// A folder; `None` = the collection root.
    Folder(Option<String>),
}

impl Selection {
    pub fn label(&self) -> String {
        match self {
            Selection::All => "All books".to_string(),
            Selection::Author(id) => format!("Author #{id}"),
            Selection::Genre(code) => format!("Genre: {code}"),
            Selection::Series(id) => format!("Series #{id}"),
            Selection::Folder(None) => "(root folder)".to_string(),
            Selection::Folder(Some(f)) => f.clone(),
        }
    }
}

/// One selectable row in a left-panel category tree.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub selection: Selection,
    pub label: String,
    pub count: i64,
}

/// Left-panel category tabs (index = [`State::tab`]).
pub const TABS: [&str; 4] = ["Authors", "Genres", "Series", "Folders"];

/// Multi-field search filters. When any field is set, the list shows matches instead of the tree node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchFilters {
    pub title: String,
    pub author: String,
    pub lang: String,
    pub ext: String,
    pub only_local: bool,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.author.trim().is_empty()
            && self.lang.trim().is_empty()
            && self.ext.trim().is_empty()
            && !self.only_local
    }
}

/// Totals + breakdowns for the statistics view.
#[derive(Debug, Clone, Default)]
pub struct StatsData {
    pub books: i64,
    pub authors: i64,
    pub series: i64,
    pub genres: i64,
    pub by_lang: Vec<(String, i64)>,
    pub by_ext: Vec<(String, i64)>,
}

/// GUI application state.
pub struct State {
    pub db: Db,
    /// Path to the collection DB; settings are stored at `<db_path>.ini`.
    pub db_path: PathBuf,
    pub selection: Selection,
    /// Books currently loaded for `selection` (or the search filters if active).
    pub books: Vec<Book>,
    /// Row in `books` shown in the detail panel (`None` = none selected).
    pub selected_book: Option<usize>,
    /// Active search filters; when non-empty they override the tree selection.
    pub filters: SearchFilters,
    /// Active left-panel tab index (into [`TABS`]).
    pub tab: usize,
    /// Status-bar message.
    pub status: String,
    /// Persisted user settings (`<db>.ini`).
    pub settings: Settings,
    // Dialog visibility + editor buffers for the settings dialog.
    pub show_stats: bool,
    pub show_settings: bool,
    pub reader_ext: String,
    pub reader_cmd: String,
}

impl State {
    pub fn new(db: Db, db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        let settings = Settings::load(&db_path);
        let mut s = Self {
            db,
            db_path,
            selection: Selection::All,
            books: Vec::new(),
            selected_book: None,
            filters: SearchFilters::default(),
            tab: settings.active_tab().min(TABS.len() - 1),
            status: "Ready".to_string(),
            settings,
            show_stats: false,
            show_settings: false,
            reader_ext: String::new(),
            reader_cmd: String::new(),
        };
        s.reload();
        s
    }

    /// Re-load the book list for the active search filters (if any) or the current tree selection.
    pub fn reload(&mut self) {
        let res = if !self.filters.is_empty() {
            let q = BookQuery {
                title_like: nonempty(&self.filters.title),
                author_name_like: nonempty(&self.filters.author),
                lang: nonempty(&self.filters.lang),
                ext: nonempty(&self.filters.ext),
                only_local: self.filters.only_local,
                include_deleted: false,
            };
            self.db.search_books(&q, -1, 0)
        } else {
            match &self.selection {
                Selection::All => self.db.search_books(&BookQuery::default(), -1, 0),
                Selection::Author(id) => self.db.books_by_author(*id),
                Selection::Genre(code) => self.db.books_by_genre(code),
                Selection::Series(id) => self.db.books_by_series(*id),
                Selection::Folder(f) => self.db.books_by_folder(f.as_deref()),
            }
        };
        match res {
            Ok(b) => {
                self.books = b;
                self.selected_book = None;
            }
            Err(e) => self.status = format!("error: {e}"),
        }
    }

    /// Select a tree node and load its books (clears any active search).
    pub fn select(&mut self, sel: Selection) {
        self.selection = sel;
        self.filters = SearchFilters::default();
        self.reload();
    }

    /// Clear the search filters, returning to the tree selection.
    pub fn clear_search(&mut self) {
        self.filters = SearchFilters::default();
        self.reload();
    }

    /// Switch left-panel tab and persist it.
    pub fn set_tab(&mut self, tab: usize) {
        if (0..TABS.len()).contains(&tab) && tab != self.tab {
            self.tab = tab;
            self.settings.set_active_tab(tab);
            let _ = self.settings.save(&self.db_path);
        }
    }

    /// Build the rows for the active left-panel tab.
    pub fn tree_nodes(&self) -> Vec<TreeNode> {
        let mut out = Vec::new();
        match self.tab {
            0 => {
                if let Ok(v) = self.db.list_author_nodes() {
                    for (id, name, count) in v {
                        out.push(TreeNode { selection: Selection::Author(id), label: name, count });
                    }
                }
            }
            1 => {
                if let Ok(v) = self.db.list_genre_nodes() {
                    for (code, alias, count) in v {
                        out.push(TreeNode {
                            selection: Selection::Genre(code.clone()),
                            label: alias,
                            count,
                        });
                    }
                }
            }
            2 => {
                if let Ok(v) = self.db.list_series_nodes() {
                    for (id, title, count) in v {
                        out.push(TreeNode { selection: Selection::Series(id), label: title, count });
                    }
                }
            }
            _ => {
                if let Ok(v) = self.db.list_folder_nodes() {
                    for (folder, count) in v {
                        let label = folder.clone().unwrap_or_else(|| "(root)".to_string());
                        out.push(TreeNode { selection: Selection::Folder(folder), label, count });
                    }
                }
            }
        }
        out
    }

    /// Select a book row for the detail panel.
    pub fn select_book(&mut self, index: usize) {
        if index < self.books.len() {
            self.selected_book = Some(index);
        }
    }

    /// Soft-delete the currently selected book and refresh.
    pub fn delete_selected_book(&mut self) {
        if let Some(i) = self.selected_book {
            let id = self.books[i].book_id;
            let title = self.books[i].title.clone();
            match self.db.soft_delete_book(id) {
                Ok(()) => {
                    self.status = format!("Deleted “{title}”");
                    self.selected_book = None;
                    self.reload();
                }
                Err(e) => self.status = format!("delete failed: {e}"),
            }
        }
    }

    /// Total (non-deleted) book count, for the top bar.
    pub fn total_books(&self) -> i64 {
        self.db.count_books(&BookQuery::default()).unwrap_or(0)
    }

    /// Totals + breakdowns for the statistics view (in-use counts via the link tables, so they reflect
    /// what's actually in the collection rather than raw catalog row counts).
    pub fn stats_data(&self) -> StatsData {
        StatsData {
            books: self.db.count_books(&BookQuery::default()).unwrap_or(0),
            authors: self.db.count_authors_in_use().unwrap_or(0),
            series: self.db.count_series_in_use().unwrap_or(0),
            genres: self.db.count_genres_in_use().unwrap_or(0),
            by_lang: self.db.counts_by_lang().unwrap_or_default(),
            by_ext: self.db.counts_by_ext().unwrap_or_default(),
        }
    }

    /// Apply the reader association currently in the settings dialog inputs and persist it.
    pub fn apply_reader(&mut self) {
        let ext = self.reader_ext.trim().to_ascii_lowercase();
        if !ext.is_empty() {
            self.settings.set_reader(&ext, &self.reader_cmd);
            let _ = self.settings.save(&self.db_path);
            self.status = format!("Reader for .{ext} updated");
        }
    }

    /// Persist the current window size (called by the app when it learns the real size).
    pub fn save_window_size(&mut self, size: [f32; 2]) {
        self.settings.set_window_size(size);
        let _ = self.settings.save(&self.db_path);
    }
}

/// `Some(trimmed)` if `s` is non-blank, else `None`.
fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Natural-case "Last First Middle" display for an author (skips empty parts).
pub fn author_display(a: &Author) -> String {
    let mut s = a.last_name.clone();
    if let Some(f) = &a.first_name {
        if !f.is_empty() {
            s.push(' ');
            s.push_str(f);
        }
    }
    if let Some(m) = &a.middle_name {
        if !m.is_empty() {
            s.push(' ');
            s.push_str(m);
        }
    }
    s
}

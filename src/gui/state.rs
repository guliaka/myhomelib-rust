//! GUI application state. Pure Rust (no egui dependency) so the navigation logic is fully testable
//! headlessly; [`crate::gui::view`] renders from this and mutates it on user interaction.

use crate::core::{Author, Book};
use crate::db::{BookQuery, Db};

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

/// GUI application state.
pub struct State {
    pub db: Db,
    pub selection: Selection,
    /// Books currently loaded for `selection` (or the search box if it has text).
    pub books: Vec<Book>,
    /// Row in `books` shown in the detail panel (`None` = none selected).
    pub selected_book: Option<usize>,
    /// Search text; when non-empty it overrides the tree selection.
    pub search: String,
    /// Active left-panel tab index (into [`TABS`]).
    pub tab: usize,
    /// Status-bar message.
    pub status: String,
}

impl State {
    pub fn new(db: Db) -> Self {
        let mut s = Self {
            db,
            selection: Selection::All,
            books: Vec::new(),
            selected_book: None,
            search: String::new(),
            tab: 0,
            status: "Ready".to_string(),
        };
        s.reload();
        s
    }

    /// Re-load the book list for the current selection (or the search box if it has text).
    pub fn reload(&mut self) {
        let res = if !self.search.trim().is_empty() {
            let q = BookQuery {
                title_like: Some(self.search.trim().to_string()),
                ..Default::default()
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

    /// Select a tree node and load its books.
    pub fn select(&mut self, sel: Selection) {
        self.selection = sel;
        self.search.clear();
        self.reload();
    }

    /// Set the search text (empty clears back to the tree selection).
    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        self.reload();
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

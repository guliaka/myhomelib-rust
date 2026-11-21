//! GUI application state. Pure Rust (no egui dependency) so the navigation/search/stats logic is fully
//! testable headlessly; [`crate::gui::view`] renders from this and mutates it on user interaction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::{Author, Book};
use crate::db::{BookQuery, BookScope, Db};
use crate::import::{Importer, ImportReport};
use crate::inpx::{import_inpx, InpxOptions};
use crate::jobs::Job;
use crate::librusec::catalog::{Catalog, CatalogBook};
use crate::librusec::queue::DownloadQueue;
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

/// What the book list currently pages through: a search (flat [`BookQuery`]) or a tree-node scope.
#[derive(Debug, Clone, PartialEq)]
enum ListSource {
    Search(BookQuery),
    Scope(BookScope),
}

/// Map a GUI [`Selection`] to its DAO [`BookScope`].
impl From<Selection> for BookScope {
    fn from(s: Selection) -> Self {
        match s {
            Selection::All => BookScope::All,
            Selection::Author(id) => BookScope::Author(id),
            Selection::Genre(code) => BookScope::Genre(code),
            Selection::Series(id) => BookScope::Series(id),
            Selection::Folder(f) => BookScope::Folder(f),
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

/// Main page-control tabs, in the original's order. Index = [`State::tab`].
///   0 Авторы / 1 Серии / 2 Жанры — category views (left tree + book grid)
///   3 Поиск — multi-field search form + results grid
///   4 Группы — collection grouping (the folder tree + book grid for now)
///   5 Список загрузок — the download queue table
pub const TABS: [&str; 6] = ["Авторы", "Серии", "Жанры", "Поиск", "Группы", "Список загрузок"];

/// The tabs that show a category tree in the left nav plus a book grid.
const CATEGORY_TABS: [usize; 4] = [0, 1, 2, 4];

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
    /// What the book list currently pages through: the active search (non-empty filters) or the selected
    /// tree node. Drives `list_total` and the lazy window fetches.
    list_source: ListSource,
    /// Total rows in the current view (search matches or node books). Drives virtualization — the list
    /// never loads more than the visible slice, so this stays correct at any library size.
    pub list_total: i64,
    /// The book shown in the detail panel, by stable id (independent of scroll position / pagination).
    pub selected_book_id: Option<i64>,
    // Genre labels for the currently-visible window, keyed by the same window identity as `win_books`,
    // so the grid's Жанр column costs one batch query per scroll position — never one per row.
    genre_win_key: Option<ListSource>,
    genre_win_start: i64,
    genre_win_count: i64,
    win_genres: HashMap<i64, String>,
    /// Active search filters; when non-empty they override the tree selection.
    pub filters: SearchFilters,
    // Virtualized book-list window cache — only the visible slice is ever materialized in memory.
    win_key: Option<ListSource>,
    win_start: i64,
    win_count: i64,
    win_books: Vec<Book>,
    // Per-tab cached left-panel trees. Each is a full-scan GROUP BY over the whole collection, so it's
    // built once per tab (and again after any data mutation) — never per-frame, which would freeze the UI
    // at large scale. Re-visiting a tab is instant after its first build.
    tree_cache: [Vec<TreeNode>; 6],
    tree_built: [bool; 6],
    /// Active main page-control tab index (into [`TABS`]).
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
    // Online (Librusec) search + download queue.
    pub show_online: bool,
    pub online_query: String,
    /// Last fetched catalog (so results can be re-filtered and books resolved for download).
    pub online_catalog: Option<Catalog>,
    /// Indices into `online_catalog.books` matching `online_query`.
    pub online_results: Vec<usize>,
    /// A background search thread writes a finished result here; the UI drains it via `poll_online`.
    pub online_slot: Arc<Mutex<Option<crate::error::Result<Catalog>>>>,
    /// Download queue for queued/active/finished online downloads.
    pub downloads: DownloadQueue,
    /// Editor buffers for the Librusec config in the Online panel.
    pub lib_user: String,
    pub lib_pass: String,
    pub lib_url: String,
    pub lib_script: String,
    /// The running (or just-finished, not yet acknowledged) background job — an import. While it is
    /// set the view shows only the progress page, so the UI connection issues no queries that could
    /// wait on the job's write lock.
    pub job: Option<Job>,
}

impl State {
    pub fn new(db: Db, db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        let settings = Settings::load(&db_path);
        // Read the persisted Librusec config before `settings` is moved into the struct below.
        let active_tab = settings.active_tab().min(TABS.len() - 1);
        let lib_user = settings.librusec_user();
        let lib_pass = settings.librusec_password();
        let lib_url = settings.librusec_url();
        let lib_script = settings.librusec_script();
        let mut s = Self {
            db,
            db_path,
            selection: Selection::All,
            list_source: ListSource::Scope(BookScope::All),
            list_total: 0,
            selected_book_id: None,
            filters: SearchFilters::default(),
            win_key: None,
            win_start: 0,
            win_count: 0,
            win_books: Vec::new(),
            genre_win_key: None,
            genre_win_start: 0,
            genre_win_count: 0,
            win_genres: HashMap::new(),
            tree_cache: std::array::from_fn(|_| Vec::new()),
            tree_built: [false; 6],
            tab: active_tab,
            status: "Ready".to_string(),
            settings,
            show_stats: false,
            show_settings: false,
            reader_ext: String::new(),
            reader_cmd: String::new(),
            show_online: false,
            online_query: String::new(),
            online_catalog: None,
            online_results: Vec::new(),
            online_slot: Arc::new(Mutex::new(None)),
            downloads: DownloadQueue::new(),
            lib_user,
            lib_pass,
            lib_url,
            lib_script,
            job: None,
        };
        s.reload();
        s
    }

    /// Recompute what the book list shows (search if filters are set, else the selected node) and its
    /// total row count. Does NOT load all rows — the view pages through them lazily via
    /// [`Self::books_range`], so startup cost is a single `COUNT(*)` regardless of library size.
    pub fn reload(&mut self) {
        let source = if !self.filters.is_empty() {
            ListSource::Search(BookQuery {
                title_like: nonempty(&self.filters.title),
                author_name_like: nonempty(&self.filters.author),
                lang: nonempty(&self.filters.lang),
                ext: nonempty(&self.filters.ext),
                only_local: self.filters.only_local,
                include_deleted: false,
            })
        } else {
            ListSource::Scope(self.selection.clone().into())
        };
        let total = match &source {
            ListSource::Search(q) => self.db.count_books(q),
            ListSource::Scope(s) => self.db.count_scope_books(s),
        };
        self.list_source = source;
        self.list_total = total.unwrap_or(0);
        // Invalidate the cached window so the next render refetches for the new view.
        self.win_key = None;
        self.win_books.clear();
    }

    /// Fetch rows [`start..start+count`] of the current view (cached while the view + window are
    /// unchanged). This is the only place book rows are materialized — the virtualized list calls it for
    /// the visible slice only, so memory stays O(visible rows) at any library size.
    pub fn books_range(&mut self, start: i64, count: i64) -> Vec<Book> {
        if count <= 0 || self.list_total <= 0 {
            return Vec::new();
        }
        let s = start.max(0).min(self.list_total);
        let c = (count as i64).min(self.list_total - s).max(0);
        if c <= 0 {
            return Vec::new();
        }
        // Cache hit: same view + same window.
        if self.win_key == Some(self.list_source.clone()) && self.win_start == s && self.win_count == c {
            return self.win_books.clone();
        }
        let books = match &self.list_source {
            ListSource::Search(q) => self.db.search_books(q, c, s).unwrap_or_default(),
            ListSource::Scope(sc) => self.db.list_scope_books(sc, c, s).unwrap_or_default(),
        };
        self.win_key = Some(self.list_source.clone());
        self.win_start = s;
        self.win_count = c;
        self.win_books = books.clone();
        books
    }

    /// Genre labels for the currently-visible window (see [`Self::win_books`]), cached per window so the
    /// grid's Жанр column is one batch query per scroll position. Call after [`Self::books_range`].
    pub fn genre_map(&mut self) -> &HashMap<i64, String> {
        let fresh = self.genre_win_key == Some(self.list_source.clone())
            && self.genre_win_start == self.win_start
            && self.genre_win_count == self.win_count;
        if !fresh {
            let ids: Vec<i64> = self.win_books.iter().map(|b| b.book_id).collect();
            let genres = self.db.book_genres_batch(&ids).unwrap_or_default();
            self.win_genres = genres;
            self.genre_win_key = Some(self.list_source.clone());
            self.genre_win_start = self.win_start;
            self.genre_win_count = self.win_count;
        }
        &self.win_genres
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

    /// Switch the main page-control tab and persist it. Each tab starts from a clean slate (all books,
    /// no active search) so its left tree + grid reflect that tab's own context.
    pub fn set_tab(&mut self, tab: usize) {
        if (0..TABS.len()).contains(&tab) && tab != self.tab {
            self.tab = tab;
            self.selection = Selection::All;
            self.filters = SearchFilters::default();
            self.reload();
            self.settings.set_active_tab(tab);
            let _ = self.settings.save(&self.db_path);
        }
    }

    /// True for the tabs that show a category tree in the left nav plus a book grid.
    pub fn is_category_tab(&self) -> bool {
        CATEGORY_TABS.contains(&self.tab)
    }

    /// True for the multi-field search tab (search form + results grid, no left tree).
    pub fn is_search_tab(&self) -> bool {
        self.tab == 3
    }

    /// True for the download-queue tab.
    pub fn is_downloads_tab(&self) -> bool {
        self.tab == 5
    }

    /// Rows for the active left-panel tab. Cached per tab — the underlying query is a full-scan GROUP BY
    /// over the whole collection, so each tab builds once (and again after any data mutation), never
    /// per-frame. Re-visiting a tab returns its cache instantly.
    pub fn tree_nodes(&mut self) -> Vec<TreeNode> {
        let t = self.tab.min(TABS.len() - 1);
        if self.tree_built[t] {
            return self.tree_cache[t].clone();
        }
        let mut out = Vec::new();
        match t {
            0 => { // Авторы
                if let Ok(v) = self.db.list_author_nodes() {
                    for (id, name, count) in v {
                        out.push(TreeNode { selection: Selection::Author(id), label: name, count });
                    }
                }
            }
            1 => { // Серии
                if let Ok(v) = self.db.list_series_nodes() {
                    for (id, title, count) in v {
                        out.push(TreeNode { selection: Selection::Series(id), label: title, count });
                    }
                }
            }
            2 => { // Жанры
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
            4 => { // Группы (folder-based grouping for now)
                if let Ok(v) = self.db.list_folder_nodes() {
                    for (folder, count) in v {
                        let label = folder.clone().unwrap_or_else(|| "(root)".to_string());
                        out.push(TreeNode { selection: Selection::Folder(folder), label, count });
                    }
                }
            }
            _ => {} // Поиск (3) and Список загрузок (5) have no left tree
        }
        self.tree_built[t] = true;
        self.tree_cache[t] = out.clone();
        out
    }

    /// Select a book (by stable id) for the detail panel.
    pub fn select_book_id(&mut self, id: i64) {
        self.selected_book_id = Some(id);
    }

    /// Soft-delete the currently selected book (by id) and refresh the view's total.
    pub fn delete_selected_book(&mut self) {
        if let Some(id) = self.selected_book_id {
            match self.db.soft_delete_book(id) {
                Ok(()) => {
                    self.status = format!("Deleted book #{id}");
                    self.selected_book_id = None;
                    self.reload();
                    self.tree_built = [false; 6]; // per-node book counts may have changed
                }
                Err(e) => self.status = format!("delete failed: {e}"),
            }
        }
    }

    /// Import every FB2/FBD file under `dir` into the collection. The native folder picker lives in the
    /// view; this does the actual work so it's testable headlessly. New books may add authors/genres/
    /// series/folders, so the per-tab trees are rebuilt and the list total reloaded. Runs synchronously
    /// (the SQLite connection isn't `Send`); large folders briefly block the UI while they import.
    pub fn import_dir(&mut self, dir: &Path) -> crate::error::Result<ImportReport> {
        let report = Importer::new(&mut self.db, dir).import_dir(dir)?;
        self.reload();
        self.tree_built = [false; 6]; // new books may add nodes / change per-node counts
        self.status = format!(
            "Imported {} book(s) from {} — {} already present, {} error(s)",
            report.imported,
            dir.display(),
            report.existing,
            report.errors.len()
        );
        Ok(report)
    }

    // ------------------------------------------------------------------ background jobs

    /// True while a background job is set (running, or finished and waiting to be acknowledged).
    pub fn job_active(&self) -> bool {
        self.job.is_some()
    }

    /// Build/extend this collection from an INPX index on a background thread (its own connection).
    /// Returns false — and reports why in the status bar — if another job is still active.
    pub fn start_inpx_import(&mut self, inpx: &Path) -> bool {
        if self.job_active() {
            self.status = "Дождитесь завершения текущей операции".into();
            return false;
        }
        let (db_path, inpx) = (self.db_path.clone(), inpx.to_path_buf());
        let title = format!("Импорт INPX: {}", inpx.display());
        self.job = Some(Job::spawn(title, move |p| {
            let db = Db::open(&db_path)?;
            Ok(import_inpx(&db, &inpx, &InpxOptions::default(), p)?.summary())
        }));
        true
    }

    /// Import every FB2/FBD file under `dir` on a background thread, so a large folder no longer
    /// freezes the window. Same engine as [`Self::import_dir`].
    pub fn start_folder_import(&mut self, dir: &Path) -> bool {
        if self.job_active() {
            self.status = "Дождитесь завершения текущей операции".into();
            return false;
        }
        let (db_path, dir) = (self.db_path.clone(), dir.to_path_buf());
        let title = format!("Импорт папки: {}", dir.display());
        self.job = Some(Job::spawn(title, move |p| {
            p.set_comment("Импорт файлов…");
            let mut db = Db::open(&db_path)?;
            let report = Importer::new(&mut db, &dir).import_dir(&dir)?;
            for (path, e) in &report.errors {
                p.error(format!("{}: {e}", path.display()));
            }
            p.set_percent(100.0);
            Ok(format!("Добавлено {} книг, уже в коллекции: {}", report.imported, report.existing))
        }));
        true
    }

    /// Acknowledge a finished job: drop it, put its outcome in the status bar and refresh every view
    /// (new books may add tree nodes and change counts). No-op while the job is still running.
    pub fn finish_job(&mut self) {
        if !self.job.as_ref().is_some_and(Job::is_done) {
            return;
        }
        if let Some(mut job) = self.job.take() {
            job.join();
            self.status = match job.progress.result() {
                Some(Ok(summary)) => summary,
                Some(Err(e)) => format!("Ошибка: {e}"),
                None => String::new(),
            };
        }
        self.tree_built = [false; 6];
        self.genre_win_key = None;
        self.reload();
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

    // -- Online (Librusec) -------------------------------------------------

    /// Drain a finished background search from `online_slot` into the results list.
    pub fn poll_online(&mut self) {
        if let Some(res) = self.online_slot.lock().ok().and_then(|mut g| g.take()) {
            match res {
                Ok(cat) => {
                    self.online_results = cat.search(&self.online_query);
                    self.online_catalog = Some(cat);
                    self.status = format!(
                        "{} online book(s) match “{}”",
                        self.online_results.len(),
                        self.online_query
                    );
                }
                Err(e) => {
                    self.online_catalog = None;
                    self.online_results.clear();
                    self.status = format!("online search failed: {e}");
                }
            }
        }
    }

    /// Queue a catalog book (by index into `online_catalog`) for download; returns its queue id.
    pub fn queue_download(&mut self, idx: usize) -> Option<u64> {
        let cat = self.online_catalog.as_ref()?;
        if idx >= cat.books.len() {
            return None;
        }
        let book = &cat.books[idx];
        let label = online_label(book);
        let url = cat.resolve_url(book)?;
        Some(self.downloads.enqueue(label, url))
    }

    /// Persist the Librusec config from the Online-panel inputs.
    pub fn save_lib_config(&mut self) {
        self.settings.set_librusec_user(&self.lib_user);
        self.settings.set_librusec_password(&self.lib_pass);
        self.settings.set_librusec_url(&self.lib_url);
        self.settings.set_librusec_script(&self.lib_script);
        let _ = self.settings.save(&self.db_path);
        self.status = "Librusec settings saved".into();
    }

    /// Kick off a background catalog fetch (the network edge; not exercised by offline tests).
    pub fn start_online_search(&mut self) {
        let url = self.settings.librusec_url().trim().to_string();
        if url.is_empty() {
            self.status = "Set a Librusec URL in the Online panel first".into();
            return;
        }
        let slot = self.online_slot.clone();
        std::thread::spawn(move || {
            let res = crate::librusec::service::fetch_catalog(&url);
            if let Ok(mut g) = slot.lock() {
                *g = Some(res);
            }
        });
        self.status = "Searching online…".into();
    }
}

/// `"Last First — Title"` display for a catalog book (title-only when there's no author).
fn online_label(book: &CatalogBook) -> String {
    let author = book
        .authors
        .first()
        .map(|(l, f, _)| if f.is_empty() { l.clone() } else { format!("{l} {f}") })
        .unwrap_or_default();
    if author.trim().is_empty() {
        book.title.clone()
    } else {
        format!("{} — {}", author.trim(), book.title)
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

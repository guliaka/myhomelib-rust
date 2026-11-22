//! GUI application state. Pure Rust (no egui dependency) so the navigation/search/stats logic is fully
//! testable headlessly; [`crate::gui::view`] renders from this and mutates it on user interaction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::{Author, Book};
use crate::db::{BookQuery, BookScope, Db};
use crate::import::{Importer, ImportReport};
use crate::librusec::catalog::{Catalog, CatalogBook};
use crate::librusec::queue::DownloadQueue;
use crate::settings::Settings;
use crate::systemdb::{CollectionInfo, SystemDb};

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
    /// A reading group (system-DB `GroupID`).
    Group(i64),
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
            Selection::Group(id) => format!("Group #{id}"),
        }
    }
}

/// What the book list currently pages through: a search (flat [`BookQuery`]), a tree-node scope,
/// or a reading group's member books (ids resolved from the system DB).
#[derive(Debug, Clone, PartialEq)]
enum ListSource {
    Search(BookQuery),
    Scope(BookScope),
    Group(i64),
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
            // Group lists page over system-DB membership ids (ListSource::Group), never this scope.
            Selection::Group(_) => BookScope::All,
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
    // -- Multi-collection management (system DB registry + dialogs) ---------------------------
    /// System-DB handle (`None` in headless tests / when no registry is attached).
    pub system: Option<SystemDb>,
    /// The collection-manager dialog (the original's `frm_bases`).
    pub show_collections: bool,
    /// Registry rows shown in the manager dialog.
    pub collection_rows: Vec<CollectionInfo>,
    /// Last collection-management error, shown in the manager dialog.
    pub collection_error: String,
    /// Pending delete confirmation (collection id), plus whether to also purge files.
    pub confirm_delete: Option<(i64, bool)>,
    /// The new-collection wizard dialog (the original's NCW).
    pub show_wizard: bool,
    /// Wizard editor buffers (name / db file / root folder / notes / type).
    pub wizard: WizardData,
    /// The collection-properties dialog (the original's collection properties page).
    pub show_properties: bool,
    /// Editing copy of the current collection's registry entry (`None` = not registered).
    pub props: Option<CollectionInfo>,
    // -- Group editor (the original's Группа menu + frm_EditGroup) ----------------------------
    /// The create/rename-group dialog: mode + text input buffer.
    pub group_edit: Option<(GroupEditMode, String)>,
    /// Pending group deletion/clear confirmation: `(group_id, name, clear_only)`.
    pub group_delete_confirm: Option<(i64, String, bool)>,
    /// The «add selected book to a group» picker.
    pub show_group_picker: bool,
    /// `(group_id, name, book count)` rows for the Группы tab's left tree and the picker.
    pub groups_cache: Vec<(i64, String, i64)>,
    /// Bumped on any group-membership mutation; invalidates the per-group id cache.
    groups_version: u64,
    /// Cached member book ids for `(group_id, groups_version)`.
    group_ids_cache: Option<(i64, u64, Vec<i64>)>,
}

/// Which group the create/rename dialog is editing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GroupEditMode {
    Create,
    Rename(i64),
}

/// Editor buffers for the new-collection wizard. Mirrors the original wizard's
/// name-and-location + file-types + finish steps; the genre source is always the built-in tree
/// (the original's custom-genre-file and INPX-download steps are follow-ups).
#[derive(Debug, Clone, Default)]
pub struct WizardData {
    pub name: String,
    pub db_file: String,
    pub root_folder: String,
    pub notes: String,
    /// `true` = online (Librusec) collection, `false` = local FB2/FBD.
    pub online: bool,
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
            system: None,
            show_collections: false,
            collection_rows: Vec::new(),
            collection_error: String::new(),
            confirm_delete: None,
            show_wizard: false,
            wizard: WizardData::default(),
            show_properties: false,
            props: None,
            group_edit: None,
            group_delete_confirm: None,
            show_group_picker: false,
            groups_cache: Vec::new(),
            groups_version: 0,
            group_ids_cache: None,
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
        } else if let Selection::Group(g) = self.selection {
            ListSource::Group(g)
        } else {
            ListSource::Scope(self.selection.clone().into())
        };
        let total = match &source {
            ListSource::Search(q) => self.db.count_books(q),
            ListSource::Scope(s) => self.db.count_scope_books(s),
            ListSource::Group(g) => Ok(self.group_book_ids(*g).len() as i64),
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
            // Group membership lives in the system DB: page over the id list, fetch rows by id.
            ListSource::Group(g) => {
                let ids = self.group_book_ids(*g);
                let slice = &ids[s as usize..(s + c) as usize];
                self.db.list_books_by_ids(slice).unwrap_or_default()
            }
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
            4 => { // Группы — real reading groups from the system DB (the original's groups view)
                for (id, name, count) in self.refresh_groups_cache() {
                    out.push(TreeNode {
                        selection: Selection::Group(id),
                        label: name,
                        count,
                    });
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

    // -- Multi-collection management -----------------------------------------

    /// Attach the system-DB registry (the app runner does this on startup).
    pub fn attach_system(&mut self, system: SystemDb) {
        // The original seeds «Избранное» / «К прочтению» on a fresh system DB.
        let _ = system.ensure_default_groups();
        self.system = Some(system);
        self.refresh_groups_cache();
        self.tree_built[4] = false;
    }

    /// Re-read the registry rows for the manager dialog.
    pub fn refresh_collections(&mut self) {
        if let Some(sys) = self.system.as_ref() {
            match sys.list() {
                Ok(rows) => {
                    self.collection_rows = rows;
                    self.collection_error.clear();
                }
                Err(e) => self.collection_error = e.to_string(),
            }
        }
    }

    /// Open the collection-manager dialog (`Коллекция → Выбрать коллекцию…`).
    pub fn open_collections(&mut self) {
        self.refresh_collections();
        self.show_collections = true;
    }

    /// Replace the open collection with another registered one (`db_file_name`), or just open a
    /// path directly. Resets every cached view (trees, window, genres, selection, settings) so the
    /// whole UI reflects the new collection — the original re-creates its main form; we swap in place.
    pub fn switch_collection(&mut self, db_file: &str) {
        match Db::open(db_file) {
            Ok(db) => {
                self.db = db;
                self.db_path = PathBuf::from(db_file);
                self.settings = Settings::load(&self.db_path);
                self.selection = Selection::All;
                self.filters = SearchFilters::default();
                self.selected_book_id = None;
                self.tab = self.settings.active_tab().min(TABS.len() - 1);
                self.tree_built = [false; 6];
                self.win_key = None;
                self.win_books.clear();
                self.genre_win_key = None;
                self.win_genres.clear();
                self.group_ids_cache = None;
                self.refresh_groups_cache();
                self.reload();
                let name = self
                    .collection_rows
                    .iter()
                    .find(|c| c.db_file_name == db_file)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| db_file.to_string());
                self.status = format!("Collection “{name}” opened");
            }
            Err(e) => self.status = format!("open {db_file} failed: {e}"),
        }
    }

    /// Finish the new-collection wizard: create the collection file, register it, and open it.
    pub fn create_collection(&mut self) {
        let w = &self.wizard;
        let name = w.name.trim().to_string();
        let db_file = w.db_file.trim().to_string();
        if name.is_empty() || db_file.is_empty() {
            self.collection_error = "Укажите имя и файл базы коллекции".into();
            return;
        }
        // Create the collection file (schema + built-in genres) — matches the original wizard,
        // which also creates the DB before registering it.
        if let Err(e) = Db::open(&db_file) {
            self.collection_error = format!("не удалось создать базу: {e}");
            return;
        }
        // The original's collection-type codes: local FB2 = CT_EXTERNAL_LOCAL_FB ($0001 0000),
        // online FB2 = CT_EXTERNAL_ONLINE_FB ($0801 0000).
        let code: i64 = if w.online { 0x0801_0000 } else { 0x0001_0000 };
        let info = CollectionInfo {
            name,
            db_file_name: db_file.clone(),
            root_folder: w.root_folder.trim().to_string(),
            notes: w.notes.trim().to_string(),
            code,
            ..Default::default()
        };
        let sys = match self.system.as_ref() {
            Some(s) => s,
            None => {
                self.collection_error = "системная база недоступна".into();
                return;
            }
        };
        if let Err(e) = sys.add(&info) {
            self.collection_error = e.to_string();
            return;
        }
        self.refresh_collections();
        self.show_wizard = false;
        self.switch_collection(&db_file);
    }

    /// Delete a collection from the registry (and optionally its files), after user confirmation.
    /// The `confirm_delete` flag is set by the dialog; this is the actual action.
    pub fn remove_collection(&mut self, id: i64, purge_files: bool) {
        let file = self.collection_rows.iter().find(|c| c.id == id).map(|c| c.db_file_name.clone());
        let current = self.db_path.to_string_lossy().to_string();
        let purging_current = purge_files && file.as_deref() == Some(current.as_str());

        // Windows can't delete an open file, so when purging the open collection, switch to another
        // one FIRST (dropping the connection), and only then delete the row + files.
        if purging_current {
            let next = self
                .collection_rows
                .iter()
                .find(|c| c.id != id && c.file_exists())
                .map(|c| c.db_file_name.clone());
            match next {
                Some(next) => self.switch_collection(&next),
                None => {
                    self.collection_error =
                        "Нельзя удалить файл открытой коллекции — сначала откройте другую".into();
                    return;
                }
            }
        }

        let sys = match self.system.as_ref() {
            Some(s) => s,
            None => return,
        };
        if let Err(e) = sys.delete(id) {
            self.collection_error = e.to_string();
            return;
        }
        if purge_files {
            if let Some(f) = &file {
                if let Err(e) = SystemDb::purge_files(f) {
                    self.collection_error = e.to_string();
                }
            }
        }
        self.confirm_delete = None;
        self.refresh_collections();
        // If the open collection's registry row is gone, fall back to the first remaining
        // registered collection — the original closes its main form and re-opens another base.
        let still_current = self
            .collection_rows
            .iter()
            .any(|c| c.db_file_name == self.db_path.to_string_lossy());
        if !still_current {
            if let Some(next) = self
                .collection_rows
                .iter()
                .find(|c| c.file_exists())
                .map(|c| c.db_file_name.clone())
            {
                self.switch_collection(&next);
            } else {
                self.status = "Коллекция удалена (других коллекций нет)".into();
            }
        } else {
            self.status = "Коллекция удалена из списка".into();
        }
    }

    /// `Коллекция → Сжатие базы` — VACUUM, reporting the file-size delta.
    pub fn compact_collection(&mut self) {
        let before = self.db.file_size_bytes();
        match self.db.compact() {
            Ok(()) => {
                let after = self.db.file_size_bytes();
                let msg = match (before, after) {
                    (Some(b), Some(a)) => format!("База сжата: {b} → {a} байт"),
                    _ => "База сжата".to_string(),
                };
                self.status = msg;
            }
            Err(e) => self.status = format!("сжатие не удалось: {e}"),
        }
    }

    /// `Коллекция → Ремонт базы` — integrity check + rebuild of the derived search columns.
    pub fn repair_collection(&mut self) {
        match self.db.repair() {
            Ok(n) => self.status = format!("База проверена, восстановлено строк: {n}"),
            Err(e) => self.status = format!("ремонт не удался: {e}"),
        }
    }

    /// Open the properties dialog for the current collection (must be registered).
    pub fn open_properties(&mut self) {
        let current = self.db_path.to_string_lossy().to_string();
        self.props = self
            .system
            .as_ref()
            .and_then(|s| s.list().ok())
            .and_then(|rows| rows.into_iter().find(|c| c.db_file_name == current));
        if self.props.is_some() {
            self.show_properties = true;
        } else {
            self.status = "Текущая коллекция не зарегистрирована в системной базе".into();
        }
    }

    /// The current collection's `Bases.DatabaseID`, if it's registered in the system DB
    /// (needed to scope group membership to this collection).
    pub fn current_database_id(&self) -> Option<i64> {
        let current = self.db_path.to_string_lossy().to_string();
        self.system
            .as_ref()
            .and_then(|s| s.list().ok())
            .and_then(|rows| rows.into_iter().find(|c| c.db_file_name == current).map(|c| c.id))
    }

    /// The current collection's registry entry (for the INPX export header), if registered.
    fn current_info(&self) -> Option<CollectionInfo> {
        let current = self.db_path.to_string_lossy().to_string();
        self.system
            .as_ref()
            .and_then(|s| s.list().ok())
            .and_then(|rows| rows.into_iter().find(|c| c.db_file_name == current))
    }

    /// `Книга → Импорт INPX…`: import a lib.rus.ec INPX into the open collection.
    pub fn import_inpx_file(&mut self, path: &str) {
        match crate::inpx::import_inpx(&mut self.db, Path::new(path)) {
            Ok(report) => {
                // Registry fields carried by collection.info/version.info, when registered.
                if let (Some(sys), Some(info)) = (self.system.as_ref(), &self.current_info()) {
                    if let Some(h) = &report.header {
                        let mut updated = info.clone();
                        if !h.notes.is_empty() {
                            updated.notes = h.notes.clone();
                        }
                        if !h.url.is_empty() {
                            updated.url = h.url.clone();
                            updated.connection_script = h.script.clone();
                        }
                        if let Some(v) = report.version {
                            updated.data_version = v;
                        }
                        let _ = sys.update(&updated);
                        self.refresh_collections();
                    }
                }
                self.reload();
                self.tree_built = [false; 6];
                self.status = format!(
                    "INPX: {} импортировано, {} уже было, {} ошибок",
                    report.imported,
                    report.existing,
                    report.errors.len()
                );
            }
            Err(e) => self.status = format!("импорт INPX не удался: {e}"),
        }
    }

    /// `Коллекция → Экспорт в INPX…`: write the collection as a lib.rus.ec-format INPX.
    pub fn export_inpx_file(&mut self, path: &str) {
        let header = match self.current_info() {
            Some(c) => crate::inpx::InpxHeader {
                name: c.name,
                file_name: c.db_file_name,
                content_type: c.code,
                notes: c.notes,
                url: c.url,
                script: c.connection_script,
            },
            None => crate::inpx::InpxHeader {
                name: self
                    .db_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                ..Default::default()
            },
        };
        let version = self
            .current_info()
            .map(|c| if c.data_version > 0 { c.data_version.to_string() } else { "0".into() })
            .unwrap_or_else(|| "0".into());
        match crate::inpx::export_inpx(&self.db, Path::new(path), &header, &version) {
            Ok(n) => self.status = format!("INPX: экспортировано книг — {n}"),
            Err(e) => self.status = format!("экспорт INPX не удался: {e}"),
        }
    }

    /// `Коллекция → Экспорт пользовательских данных…`: rate/progress/review + groups to the
    /// original's zlib-XML user-data file.
    pub fn export_user_data(&mut self, path: &str) {
        let mut data = match crate::userdata::UserData::from_collection(&self.db) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("экспорт не удался: {e}");
                return;
            }
        };
        if let (Some(sys), Some(id)) = (self.system.as_ref(), self.current_database_id()) {
            if let Err(e) = crate::userdata::export_groups_from_system(&mut data, sys, id) {
                self.status = format!("экспорт групп не удался: {e}");
                return;
            }
        }
        match data.save(Path::new(path)) {
            Ok(()) => self.status = format!(
                "Пользовательские данные: {} книг, {} групп → {path}",
                data.extras.len(),
                data.groups.len()
            ),
            Err(e) => self.status = format!("экспорт не удался: {e}"),
        }
    }

    /// `Коллекция → Импорт пользовательских данных…`: apply rates/progress/reviews to the open
    /// collection and groups into the system DB (both keyed by the stable `LibID`).
    pub fn import_user_data(&mut self, path: &str) {
        let data = match crate::userdata::UserData::load(Path::new(path)) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("импорт не удался: {e}");
                return;
            }
        };
        let (applied, missing) = match data.apply_to_collection(&mut self.db) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("импорт не удался: {e}");
                return;
            }
        };
        let mut groups_msg = String::new();
        if let (Some(sys), Some(id)) = (self.system.as_ref(), self.current_database_id()) {
            match crate::userdata::import_groups_to_system(&data.groups, sys, &self.db, id) {
                Ok((linked, unresolved)) => {
                    groups_msg = format!(", групп: {linked} книг ({unresolved} не найдено)");
                }
                Err(e) => groups_msg = format!(", группы не импортированы: {e}"),
            }
        }
        self.reload();
        self.status = format!("Импорт: {applied} книг обновлено, {missing} не найдено{groups_msg}");
    }

    // -- Group editor (the original's Группа menu) ------------------------------------------

    /// `(group_id, name, member count for the current collection)` for the Группы tab + picker.
    /// One system-DB query; cached in `groups_cache` and rebuilt by `refresh_groups_cache`.
    pub fn refresh_groups_cache(&mut self) -> Vec<(i64, String, i64)> {
        if let (Some(sys), Some(dbid)) = (self.system.as_ref(), self.current_database_id()) {
            if let Ok(members) = sys.list_group_books(dbid) {
                if let Ok(groups) = sys.list_groups() {
                    self.groups_cache = groups
                        .into_iter()
                        .map(|(gid, name)| {
                            let count = members.iter().filter(|m| m.0 == gid).count() as i64;
                            (gid, name, count)
                        })
                        .collect();
                }
            }
        } else {
            self.groups_cache.clear();
        }
        self.groups_cache.clone()
    }

    /// Member book ids of a group (collection-local ids), cached per groups_version.
    fn group_book_ids(&mut self, group_id: i64) -> Vec<i64> {
        if let Some((g, v, ids)) = &self.group_ids_cache {
            if *g == group_id && *v == self.groups_version {
                return ids.clone();
            }
        }
        let ids = match (self.system.as_ref(), self.current_database_id()) {
            (Some(sys), Some(dbid)) => sys
                .list_group_books(dbid)
                .map(|rows| rows.into_iter().filter(|r| r.0 == group_id).map(|r| r.2).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.group_ids_cache = Some((group_id, self.groups_version, ids.clone()));
        ids
    }

    /// Bump the membership version and rebuild every group-dependent view.
    fn groups_changed(&mut self) {
        self.groups_version += 1;
        self.group_ids_cache = None;
        self.tree_built[4] = false;
        self.refresh_groups_cache();
        // The open view may be the changed group itself — recompute totals.
        self.reload();
    }

    /// `Группа → Создать…` / `Переименовать…`: open the name prompt.
    pub fn open_group_edit(&mut self, mode: GroupEditMode) {
        let initial = match mode {
            GroupEditMode::Create => String::new(),
            GroupEditMode::Rename(id) => self
                .groups_cache
                .iter()
                .find(|g| g.0 == id)
                .map(|g| g.1.clone())
                .unwrap_or_default(),
        };
        self.group_edit = Some((mode, initial));
    }

    /// Apply the create/rename prompt.
    pub fn apply_group_edit(&mut self) {
        let Some((mode, text)) = self.group_edit.take() else { return };
        let name = text.trim().to_string();
        if name.is_empty() {
            self.status = "Имя группы не может быть пустым".into();
            return;
        }
        let Some(sys) = self.system.as_ref() else {
            self.status = "системная база недоступна".into();
            return;
        };
        let res = match mode {
            GroupEditMode::Create => sys.add_group(&name, true).map(|_| ()),
            GroupEditMode::Rename(id) => sys.rename_group(id, &name),
        };
        match res {
            Ok(()) => {
                self.status = format!("Группа «{name}» сохранена");
                self.groups_changed();
            }
            Err(e) => {
                self.status = e.to_string();
                self.group_edit = Some((mode, text)); // keep the dialog open with the input
            }
        }
    }

    /// Delete a group (all memberships) or just clear it, after confirmation.
    pub fn delete_or_clear_group(&mut self, group_id: i64, clear_only: bool) {
        self.group_delete_confirm = None;
        let Some(sys) = self.system.as_ref() else { return };
        let res = if clear_only {
            sys.clear_group(group_id)
        } else {
            sys.delete_group(group_id)
        };
        match res {
            Ok(()) => {
                // A cleared group stays (now empty); a deleted one doesn't — fall back to All.
                if !clear_only {
                    if let Selection::Group(g) = self.selection {
                        if g == group_id {
                            self.selection = Selection::All;
                        }
                    }
                }
                self.status = if clear_only {
                    "Группа очищена".into()
                } else {
                    "Группа удалена".into()
                };
                self.groups_changed();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Add the selected book to a group (the original's context «Добавить в группу»).
    pub fn add_selected_to_group(&mut self, group_id: i64) {
        self.show_group_picker = false;
        let Some(book_id) = self.selected_book_id else {
            self.status = "Сначала выберите книгу".into();
            return;
        };
        let Some(dbid) = self.current_database_id() else {
            self.status = "Коллекция не зарегистрирована — группы недоступны".into();
            return;
        };
        let Some(sys) = self.system.as_ref() else { return };
        // Need libid + title for the system-DB Books row.
        match self.db.get_book(book_id) {
            Ok(Some(b)) => match sys.add_group_book(group_id, book_id, dbid, &b.lib_id, &b.title) {
                Ok(()) => {
                    let name = self
                        .groups_cache
                        .iter()
                        .find(|g| g.0 == group_id)
                        .map(|g| g.1.clone())
                        .unwrap_or_default();
                    self.status = format!("«{}» добавлена в «{name}»", b.title);
                    self.groups_changed();
                }
                Err(e) => self.status = e.to_string(),
            },
            _ => self.status = "Книга не найдена".into(),
        }
    }

    /// Remove the selected book from the open group (the original's «Удалить из группы»).
    pub fn remove_selected_from_group(&mut self) {
        let Selection::Group(g) = self.selection else {
            self.status = "Откройте группу, чтобы удалить книгу из неё".into();
            return;
        };
        let Some(book_id) = self.selected_book_id else {
            self.status = "Сначала выберите книгу".into();
            return;
        };
        let Some(dbid) = self.current_database_id() else { return };
        let Some(sys) = self.system.as_ref() else { return };
        match sys.remove_group_book(g, book_id, dbid) {
            Ok(()) => {
                self.selected_book_id = None;
                self.status = "Книга удалена из группы".into();
                self.groups_changed();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Save the edited properties back to the registry.
    pub fn save_properties(&mut self) {
        let props = match self.props.take() {
            Some(p) => p,
            None => return,
        };
        let sys = match self.system.as_ref() {
            Some(s) => s,
            None => return,
        };
        match sys.update(&props) {
            Ok(()) => {
                self.show_properties = false;
                self.refresh_collections();
                self.status = format!("Свойства “{}” сохранены", props.name);
            }
            Err(e) => {
                self.props = Some(props);
                self.collection_error = e.to_string();
            }
        }
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

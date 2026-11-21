//! Local collection import from an INPX index — the port of the original's `TImportInpxThread`
//! (`Program/ImportImpl/unit_ImportInpxThread.pas`).
//!
//! An `.inpx` is a ZIP holding one `.inp` per book archive (`fb2-000024-030559.inp` describes
//! `fb2-000024-030559.zip`), plus optional `structure.info` (field order), `collection.info`
//! (name / type / notes / URL / connection script) and `version.info` (data version). Each `.inp`
//! line is one book: fields separated by `Chr(4)`, list items by `:`, author name parts by `,`.
//!
//! This is how a million-book library is built: no book file is opened, the catalog comes entirely
//! from the index. The importer is headless and reports through a [`JobProgress`], so it runs the
//! same from the CLI, from tests and from a GUI background job.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rusqlite::params;
use zip::ZipArchive;

use crate::core::{search, Book};
use crate::db::{props, Db};
use crate::error::{AppError, Result};
use crate::jobs::JobProgress;

const FIELD_DELIM: char = '\u{04}';
const ITEM_DELIM: char = ':';
const SUBITEM_DELIM: char = ',';

const STRUCTURE_INFO: &str = "structure.info";
const COLLECTION_INFO: &str = "collection.info";
const VERSION_INFO: &str = "version.info";
/// The original's `DEFAULTSTRUCTURE`.
const DEFAULT_STRUCTURE: &str =
    "AUTHOR;GENRE;TITLE;SERIES;SERNO;FILE;SIZE;LIBID;DEL;EXT;DATE;LANG;LIBRATE;KEYWORDS";

pub const NO_TITLE: &str = "Без названия";
pub const UNKNOWN_AUTHOR: &str = "Неизвестный";
pub const UNKNOWN_GENRE_CODE: &str = "0.0";
const UNKNOWN_GENRE_ALIAS: &str = "Неотсортированное";

/// How often (in records) progress is published and cancellation is checked.
const CHECKPOINT: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    Author,
    Title,
    Series,
    SerNo,
    Genre,
    LibId,
    InsideNo,
    File,
    Folder,
    Ext,
    Size,
    Lang,
    Date,
    Deleted,
    LibRate,
    KeyWords,
}

fn field_of(code: &str) -> Field {
    match code {
        "AUTHOR" => Field::Author,
        "TITLE" => Field::Title,
        "SERIES" => Field::Series,
        "SERNO" => Field::SerNo,
        "GENRE" => Field::Genre,
        "LIBID" => Field::LibId,
        "INSNO" => Field::InsideNo,
        "FILE" => Field::File,
        "FOLDER" => Field::Folder,
        "EXT" => Field::Ext,
        "SIZE" => Field::Size,
        "LANG" => Field::Lang,
        "DATE" => Field::Date,
        "DEL" => Field::Deleted,
        "LIBRATE" => Field::LibRate,
        "KEYWORDS" => Field::KeyWords,
        // CODE / RATE are declared by the original but never read; URI / URL are "not supported anymore".
        _ => Field::None,
    }
}

/// Parse a `structure.info` line (`AUTHOR;GENRE;…;`). Returns the field order and whether a `FOLDER`
/// field is present (then the stored folder is used instead of the archive name).
fn parse_structure(info: &str) -> (Vec<Field>, bool) {
    let fields: Vec<Field> = info
        .trim()
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(field_of)
        .collect();
    let has_folder = fields.contains(&Field::Folder);
    (fields, has_folder)
}

/// One parsed `.inp` record (the original's `TBookRecord`, import subset).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InpRecord {
    pub authors: Vec<(String, String, String)>,
    pub genres: Vec<String>,
    pub title: String,
    pub series: String,
    pub seq_number: i32,
    pub file_name: String,
    pub ext: String,
    pub size: i64,
    pub lib_id: String,
    pub deleted: bool,
    pub date: String,
    pub inside_no: i32,
    pub folder: String,
    pub lib_rate: i32,
    pub lang: String,
    pub key_words: String,
}

/// The original's `CheckSymbols` (non-full): characters illegal in file names become spaces and
/// trailing dots are dropped.
pub fn check_symbols(input: &str) -> String {
    let mut s: String = input
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '*' | '?') { ' ' } else { c })
        .collect();
    while s.ends_with('.') {
        s.pop();
    }
    s
}

/// `И\Иванов Иван Иванович\` — the original's `GenerateBookLocation` (always `\`, as stored by it).
fn generate_book_location(full_name: &str) -> String {
    let name = check_symbols(full_name);
    // Deliberately not trimmed before picking the letter: a leading space sends the author to `_`.
    let letter = match name.chars().next() {
        Some(c) if c.is_alphanumeric() => c,
        _ => '_',
    };
    let trimmed = name.trim();
    let author = if trimmed.is_empty() { UNKNOWN_AUTHOR } else { trimmed };
    format!("{letter}\\{author}\\")
}

/// `1234 Просто книга` — the original's `GenerateFileName`.
fn generate_file_name(title: &str, lib_id: &str) -> String {
    let t = check_symbols(title);
    let t = t.trim();
    format!("{lib_id} {}", if t.is_empty() { NO_TITLE } else { t })
}

/// Parse a decimal the way `StrToIntDef(s, 0)` does.
fn int_or_zero<T: std::str::FromStr + Default>(s: &str) -> T {
    s.trim().parse().unwrap_or_default()
}

/// Split a `:`-terminated list. The original only takes items *followed by* the delimiter; real
/// files always terminate the last item, but a missing terminator is tolerated here rather than
/// silently dropping the last author/genre.
fn split_items(s: &str) -> impl Iterator<Item = &str> {
    s.split(ITEM_DELIM).filter(|i| !i.is_empty())
}

/// Parse one `.inp` line. `Err` = a structural/convert error (the original's `EConvertError`).
fn parse_record(line: &str, fields: &[Field]) -> std::result::Result<InpRecord, String> {
    let mut r = InpRecord::default();
    // `ExtractStrings` keeps empty fields (so positions line up) but stops at the end of the string.
    for (value, field) in line.split(FIELD_DELIM).zip(fields.iter()) {
        match field {
            Field::Author => {
                for a in split_items(value) {
                    let mut it = a.splitn(3, SUBITEM_DELIM);
                    let last = it.next().unwrap_or("").to_string();
                    let first = it.next().unwrap_or("").to_string();
                    let middle = it.next().unwrap_or("").to_string();
                    r.authors.push((last, first, middle));
                }
            }
            Field::Genre => r.genres.extend(split_items(value).map(str::to_string)),
            Field::Title => r.title = value.to_string(),
            Field::Series => r.series = value.to_string(),
            Field::SerNo => r.seq_number = int_or_zero(value),
            Field::File => r.file_name = check_symbols(value.trim()),
            Field::Ext => r.ext = format!(".{value}"),
            Field::Size => r.size = int_or_zero(value),
            Field::LibId => r.lib_id = value.to_string(),
            // `CompareStr(value, '1') >= 0` in the original: anything sorting at or after "1".
            Field::Deleted => r.deleted = value >= "1",
            Field::Date => {
                r.date = if value.is_empty() {
                    props::date_string(1970, 1, 1)
                } else {
                    let num = |range: std::ops::Range<usize>| -> std::result::Result<u32, String> {
                        value
                            .get(range)
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| format!("bad date '{value}'"))
                    };
                    props::date_string(num(0..4)? as i32, num(5..7)?, num(8..10)?)
                }
            }
            Field::InsideNo => {
                r.inside_no = value.trim().parse().map_err(|_| format!("bad INSNO '{value}'"))?
            }
            Field::Folder => r.folder = value.to_string(),
            Field::LibRate => r.lib_rate = int_or_zero(value),
            Field::Lang => r.lang = value.to_string(),
            Field::KeyWords => r.key_words = value.to_string(),
            Field::None => {}
        }
    }

    // `TBookRecord.Normalize`.
    if r.title.is_empty() {
        r.title = NO_TITLE.to_string();
    }
    for a in &mut r.authors {
        if a.0.is_empty() {
            a.0 = UNKNOWN_AUTHOR.to_string();
        }
    }
    if r.authors.is_empty() {
        r.authors.push((UNKNOWN_AUTHOR.to_string(), String::new(), String::new()));
    }
    if r.genres.is_empty() {
        r.genres.push(UNKNOWN_GENRE_CODE.to_string());
    }
    if r.date.is_empty() {
        r.date = props::date_string(1970, 1, 1);
    }
    Ok(r)
}

/// The parsed `collection.info` (the original's `TINPXHeader`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InpxHeader {
    pub name: String,
    pub file_name: String,
    pub content_type: i64,
    pub notes: String,
    pub url: String,
    pub script: String,
}

impl InpxHeader {
    pub fn parse(text: &str) -> Self {
        let text = text.trim_start_matches('\u{feff}');
        let lines: Vec<&str> = text.lines().collect();
        let get = |i: usize| lines.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut script = String::new();
        for l in lines.iter().skip(5) {
            script.push_str(l);
            script.push_str("\r\n");
        }
        Self {
            name: get(0),
            file_name: get(1),
            content_type: lines.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(props::CT_PRIVATE_FB),
            notes: get(3),
            url: get(4),
            script,
        }
    }
}

/// Read just the `collection.info` header of an INPX (the new-collection flow uses it to pre-fill
/// the collection name and type). `None` when the file has no header.
pub fn read_header(inpx: &Path) -> Result<Option<InpxHeader>> {
    let mut zip = open_zip(inpx)?;
    Ok(read_entry_text(&mut zip, COLLECTION_INFO)?.map(|t| InpxHeader::parse(&t)))
}

/// Outcome of an INPX import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InpxReport {
    /// Books inserted.
    pub added: usize,
    /// Records already present in the collection (same folder + file + position) — skipped.
    pub existing: usize,
    /// Records that could not be parsed or stored.
    pub errors: usize,
    /// `.inp` files processed.
    pub inp_files: usize,
    pub cancelled: bool,
}

impl InpxReport {
    pub fn summary(&self) -> String {
        let mut s = format!("Добавлено {} книг", self.added);
        if self.existing > 0 {
            s.push_str(&format!(", уже в коллекции: {}", self.existing));
        }
        if self.errors > 0 {
            s.push_str(&format!(", ошибок: {}", self.errors));
        }
        if self.cancelled {
            s.push_str(" (прервано)");
        }
        s
    }
}

fn zip_err(what: &str, e: impl std::fmt::Display) -> AppError {
    AppError::Other(format!("{what}: {e}"))
}

fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(path).map_err(|e| zip_err(&format!("open {}", path.display()), e))?;
    ZipArchive::new(file).map_err(|_| AppError::Other("Неправильный формат файла INPX!".to_string()))
}

fn entry_names(zip: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let zf = zip.by_index(i).map_err(|e| zip_err("inpx entry", e))?;
        names.push(zf.name().map_err(|e| zip_err("inpx entry name", e))?.to_string());
    }
    Ok(names)
}

fn read_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<Option<Vec<u8>>> {
    let mut zf = match zip.by_name(name) {
        Ok(zf) => zf,
        Err(_) => return Ok(None),
    };
    let mut data = Vec::new();
    zf.read_to_end(&mut data).map_err(|e| zip_err(&format!("read {name}"), e))?;
    Ok(Some(data))
}

fn read_entry_text(zip: &mut ZipArchive<File>, name: &str) -> Result<Option<String>> {
    Ok(read_entry(zip, name)?.map(|d| String::from_utf8_lossy(&d).into_owned()))
}

/// In-memory id caches so a million-record import does no per-book lookups for authors, series or
/// genres. Pre-loaded from the collection, so importing into a non-empty collection reuses its rows.
struct Caches {
    authors: HashMap<(String, String, String), i64>,
    series: HashMap<String, i64>,
    /// FB2 code (lowercased) → universal genre code.
    genre_by_fb2: HashMap<String, String>,
    /// Known universal genre codes (lowercased).
    genre_codes: HashSet<String>,
    /// Source-file identities already in the collection: (folder, file name, inside no).
    files: HashSet<(String, String, i32)>,
}

impl Caches {
    fn load(db: &Db) -> Result<Self> {
        let conn = db.connection();
        let mut authors = HashMap::new();
        {
            let mut st = conn.prepare("SELECT AuthorID, LastName, IFNULL(FirstName,''), IFNULL(MiddleName,'') FROM Authors")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                authors.insert((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?), r.get(0)?);
            }
        }
        let mut series = HashMap::new();
        {
            let mut st = conn.prepare("SELECT SeriesID, SeriesTitle FROM Series")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                series.insert(r.get::<_, String>(1)?, r.get(0)?);
            }
        }
        let mut genre_by_fb2 = HashMap::new();
        let mut genre_codes = HashSet::new();
        {
            let mut st = conn.prepare("SELECT GenreCode, FB2Code FROM Genres")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                let code: String = r.get(0)?;
                if let Some(fb2) = r.get::<_, Option<String>>(1)? {
                    if !fb2.is_empty() {
                        genre_by_fb2.entry(fb2.to_lowercase()).or_insert_with(|| code.clone());
                    }
                }
                genre_codes.insert(code.to_lowercase());
            }
        }
        let mut files = HashSet::new();
        {
            let mut st = conn.prepare("SELECT IFNULL(Folder,''), FileName, InsideNo FROM Books")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                files.insert((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?));
            }
        }
        Ok(Self { authors, series, genre_by_fb2, genre_codes, files })
    }

    fn author_id(&mut self, db: &Db, a: &(String, String, String)) -> Result<i64> {
        if let Some(id) = self.authors.get(a) {
            return Ok(*id);
        }
        let first = (!a.1.is_empty()).then_some(a.1.as_str());
        let middle = (!a.2.is_empty()).then_some(a.2.as_str());
        let search_name = search::upper(&search::format_name(&a.0, first, middle));
        // The original stores empty name parts as '' (not NULL); kept so its `=`-based lookups match.
        db.connection()
            .prepare_cached("INSERT INTO Authors (LastName, FirstName, MiddleName, SearchName) VALUES (?1,?2,?3,?4)")?
            .execute(params![a.0, a.1, a.2, search_name])?;
        let id = db.connection().last_insert_rowid();
        self.authors.insert(a.clone(), id);
        Ok(id)
    }

    fn series_id(&mut self, db: &Db, title: &str) -> Result<i64> {
        if let Some(id) = self.series.get(title) {
            return Ok(*id);
        }
        let conn = db.connection();
        // `SeriesTitle` is UNIQUE under a case-insensitive collation, so a title differing only in
        // case from an existing one must resolve to that row instead of failing the insert.
        let existing: Option<i64> = {
            use rusqlite::OptionalExtension;
            conn.prepare_cached("SELECT SeriesID FROM Series WHERE SeriesTitle = ?1")?
                .query_row([title], |r| r.get(0))
                .optional()?
        };
        let id = match existing {
            Some(id) => id,
            None => {
                conn.prepare_cached("INSERT INTO Series (SeriesTitle, SearchSeriesTitle) VALUES (?1,?2)")?
                    .execute(params![title, search::upper(title)])?;
                conn.last_insert_rowid()
            }
        };
        self.series.insert(title.to_string(), id);
        Ok(id)
    }

    /// Resolve an INPX genre item to the code stored in `Genre_List`. With FB2 genres the FB2 code is
    /// translated to the collection's universal code; a code the catalog doesn't know is kept as-is
    /// (the views join the catalog by either code), and an empty one becomes the "unknown" genre.
    fn genre_code(&mut self, db: &Db, item: &str, fb2: bool) -> Result<String> {
        let key = item.trim().to_lowercase();
        if key.is_empty() || key == UNKNOWN_GENRE_CODE {
            if self.genre_codes.insert(UNKNOWN_GENRE_CODE.to_string()) {
                db.connection().execute(
                    "INSERT OR IGNORE INTO Genres (GenreCode, ParentCode, FB2Code, GenreAlias) VALUES (?1, '0', NULL, ?2)",
                    params![UNKNOWN_GENRE_CODE, UNKNOWN_GENRE_ALIAS],
                )?;
            }
            return Ok(UNKNOWN_GENRE_CODE.to_string());
        }
        if fb2 {
            if let Some(code) = self.genre_by_fb2.get(&key) {
                return Ok(code.clone());
            }
        }
        Ok(item.trim().to_string())
    }
}

/// Options for [`import_inpx`].
#[derive(Debug, Clone)]
pub struct InpxOptions {
    /// The INPX carries FB2 genre codes (`gtFb2`, the usual case) rather than universal codes.
    pub fb2_genres: bool,
    /// Collection root folder — only used by online collections to flag already-downloaded books.
    pub root: Option<std::path::PathBuf>,
}

impl Default for InpxOptions {
    fn default() -> Self {
        Self { fb2_genres: true, root: None }
    }
}

/// Import every `.inp` of `inpx` into `db`. Commits once per `.inp` file; a cancelled import keeps
/// what was committed so far (re-running it skips those books and carries on).
pub fn import_inpx(db: &Db, inpx: &Path, opts: &InpxOptions, progress: &JobProgress) -> Result<InpxReport> {
    let mut zip = open_zip(inpx)?;
    let names = entry_names(&mut zip)?;

    let structure = read_entry_text(&mut zip, STRUCTURE_INFO)?;
    let (fields, use_stored_folder) = parse_structure(
        structure.as_deref().map(|s| s.trim_start_matches('\u{feff}')).unwrap_or(DEFAULT_STRUCTURE),
    );
    if fields.is_empty() {
        return Err(AppError::Other("Неправильный формат файла INPX!".to_string()));
    }

    let code = db.collection_code();
    let online = props::is_online(code);
    let non_fb2 = props::is_non_fb2(code);

    let inp_names: Vec<&String> = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".inp"))
        // `extra.inp` lists books that exist only online.
        .filter(|n| online || n.as_str() != "extra.inp")
        .collect();
    let total_files = inp_names.len().max(1);

    progress.set_percent(0.0);
    progress.set_comment("Чтение коллекции…");
    let mut caches = Caches::load(db)?;
    let mut report = InpxReport::default();

    // The DAO computes the Search* columns itself, so the schema triggers are pure overhead here
    // (the original's StartBatchUpdate / FinishBatchUpdate pair).
    db.set_triggers_enabled(false);
    // `Books` carries ~20 indexes; with SQLite's default 2 MB page cache a large import spends most of
    // its time re-reading index pages. Give this (import-only) connection a 256 MB cache.
    let _ = db.connection().execute_batch("PRAGMA cache_size = -262144; PRAGMA temp_store = MEMORY;");
    let outcome = (|| -> Result<()> {
        for (file_idx, name) in inp_names.iter().enumerate() {
            progress.info(format!("Обрабатываем файл {name}"));
            let Some(data) = read_entry(&mut zip, name)? else { continue };
            let text = String::from_utf8_lossy(&data);
            let text = text.trim_start_matches('\u{feff}');
            let lines: Vec<&str> = text.lines().collect();
            let line_count = lines.len().max(1);
            // 98058-98693.inp -> 98058-98693.zip
            let archive = Path::new(name.as_str()).with_extension("zip").to_string_lossy().into_owned();

            db.begin()?;
            let file_result = (|| -> Result<()> {
                for (j, line) in lines.iter().enumerate() {
                    if line.is_empty() {
                        continue;
                    }
                    if j % CHECKPOINT == 0 {
                        progress.set_percent((file_idx as f64 + j as f64 / line_count as f64) * 100.0 / total_files as f64);
                        progress.set_comment(format!("Добавлено {} книг", report.added));
                        if progress.is_cancelled() {
                            report.cancelled = true;
                            break;
                        }
                    }
                    let mut r = match parse_record(line, &fields) {
                        Ok(r) => r,
                        Err(_) => {
                            report.errors += 1;
                            progress.error(format!("Ошибка структуры inp. Файл {name}, Строка {j}"));
                            continue;
                        }
                    };
                    if r.file_name.is_empty() {
                        continue; // the original's InsertBook ignores records without a file name
                    }

                    let is_local = if online {
                        if !non_fb2 {
                            let a = &r.authors[0];
                            let full = search::format_name(
                                &a.0,
                                (!a.1.is_empty()).then_some(a.1.as_str()),
                                (!a.2.is_empty()).then_some(a.2.as_str()),
                            );
                            // И\Иванов Иван\1234 Просто книга.fb2.zip
                            r.folder = format!(
                                "{}{}.fb2.zip",
                                generate_book_location(&full),
                                generate_file_name(&r.title, &r.lib_id)
                            );
                        }
                        opts.root
                            .as_ref()
                            .map(|root| root.join(r.folder.replace('\\', std::path::MAIN_SEPARATOR_STR)).is_file())
                            .unwrap_or(false)
                    } else {
                        if !use_stored_folder {
                            r.folder = archive.clone();
                            r.inside_no = j as i32;
                        }
                        true
                    };

                    let key = (r.folder.clone(), r.file_name.clone(), r.inside_no);
                    if caches.files.contains(&key) {
                        report.existing += 1;
                        continue;
                    }

                    match insert_record(db, &mut caches, &r, is_local, opts.fb2_genres) {
                        Ok(_) => {
                            caches.files.insert(key);
                            report.added += 1;
                        }
                        Err(e) => {
                            report.errors += 1;
                            progress.error(format!("Ошибка базы данных при импорте книги. Файл {name}, Строка {j}: {e}"));
                        }
                    }
                }
                Ok(())
            })();
            match file_result {
                Ok(()) => db.commit()?,
                Err(e) => {
                    let _ = db.rollback();
                    return Err(e);
                }
            }
            report.inp_files += 1;
            if report.cancelled {
                break;
            }
        }
        Ok(())
    })();
    db.set_triggers_enabled(true);
    outcome?;

    progress.info(format!("Добавлено {} книг", report.added));

    // Collection properties carried by the index.
    if !report.cancelled {
        progress.set_comment("Обновление базы данных. Пожалуйста, подождите…");
        if let Some(text) = read_entry_text(&mut zip, COLLECTION_INFO)? {
            let h = InpxHeader::parse(&text);
            db.set_prop_str(props::PROP_NOTES, &h.notes)?;
            db.set_prop_str(props::PROP_URL, &h.url)?;
            db.set_prop_str(props::PROP_CONNECTIONSCRIPT, &h.script)?;
        }
        if let Some(text) = read_entry_text(&mut zip, VERSION_INFO)? {
            // -1 = the original's UNVERSIONED_COLLECTION.
            let v: i64 = text.trim().trim_start_matches('\u{feff}').parse().unwrap_or(-1);
            db.set_prop_int(props::PROP_DATAVERSION, v)?;
        }
        progress.set_percent(100.0);
    }
    progress.set_comment(report.summary());
    Ok(report)
}

fn insert_record(db: &Db, caches: &mut Caches, r: &InpRecord, is_local: bool, fb2_genres: bool) -> Result<i64> {
    let mut author_ids = Vec::with_capacity(r.authors.len());
    for a in &r.authors {
        let id = caches.author_id(db, a)?;
        if !author_ids.contains(&id) {
            author_ids.push(id);
        }
    }
    let mut genres: Vec<String> = Vec::with_capacity(r.genres.len());
    for g in &r.genres {
        let code = caches.genre_code(db, g, fb2_genres)?;
        if !genres.contains(&code) {
            genres.push(code);
        }
    }
    let (series_id, seq_number) = if r.series.is_empty() {
        (None, None)
    } else {
        // The original zeroes absurd sequence numbers.
        let seq = if r.seq_number > 5000 { 0 } else { r.seq_number };
        (Some(caches.series_id(db, &r.series)?), Some(seq))
    };
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let book = Book {
        book_id: 0,
        lib_id: r.lib_id.clone(),
        title: r.title.clone(),
        series_id,
        seq_number,
        update_date: r.date.clone(),
        lib_rate: r.lib_rate,
        lang: opt(&r.lang),
        folder: Some(r.folder.clone()),
        file_name: r.file_name.clone(),
        inside_no: r.inside_no,
        ext: Some(r.ext.clone()),
        book_size: Some(r.size),
        is_local,
        is_deleted: r.deleted,
        key_words: opt(&r.key_words),
        rate: 0,
        progress: 0,
        annotation: None,
        review: None,
    };
    let genre_refs: Vec<&str> = genres.iter().map(String::as_str).collect();
    db.insert_book_no_tx(&book, &author_ids, &genre_refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_and_record_parsing() {
        let (fields, folder) = parse_structure("AUTHOR;GENRE;TITLE;SERIES;SERNO;FILE;SIZE;LIBID;DEL;EXT;DATE;LANG;LIBRATE;KEYWORDS;");
        assert_eq!(fields.len(), 14);
        assert!(!folder);
        let line = "Толстой,Лев,Николаевич:Иванов,Иван,:\u{4}prose_classic:sf:\u{4}Война и мир\u{4}Эпопея\u{4}2\u{4}12345\u{4}777\u{4}12345\u{4}0\u{4}fb2\u{4}2011-03-05\u{4}ru\u{4}4\u{4}война, мир\u{4}";
        let r = parse_record(line, &fields).unwrap();
        assert_eq!(r.authors.len(), 2);
        assert_eq!(r.authors[0], ("Толстой".into(), "Лев".into(), "Николаевич".into()));
        assert_eq!(r.authors[1], ("Иванов".into(), "Иван".into(), "".into()));
        assert_eq!(r.genres, vec!["prose_classic", "sf"]);
        assert_eq!(r.title, "Война и мир");
        assert_eq!(r.series, "Эпопея");
        assert_eq!(r.seq_number, 2);
        assert_eq!(r.file_name, "12345");
        assert_eq!(r.ext, ".fb2");
        assert_eq!(r.size, 777);
        assert!(!r.deleted);
        assert_eq!(r.date, "2011-03-05 00:00:00.000");
        assert_eq!(r.lang, "ru");
        assert_eq!(r.lib_rate, 4);
        assert_eq!(r.key_words, "война, мир");
    }

    #[test]
    fn record_normalization_and_errors() {
        let (fields, _) = parse_structure(DEFAULT_STRUCTURE);
        // No author, no genre, no title, deleted, empty date.
        let r = parse_record("\u{4}\u{4}\u{4}\u{4}\u{4}f1\u{4}\u{4}9\u{4}1\u{4}fb2\u{4}\u{4}", &fields).unwrap();
        assert_eq!(r.authors, vec![(UNKNOWN_AUTHOR.to_string(), String::new(), String::new())]);
        assert_eq!(r.genres, vec![UNKNOWN_GENRE_CODE]);
        assert_eq!(r.title, NO_TITLE);
        assert!(r.deleted);
        assert_eq!(r.date, "1970-01-01 00:00:00.000");
        // A malformed date is a structure error, not a panic.
        assert!(parse_record("a,b,c:\u{4}g:\u{4}t\u{4}\u{4}\u{4}f\u{4}1\u{4}1\u{4}0\u{4}fb2\u{4}20x1-aa\u{4}", &fields).is_err());
        // Short line: missing trailing fields are simply absent.
        let r = parse_record("a,b,c:\u{4}g:\u{4}t", &fields).unwrap();
        assert_eq!(r.title, "t");
        assert_eq!(r.file_name, "");
    }

    #[test]
    fn locations_and_header() {
        assert_eq!(check_symbols("a:b?c..."), "a b c");
        assert_eq!(generate_book_location("Иванов Иван"), "И\\Иванов Иван\\");
        assert_eq!(generate_book_location(" Иванов"), "_\\Иванов\\");
        assert_eq!(generate_file_name("Просто: книга", "1234"), "1234 Просто  книга");
        let h = InpxHeader::parse("Lib.rus.ec\r\nlibrusec\r\n65536\r\nnotes here\r\nhttp://lib.rus.ec/\r\nGET %URL%\r\nCHECK\r\n");
        assert_eq!(h.name, "Lib.rus.ec");
        assert_eq!(h.content_type, 65536);
        assert_eq!(h.notes, "notes here");
        assert_eq!(h.url, "http://lib.rus.ec/");
        assert_eq!(h.script, "GET %URL%\r\nCHECK\r\n");
    }
}

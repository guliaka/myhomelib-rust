//! INPX import/export — the original's `TImportInpxThread` / `TExport2INPXThread`.
//!
//! An INPX is a ZIP with:
//! - `books.inp` (any `*.inp` except `extra.inp`): one record per line, fields separated by
//!   `Chr(4)`; author parts by `,`, list items (authors/genres) by `:` — the lib.rus.ec format.
//! - `structure.info`: the field order, `AUTHOR;GENRE;TITLE;…;` (semicolon-separated).
//! - `version.info`: the collection data version.
//! - `collection.info`: 6 CRLF lines — Name / FileName / ContentType / Notes / URL / Script.
//!
//! Local import (the original's non-online path) marks books local, points `Folder` at the
//! companion `<name>.zip` (the FB2 archives that ship next to the .inp files) and sets `InsideNo`
//! to the record's line index — exactly the original's linkage — unless the record itself carries
//! a FOLDER field. Dedupe is the source-file identity (`Folder + FileName + InsideNo`), same as
//! the FB2 importer.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::core::Book;
use crate::db::{BookScope, Db};
use crate::error::{AppError, Result};

const FIELD_DELIM: char = '\u{04}';
const ITEM_DELIM: char = ':';
const SUBITEM_DELIM: char = ',';

/// The original's DEFAULTSTRUCTURE (used when `structure.info` is absent).
pub const DEFAULT_STRUCTURE: [&str; 14] = [
    "AUTHOR", "GENRE", "TITLE", "SERIES", "SERNO", "FILE", "SIZE", "LIBID", "DEL", "EXT", "DATE",
    "LANG", "LIBRATE", "KEYWORDS",
];

/// The export field order (the original's INPRecordCreate writes exactly this).
const EXPORT_FIELDS: [&str; 15] = [
    "AUTHOR", "GENRE", "TITLE", "SERIES", "SERNO", "FILE", "SIZE", "LIBID", "DEL", "EXT", "DATE",
    "INSNO", "FOLDER", "LANG", "KEYWORDS",
];

/// One parsed INPX record, keeping every field the import path needs.
#[derive(Debug, Clone, Default)]
pub struct InpxRecord {
    pub authors: Vec<(String, String, String)>, // (last, first, middle)
    pub genres: Vec<String>,
    pub title: String,
    pub series: Option<String>,
    pub seq: Option<i32>,
    pub file: String,
    pub size: Option<i64>,
    pub libid: Option<String>,
    pub deleted: bool,
    pub ext: Option<String>,
    pub date: Option<String>,
    pub inside_no: Option<i32>,
    pub folder: Option<String>,
    pub lang: Option<String>,
    pub lib_rate: Option<i32>,
    pub keywords: Option<String>,
}

/// The `collection.info` header (the original's `TINPXHeader`).
#[derive(Debug, Clone, Default)]
pub struct InpxHeader {
    pub name: String,
    pub file_name: String,
    pub content_type: i64,
    pub notes: String,
    pub url: String,
    pub script: String,
}

impl InpxHeader {
    /// The original's `AsString`: 6 CRLF-separated lines.
    fn to_string_crlf(&self) -> String {
        format!(
            "{}\r\n{}\r\n{}\r\n{}\r\n{}\r\n{}",
            self.name, self.file_name, self.content_type, self.notes, self.url, self.script
        )
    }

    fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let next = |l: &mut std::str::Lines| l.next().unwrap_or_default().trim().to_string();
        Self {
            name: next(&mut lines),
            file_name: next(&mut lines),
            content_type: next(&mut lines).parse().unwrap_or(0),
            notes: next(&mut lines),
            url: next(&mut lines),
            script: lines.collect::<Vec<_>>().join("\r\n"),
        }
    }
}

/// Parse `structure.info` into field codes (the original's `GetFields`).
pub fn parse_structure(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Parse one record line against the field order.
pub fn parse_record(line: &str, fields: &[String]) -> InpxRecord {
    let mut r = InpxRecord::default();
    let parts: Vec<&str> = line.split(FIELD_DELIM).collect();
    for (i, code) in fields.iter().enumerate() {
        let Some(&val) = parts.get(i) else { break };
        match code.as_str() {
            "AUTHOR" => {
                r.authors = val
                    .split(ITEM_DELIM)
                    .filter(|a| !a.trim().is_empty())
                    .map(|a| {
                        let mut it = a.split(SUBITEM_DELIM);
                        (
                            it.next().unwrap_or("").trim().to_string(),
                            it.next().unwrap_or("").trim().to_string(),
                            it.next().unwrap_or("").trim().to_string(),
                        )
                    })
                    .collect();
            }
            "GENRE" => {
                r.genres = val
                    .split(ITEM_DELIM)
                    .filter(|g| !g.trim().is_empty())
                    .map(|g| g.trim().to_string())
                    .collect();
            }
            "TITLE" => r.title = val.trim().to_string(),
            "SERIES" if !val.trim().is_empty() => r.series = Some(val.trim().to_string()),
            "SERNO" if !val.trim().is_empty() => r.seq = val.trim().parse().ok(),
            "FILE" => r.file = val.trim().to_string(),
            "SIZE" if !val.trim().is_empty() => r.size = val.trim().parse().ok(),
            "LIBID" if !val.trim().is_empty() => r.libid = Some(val.trim().to_string()),
            "DEL" => r.deleted = val.trim() == "1",
            "EXT" if !val.trim().is_empty() => r.ext = Some(val.trim().to_string()),
            "DATE" if !val.trim().is_empty() => r.date = Some(val.trim().to_string()),
            "INSNO" if !val.trim().is_empty() => r.inside_no = val.trim().parse().ok(),
            "FOLDER" if !val.trim().is_empty() => r.folder = Some(val.trim().to_string()),
            "LANG" if !val.trim().is_empty() => r.lang = Some(val.trim().to_string()),
            "LIBRATE" if !val.trim().is_empty() => r.lib_rate = val.trim().parse().ok(),
            "KEYWORDS" if !val.trim().is_empty() => r.keywords = Some(val.trim().to_string()),
            _ => {}
        }
    }
    r
}

/// Result of an INPX import.
#[derive(Debug, Clone, Default)]
pub struct InpxImportReport {
    pub imported: usize,
    pub existing: usize,
    pub errors: Vec<(String, String)>,
    pub header: Option<InpxHeader>,
    pub version: Option<i64>,
}

/// Import an INPX file into a collection (the original's `TImportInpxThread.Import`).
///
/// `extra_handler` — called with (libid, rate, progress, review) rows for any `extra.inp` content
/// (the user-preferences file lib.rus.ec ships in online INPXes); the caller applies them to the
/// collection. Local INPXes rarely carry one.
pub fn import_inpx(db: &mut Db, path: &Path) -> Result<InpxImportReport> {
    let file = File::open(path).map_err(|e| AppError::msg(format!("open {}: {e}", path.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::msg(format!("not an INPX (zip) file: {e}")))?;

    let mut structure: Vec<String> = DEFAULT_STRUCTURE.iter().map(|s| s.to_string()).collect();
    if let Ok(Some(text)) = read_zip_text(&mut zip, "structure.info") {
        let parsed = parse_structure(&text);
        if !parsed.is_empty() {
            structure = parsed;
        }
    }

    // Local import is faster with the schema's search-column triggers off; the DAO still computes
    // the Search* columns itself (D2), so correctness is unaffected.
    db.set_triggers_enabled(false);
    let mut report = InpxImportReport::default();

    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else { continue };
        let name = entry.name().map(|n| n.to_string()).unwrap_or_default();
        if !name.to_lowercase().ends_with(".inp") || name.eq_ignore_ascii_case("extra.inp") {
            continue;
        }
        let text = match read_entry_text(entry) {
            Ok(t) => t,
            Err(e) => {
                report.errors.push((name.clone(), e.to_string()));
                continue;
            }
        };

        for (j, line) in text.lines().enumerate() {
            if !line.contains(FIELD_DELIM) {
                continue; // header/noise line
            }
            let rec = parse_record(line, &structure);
            let inside = rec.inside_no.unwrap_or(j as i32);
            // Companion-archive linkage (the original's local path): books live in <name>.zip
            // next to the .inp the record came from, unless the record carries its own FOLDER.
            let folder = rec.folder.clone().unwrap_or_else(|| {
                let stem = Path::new(&name).with_extension("zip");
                stem.to_string_lossy().to_string()
            });

            // Dedupe on source-file identity (D5): same folder+file+inside no = already known.
            match db.find_book_by_file(Some(&folder), &rec.file, inside) {
                Ok(Some(_)) => {
                    report.existing += 1;
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    report.errors.push((format!("{name}:{j}"), e.to_string()));
                    continue;
                }
            }

            let series_id = match &rec.series {
                Some(s) if !s.is_empty() => Some(db.upsert_series(s)?),
                _ => None,
            };
            let mut author_ids = Vec::new();
            for (last, first, middle) in &rec.authors {
                author_ids.push(db.upsert_author(
                    last,
                    if first.is_empty() { None } else { Some(first) },
                    if middle.is_empty() { None } else { Some(middle) },
                )?);
            }
            let genre_refs: Vec<&str> = rec.genres.iter().map(String::as_str).collect();
            let book = Book {
                book_id: 0,
                lib_id: rec.libid.clone().unwrap_or_default(),
                title: rec.title.clone(),
                series_id,
                seq_number: rec.seq,
                update_date: rec.date.clone().unwrap_or_else(|| "1970-01-01".into()),
                lib_rate: rec.lib_rate.unwrap_or(0),
                lang: rec.lang.clone(),
                folder: Some(folder),
                file_name: rec.file.clone(),
                inside_no: inside,
                ext: rec.ext.clone(),
                book_size: rec.size,
                is_local: true,
                is_deleted: rec.deleted,
                key_words: rec.keywords.clone(),
                rate: 0,
                progress: 0,
                annotation: None,
                review: None,
            };
            match db.insert_book(&book, &author_ids, &genre_refs) {
                Ok(_) => report.imported += 1,
                Err(e) => report.errors.push((format!("{name}:{j}"), e.to_string())),
            }
        }
    }
    db.set_triggers_enabled(true);

    if let Ok(Some(text)) = read_zip_text(&mut zip, "collection.info") {
        report.header = Some(InpxHeader::parse(&text));
    }
    if let Ok(Some(text)) = read_zip_text(&mut zip, "version.info") {
        report.version = text.trim().parse().ok();
    }
    Ok(report)
}

/// Export the whole collection as an INPX file (the original's `TExport2INPXThread`).
/// `header` supplies the registry fields for `collection.info`; `version` for `version.info`.
pub fn export_inpx(db: &Db, path: &Path, header: &InpxHeader, version: &str) -> Result<usize> {
    const PAGE: i64 = 5000;
    let mut lines: Vec<String> = Vec::new();
    let total = db.count_scope_books(&BookScope::All)?;

    let mut offset = 0i64;
    while offset < total {
        let books = db.list_scope_books(&BookScope::All, PAGE, offset)?;
        for b in &books {
            let authors = db
                .book_authors(b.book_id)?
                .iter()
                .map(|a| {
                    format!(
                        "{},{},{}",
                        a.last_name,
                        a.first_name.clone().unwrap_or_default(),
                        a.middle_name.clone().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(&ITEM_DELIM.to_string());
            // FB2 genre codes when the catalog has them (the original exports gtFb2 codes),
            // else the internal genre code.
            let genres = db
                .book_genres(b.book_id)?
                .iter()
                .map(|code| {
                    let fb2: Option<String> = db
                        .connection()
                        .query_row(
                            "SELECT FB2Code FROM Genres WHERE GenreCode = ?1",
                            [code],
                            |r| r.get(0),
                        )
                        .ok()
                        .flatten();
                    fb2.filter(|f| !f.is_empty()).unwrap_or_else(|| code.clone())
                })
                .collect::<Vec<_>>()
                .join(&ITEM_DELIM.to_string());
            let series = match b.series_id {
                Some(id) => db
                    .get_series(id)
                    .ok()
                    .flatten()
                    .map(|s| s.series_title)
                    .unwrap_or_default(),
                None => String::new(),
            };
            let ext = b.ext.clone().unwrap_or_default();
            let date = b.update_date.split(' ').next().unwrap_or_default().to_string();
            let row = [
                authors,
                genres,
                b.title.replace(['\r', '\n'], " "),
                series,
                b.seq_number.map(|n| n.to_string()).unwrap_or_default(),
                b.file_name.clone(),
                b.book_size.map(|n| n.to_string()).unwrap_or_default(),
                b.lib_id.clone(),
                if b.is_deleted { "1".into() } else { "0".into() },
                ext,
                date,
                b.inside_no.to_string(),
                b.folder.clone().unwrap_or_default(),
                b.lang.clone().unwrap_or_default(),
                b.key_words.clone().unwrap_or_default(),
            ];
            lines.push(row.join(&FIELD_DELIM.to_string()));
        }
        offset += PAGE;
    }

    let file =
        File::create(path).map_err(|e| AppError::msg(format!("create {}: {e}", path.display())))?;
    let mut zip_writer = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = Default::default();

    // books.inp — UTF-8, CRLF line ends (the original's TStreamWriter).
    let books_text = lines.join("\r\n") + "\r\n";
    zip_writer
        .start_file("books.inp", opts)
        .map_err(|e| AppError::msg(format!("zip: {e}")))?;
    zip_writer
        .write_all(books_text.as_bytes())
        .map_err(|e| AppError::msg(format!("zip: {e}")))?;

    zip_writer.start_file("version.info", opts).ok();
    zip_writer.write_all(version.trim().as_bytes()).ok();
    zip_writer.start_file("structure.info", opts).ok();
    let structure_text = EXPORT_FIELDS.join(";") + ";";
    zip_writer.write_all(structure_text.as_bytes()).ok();
    zip_writer.start_file("collection.info", opts).ok();
    zip_writer
        .write_all(header.to_string_crlf().as_bytes())
        .ok();
    zip_writer
        .finish()
        .map_err(|e| AppError::msg(format!("zip: {e}")))?;
    Ok(lines.len())
}

// -- zip helpers ---------------------------------------------------------------------------

/// Read a zip entry by exact name into a String (UTF-8).
fn read_zip_text<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> std::result::Result<Option<String>, zip::result::ZipError> {
    let mut found: Option<String> = None;
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index(i) {
            if entry.name().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false) {
                found = read_entry_text(entry).ok();
                break;
            }
        }
    }
    Ok(found)
}

fn read_entry_text<R: Read>(mut entry: zip::read::ZipFile<'_, R>) -> std::io::Result<String> {
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(text)
}

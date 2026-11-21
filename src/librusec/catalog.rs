//! INPX catalog parser (delimited text) for remote browse/search.
//!
//! The original `TImportInpxThread` reads an INPX as delimited text, not XML: fields are separated
//! by `Chr(4)`, author parts by `:` then `,`, and genre codes by `:`. A leading header declares the
//! field order (`Structure:`) and a base URL; every following line that contains a field separator is
//! one book record. This mirrors the original's `ParseData`/`GetFields`.

const FIELD_DELIM: char = '\u{04}'; // Chr(4) — separates fields within a record
const ITEM_DELIM: char = ':'; // separates list items (authors, genres)
const SUBITEM_DELIM: char = ','; // separates an author's Last / First / Middle

/// Default field order when the header omits `Structure:` (the original's DEFAULTSTRUCTURE).
const DEFAULT_STRUCTURE: [&str; 14] = [
    "AUTHOR",
    "GENRE",
    "TITLE",
    "SERIES",
    "SERNO",
    "FILE",
    "SIZE",
    "LIBID",
    "DEL",
    "EXT",
    "DATE",
    "LANG",
    "LIBRATE",
    "KEYWORDS",
];

#[derive(Debug, Clone, Default)]
pub struct CatalogBook {
    pub title: String,
    /// `(last, first, middle)` per author.
    pub authors: Vec<(String, String, String)>,
    pub genres: Vec<String>,
    pub series: Option<String>,
    pub libid: Option<String>,
    pub lang: Option<String>,
    pub ext: Option<String>,
    /// The book's download reference (relative or absolute), from the `URL`/`URI` field.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub base_url: Option<String>,
    pub books: Vec<CatalogBook>,
}

fn parse_authors(s: &str) -> Vec<(String, String, String)> {
    s.split(ITEM_DELIM)
        .filter(|a| !a.is_empty())
        .map(|a| {
            let mut it = a.split(SUBITEM_DELIM);
            let last = it.next().unwrap_or("").to_string();
            let first = it.next().unwrap_or("").to_string();
            let middle = it.next().unwrap_or("").to_string();
            (last, first, middle)
        })
        .collect()
}

fn parse_record(record: &str, fields: &[&str]) -> CatalogBook {
    let mut book = CatalogBook::default();
    let parts: Vec<&str> = record.split(FIELD_DELIM).collect();
    for (i, code) in fields.iter().enumerate() {
        if i >= parts.len() {
            break;
        }
        let val = parts[i];
        match *code {
            "AUTHOR" => book.authors = parse_authors(val),
            "GENRE" => {
                book.genres = val
                    .split(ITEM_DELIM)
                    .filter(|g| !g.is_empty())
                    .map(String::from)
                    .collect()
            }
            "TITLE" => book.title = val.to_string(),
            "SERIES" if !val.is_empty() => book.series = Some(val.into()),
            "LIBID" if !val.is_empty() => book.libid = Some(val.into()),
            "LANG" if !val.is_empty() => book.lang = Some(val.into()),
            "EXT" if !val.is_empty() => book.ext = Some(val.into()),
            "URI" | "URL" if !val.is_empty() => book.url = Some(val.into()),
            _ => {}
        }
    }
    book
}

/// If `line` starts with `<key>:` (case-insensitive) return the trimmed value after it.
fn key_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    if line.to_lowercase().starts_with(&prefix.to_lowercase()) {
        Some(line[prefix.len()..].trim())
    } else {
        None
    }
}

/// Parse an INPX catalog. A line is a *record* iff it contains the field separator; other leading
/// lines are header (`Structure:` / `URL:` keys; anything else is ignored).
pub fn parse_inpx(text: &str) -> Catalog {
    let mut base_url = None;
    let mut structure: Option<Vec<&str>> = None;
    let mut records: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.contains(FIELD_DELIM) {
            records.push(line.to_string());
            continue;
        }
        // Header line.
        let trimmed = line.trim_start();
        if let Some(rest) = key_value(trimmed, "Structure") {
            structure = Some(
                rest.split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect(),
            );
        } else if let Some(rest) = key_value(trimmed, "Base URL") {
            base_url = Some(rest.to_string());
        } else if let Some(rest) = key_value(trimmed, "URL") {
            base_url = Some(rest.to_string());
        }
    }

    let fields = structure.as_deref().unwrap_or(&DEFAULT_STRUCTURE);
    let books = records.iter().map(|r| parse_record(r, fields)).collect();
    Catalog { base_url, books }
}

impl Catalog {
    pub fn len(&self) -> usize {
        self.books.len()
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    /// Case-insensitive search over titles and authors; an empty query returns all indices.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return (0..self.books.len()).collect();
        }
        self.books
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.title.to_lowercase().contains(&q)
                    || b.authors.iter().any(|(last, first, _)| {
                        format!("{last} {first}").to_lowercase().contains(&q)
                    })
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Resolve a book's download URL: absolute as-is, else joined onto the catalog base URL.
    pub fn resolve_url(&self, book: &CatalogBook) -> Option<String> {
        match (&book.url, &self.base_url) {
            (Some(u), _) if u.starts_with("http://") || u.starts_with("https://") => Some(u.clone()),
            (Some(rel), Some(base)) => Some(format!("{base}{rel}")),
            _ => None,
        }
    }
}

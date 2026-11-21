//! Online-collection service — the thin live-HTTP edge of Phase 6.
//!
//! The pure logic (script parsing, INPX catalog, download queue, script executor) lives in the sibling
//! modules and is unit-tested against mocks; this module just drives a real [`HttpTransport`]. It is the
//! part that touches the network, so it is exercised by the GUI/CLI, not by offline tests.

use std::path::Path;

use crate::error::{AppError, Result};

use super::catalog::{parse_inpx, Catalog, CatalogBook};
use super::client::{Downloader, HttpTransport, Transport};
use super::command::parse_commands;

/// Fetch the catalog INPX served at `url` and parse it.
pub fn fetch_catalog(url: &str) -> Result<Catalog> {
    let transport = HttpTransport::new();
    let resp = transport.get(url, None)?;
    let text = String::from_utf8(resp.body)
        .map_err(|e| AppError::Other(format!("catalog is not valid UTF-8: {e}")))?;
    Ok(parse_inpx(&text))
}

/// The macro table a connection script can reference, built from the Librusec config.
pub fn build_macros(user: &str, password: &str, url: &str) -> Vec<(String, String)> {
    vec![
        ("%USER%".to_string(), user.to_string()),
        ("%PASS%".to_string(), password.to_string()),
        ("%URL%".to_string(), url.to_string()),
    ]
}

/// Book-field macros the original exposes to a connection script (every `TBookRecord` field, upper-cased).
pub fn book_macros(book: &CatalogBook) -> Vec<(String, String)> {
    let mut m = Vec::new();
    if !book.title.is_empty() {
        m.push(("%TITLE%".to_string(), book.title.clone()));
    }
    if let Some(u) = &book.url {
        m.push(("%BOOKURL%".to_string(), u.clone()));
    }
    if let Some(id) = &book.libid {
        m.push(("%LIBID%".to_string(), id.clone()));
    }
    for (last, first, middle) in &book.authors {
        // The original's author macro is the formatted name; we expose "Last First Middle" (trimmed).
        let mut s = last.clone();
        if !first.is_empty() {
            s.push(' ');
            s.push_str(first);
        }
        if !middle.is_empty() {
            s.push(' ');
            s.push_str(middle);
        }
        m.push(("%AUTHOR%".to_string(), s.trim().to_string()));
        break; // first author, matching the primary-author convention
    }
    m
}

/// Download one catalog book to `out_path` by running its connection script against `transport`.
pub fn download_book(
    transport: &dyn Transport,
    user: &str,
    password: &str,
    url: &str,
    script: &str,
    book: &CatalogBook,
    out_path: &Path,
) -> Result<()> {
    let mut macros = build_macros(user, password, url);
    macros.extend(book_macros(book));
    let cmds = parse_commands(script, &macros);
    Downloader::new(transport).run(&cmds, out_path)
}

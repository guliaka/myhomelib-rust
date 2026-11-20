//! Headless export: rebuild an FB2 document from a stored book's metadata (title-info + annotation)
//! and write it to disk. The full body text lives in the source file, not the DB, so this is a
//! metadata-only reconstruction — useful for catalog exchange and as a round-trip check on import.

use std::fs;
use std::path::Path;

use crate::db::Db;
use crate::error::{AppError, Result};
use crate::fb2::model::{Element, Node};
use crate::fb2::{serialize, Fb2};

/// Rebuilds FB2 documents from stored catalog metadata.
pub struct Exporter<'a> {
    db: &'a Db,
}

impl<'a> Exporter<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// Build a metadata-only FB2 document for the book with `book_id`.
    pub fn book_to_fb2(&self, book_id: i64) -> Result<Fb2> {
        let book = self
            .db
            .get_book(book_id)?
            .ok_or_else(|| AppError::Other(format!("no book with id {book_id}")))?;

        let mut title_info_children: Vec<Node> = Vec::new();

        if !book.lib_id.is_empty() {
            title_info_children.push(Node::Element(text_elem("id", &book.lib_id)));
        }

        if let Some(g) = self.db.book_genres(book_id)?.into_iter().next() {
            title_info_children.push(Node::Element(text_elem("genre", &g)));
        }

        for a in self.db.book_authors(book_id)? {
            let mut author = Element {
                name: "author".into(),
                attrs: vec![],
                children: vec![],
            };
            author.children.push(Node::Element(text_elem("last-name", &a.last_name)));
            if let Some(f) = &a.first_name {
                author.children.push(Node::Element(text_elem("first-name", f)));
            }
            if let Some(m) = &a.middle_name {
                author.children.push(Node::Element(text_elem("middle-name", m)));
            }
            title_info_children.push(Node::Element(author));
        }

        if let Some(sid) = book.series_id {
            if let Some(s) = self.db.get_series(sid)? {
                let mut series = Element {
                    name: "series".into(),
                    attrs: vec![],
                    children: vec![],
                };
                series.children.push(Node::Element(text_elem("name", &s.series_title)));
                if let Some(n) = book.seq_number {
                    series.children.push(Node::Element(text_elem("number", &n.to_string())));
                }
                title_info_children.push(Node::Element(series));
            }
        }

        title_info_children.push(Node::Element(text_elem("book-title", &book.title)));

        if let Some(l) = &book.lang {
            title_info_children.push(Node::Element(text_elem("lang", l)));
        }

        if let Some(ann) = &book.annotation {
            let mut annotation = Element {
                name: "annotation".into(),
                attrs: vec![],
                children: vec![],
            };
            for line in ann.lines() {
                if !line.trim().is_empty() {
                    annotation.children.push(Node::Element(text_elem("p", line)));
                }
            }
            if !annotation.children.is_empty() {
                title_info_children.push(Node::Element(annotation));
            }
        }

        let title_info = Element {
            name: "title-info".into(),
            attrs: vec![],
            children: title_info_children,
        };
        let description = Element {
            name: "description".into(),
            attrs: vec![],
            children: vec![Node::Element(title_info)],
        };
        let root = Element {
            name: "FictionBook".into(),
            attrs: vec![(
                "xmlns".into(),
                "http://www.gribuser.ru/xml/fictionbook/2".into(),
            )],
            children: vec![Node::Element(description)],
        };
        Ok(Fb2 { root })
    }

    /// Serialize the book's metadata to an FB2 file at `path`. Returns the bytes written.
    pub fn export_book(&self, book_id: i64, path: &Path) -> Result<Vec<u8>> {
        let fb2 = self.book_to_fb2(book_id)?;
        let bytes = serialize(&fb2);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| AppError::Other(format!("mkdir {}: {e}", parent.display())))?;
            }
        }
        fs::write(path, &bytes).map_err(|e| AppError::Other(format!("write {}: {e}", path.display())))?;
        Ok(bytes)
    }
}

/// A simple `<name>text</name>` element (self-closing when `text` is empty).
fn text_elem(name: &str, text: &str) -> Element {
    let mut el = Element {
        name: name.to_string(),
        attrs: vec![],
        children: vec![],
    };
    if !text.is_empty() {
        el.children.push(Node::Text(text.to_string()));
    }
    el
}

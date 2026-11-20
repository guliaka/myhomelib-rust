//! FBD container support. An `.fbd` file is a ZIP archive holding one or more `.fb2` files plus an
//! optional cover image (e.g. `cover.jpg`) — mirroring MyHomeLib's use of System.Zip for FBD.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::error::{AppError, Result};

use super::model::Fb2;
use super::parser::parse_fb2;
use super::writer::serialize;

/// Contents of an FBD archive: the parsed FB2 book(s) plus any non-FB2 files (covers).
#[derive(Debug, Clone)]
pub struct Fbd {
    /// Parsed FB2 documents found in the archive.
    pub books: Vec<Fb2>,
    /// Non-FB2 entries (e.g. cover images): filename → bytes.
    pub attachments: Vec<(String, Vec<u8>)>,
}

impl Fbd {
    /// Open and parse an `.fbd` file from disk.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| AppError::Other(format!("fbd open {}: {e}", path.display())))?;
        Self::from_reader(file)
    }

    /// Parse an FBD from any `Read + Seek` source.
    pub fn from_reader(src: impl Read + Seek) -> Result<Self> {
        let mut archive = ZipArchive::new(src).map_err(|e| AppError::Other(format!("fbd (zip): {e}")))?;
        let mut books = Vec::new();
        let mut attachments = Vec::new();
        for i in 0..archive.len() {
            let mut zf = archive.by_index(i).map_err(|e| AppError::Other(format!("fbd entry {i}: {e}")))?;
            let name = zf
                .name()
                .map_err(|e| AppError::Other(format!("fbd entry {i} name: {e}")))?
                .to_string();
            let mut data = Vec::new();
            Read::read_to_end(&mut zf, &mut data)
                .map_err(|e| AppError::Other(format!("fbd read {name}: {e}")))?;
            if name.to_lowercase().ends_with(".fb2") {
                books.push(parse_fb2(&data)?);
            } else {
                attachments.push((name, data));
            }
        }
        Ok(Fbd { books, attachments })
    }

    /// Write an FBD archive to `path` from a single FB2 plus an optional cover image.
    pub fn write(path: &Path, book: &Fb2, cover: Option<&[u8]>) -> Result<()> {
        let file = File::create(path).map_err(|e| AppError::Other(format!("fbd create {}: {e}", path.display())))?;
        Self::write_to(file, book, cover)
    }

    /// Write an FBD archive to any `Write + Seek` sink.
    pub fn write_to(out: impl Write + Seek, book: &Fb2, cover: Option<&[u8]>) -> Result<()> {
        let mut zw = ZipWriter::new(out);
        let bytes = serialize(book);
        zw.start_file("book.fb2", SimpleFileOptions::default())
            .map_err(|e| AppError::Other(format!("fbd start book.fb2: {e}")))?;
        zw.write_all(&bytes).map_err(|e| AppError::Other(format!("fbd write book.fb2: {e}")))?;

        if let Some(c) = cover {
            zw.start_file(cover_name(c), SimpleFileOptions::default())
                .map_err(|e| AppError::Other(format!("fbd start cover: {e}")))?;
            zw.write_all(c).map_err(|e| AppError::Other(format!("fbd write cover: {e}")))?;
        }

        zw.finish().map_err(|e| AppError::Other(format!("fbd finish: {e}")))?;
        Ok(())
    }
}

/// Pick a cover filename from the image's magic bytes.
fn cover_name(bytes: &[u8]) -> String {
    if bytes.len() >= 4 && &bytes[0..4] == b"\x89PNG" {
        "cover.png".to_string()
    } else {
        "cover.jpg".to_string()
    }
}

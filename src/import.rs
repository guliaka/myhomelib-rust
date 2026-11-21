//! Headless catalog import: scan a directory for FB2/FBD e-books, extract metadata via the Phase 2
//! engine, dedupe against the existing collection by source-file identity (folder + file name +
//! position in container — matching the original), and insert new books through the Phase 1 DAO.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use zip::ZipArchive;

use crate::core::Book;
use crate::db::Db;
use crate::error::{AppError, Result};
use crate::fb2::{fbd::Fbd, parse_fb2, Fb2};

/// Per-book outcome of an import.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportOutcome {
    /// A new book was inserted.
    Imported { book_id: i64 },
    /// The source file is already in the collection (skipped).
    Existing,
}

/// Aggregate result of an import run.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub imported: usize,
    pub existing: usize,
    /// Files that failed to process: path → error message.
    pub errors: Vec<(PathBuf, String)>,
}

/// Imports e-books into a collection rooted at `root` (used to compute relative folder paths).
pub struct Importer<'a> {
    db: &'a mut Db,
    root: PathBuf,
    /// When true, each book's FB2 `<genre>` code is also registered in the `Genres` catalog if missing.
    register_genres: bool,
    /// Parse failures of individual books *inside* a zip archive — they must not abort the whole
    /// archive scan, so they're collected here and folded into the report by `import_dir`.
    member_errors: Vec<(PathBuf, String)>,
}

impl<'a> Importer<'a> {
    pub fn new(db: &'a mut Db, root: impl AsRef<Path>) -> Self {
        Self {
            db,
            root: root.as_ref().to_path_buf(),
            register_genres: false,
            member_errors: Vec::new(),
        }
    }

    /// Also register each imported book's genre code in the `Genres` catalog (grow it from the FB2).
    pub fn with_register_genres(mut self, on: bool) -> Self {
        self.register_genres = on;
        self
    }

    /// Recursively import every `.fb2` / `.fbd` file — and every `.fb2` book inside every `.zip`
    /// archive — under `dir` (the original's folder import scans fb2.zip archives the same way).
    pub fn import_dir(&mut self, dir: &Path) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        for path in collect_ebook_files(dir)? {
            let outcomes = if is_zip(&path) {
                self.import_zip(&path)
            } else {
                self.import_file(&path)
            };
            match outcomes {
                Ok(outcomes) => {
                    for o in outcomes {
                        match o {
                            ImportOutcome::Imported { .. } => report.imported += 1,
                            ImportOutcome::Existing => report.existing += 1,
                        }
                    }
                }
                Err(e) => report.errors.push((path.clone(), e.to_string())),
            }
        }
        report.errors.append(&mut self.member_errors);
        Ok(report)
    }

    /// Import a single `.fb2` or `.fbd` file. Returns one outcome per book found inside it
    /// (a standalone FB2 yields one; an FBD may yield several).
    pub fn import_file(&mut self, path: &Path) -> Result<Vec<ImportOutcome>> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());
        match ext.as_deref() {
            Some("fb2") => {
                let data =
                    fs::read(path).map_err(|e| AppError::Other(format!("read {}: {e}", path.display())))?;
                let fb2 = parse_fb2(&data)?;
                let size = file_size(path);
                Ok(vec![self.insert_from_fb2(&fb2, path, 0, "fb2", size)?])
            }
            Some("fbd") => {
                let fbd = Fbd::open(path)
                    .map_err(|e| AppError::Other(format!("open fbd {}: {e}", path.display())))?;
                let size = file_size(path);
                let mut outcomes = Vec::new();
                for (i, book) in fbd.books.iter().enumerate() {
                    outcomes.push(self.insert_from_fb2(book, path, i as i32, "fbd", size)?);
                }
                Ok(outcomes)
            }
            _ => Err(AppError::Other(format!("unsupported file type: {}", path.display()))),
        }
    }

    /// Import every `.fb2` book inside a `.zip` archive — the classic `fb2-NNNNNN-NNNNNN.zip`
    /// collections shipped without an INPX index. The archive is the *source file*: `folder` is its
    /// path relative to the root, `file_name` its stem (without extension), and each book's
    /// `inside_no` is its position among the FB2 entries — the same identity scheme the INPX
    /// importer writes, so opening the book later can locate it in the archive.
    pub fn import_zip(&mut self, path: &Path) -> Result<Vec<ImportOutcome>> {
        let file =
            fs::File::open(path).map_err(|e| AppError::Other(format!("open {}: {e}", path.display())))?;
        let mut zip = ZipArchive::new(file)
            .map_err(|e| AppError::Other(format!("open zip {}: {e}", path.display())))?;
        let mut outcomes = Vec::new();
        let mut inside_no = 0i32;
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| AppError::Other(format!("zip entry {i} of {}: {e}", path.display())))?;
            let name = entry
                .name()
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| format!("entry-{i}"));
            if entry.is_dir() || !name.to_ascii_lowercase().ends_with(".fb2") {
                continue; // covers and other side files are not books
            }
            let size = entry.size();
            let mut data = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut data)
                .map_err(|e| AppError::Other(format!("read {name} in {}: {e}", path.display())))?;
            match parse_fb2(&data) {
                Ok(fb2) => {
                    outcomes.push(self.insert_from_fb2(&fb2, path, inside_no, "fb2", size)?);
                    inside_no += 1;
                }
                // A bad member must not kill the whole archive; report and keep scanning.
                Err(e) => {
                    self.member_errors.push((path.join(&name), e.to_string()));
                }
            }
        }
        Ok(outcomes)
    }

    /// Extract metadata from one FB2 document and insert it (or report it as already present).
    fn insert_from_fb2(
        &mut self,
        fb2: &Fb2,
        src: &Path,
        inside_no: i32,
        ext: &str,
        size: u64,
    ) -> Result<ImportOutcome> {
        let title = fb2
            .book_title()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| file_stem(src).map(|s| s.to_string()))
            .unwrap_or_else(|| "(untitled)".to_string());

        // Dedupe by source-file identity (folder + file name + position in container).
        let folder = relative_folder(&self.root, src);
        let file_name = file_stem(src).unwrap_or_default().to_string();
        if self.db.find_book_by_file(folder.as_deref(), &file_name, inside_no)?.is_some() {
            return Ok(ImportOutcome::Existing);
        }

        // Authors.
        let mut author_ids: Vec<i64> = Vec::new();
        for a in fb2.authors() {
            let last = a.last_name.clone().unwrap_or_default();
            let aid = self
                .db
                .upsert_author(&last, a.first_name.as_deref(), a.middle_name.as_deref())?;
            author_ids.push(aid);
        }

        // Series.
        let (series_id, seq_number) = match fb2.series() {
            Some(s) => match s.name {
                Some(name) if !name.trim().is_empty() => {
                    let sid = self.db.upsert_series(&name)?;
                    (Some(sid), s.number.map(|n| n as i32))
                }
                _ => (None, None),
            },
            None => (None, None),
        };

        // Genres: link the raw FB2 genre code(s) to the book.
        let genre_codes: Vec<String> = fb2
            .title_info()
            .map(|ti| {
                ti.children_elements()
                    .into_iter()
                    .filter(|e| e.name == "genre")
                    .map(|e| e.all_text())
                    .collect()
            })
            .unwrap_or_default();

        let book = Book {
            book_id: 0,
            lib_id: fb2
                .title_info()
                .and_then(|ti| ti.find("id"))
                .map(|e| e.all_text())
                .unwrap_or_default(),
            title,
            series_id,
            seq_number,
            update_date: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            lib_rate: 0,
            lang: fb2.lang(),
            folder,
            file_name,
            inside_no,
            ext: Some(ext.to_string()),
            book_size: Some(size as i64),
            is_local: true,
            is_deleted: false,
            key_words: None,
            rate: 0,
            progress: 0,
            annotation: fb2.annotation().filter(|a| !a.trim().is_empty()),
            review: None,
        };

        // Optionally grow the Genres catalog from this book's FB2 genre codes.
        if self.register_genres {
            for gc in &genre_codes {
                self.db.register_genre_code(gc)?;
            }
        }

        let genre_refs: Vec<&str> = genre_codes.iter().map(|s| s.as_str()).collect();
        let id = self.db.insert_book(&book, &author_ids, &genre_refs)?;
        Ok(ImportOutcome::Imported { book_id: id })
    }
}

/// Recursively collect `.fb2` / `.fbd` / `.zip` files under `dir`, sorted for determinism.
fn collect_ebook_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return Err(AppError::Other(format!("read dir {}: {e}", dir.display()))),
    };
    for entry in rd {
        let entry = entry.map_err(|e| AppError::Other(format!("dir entry: {e}")))?;
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if is_ebook(&p) {
            out.push(p);
        }
    }
    Ok(())
}

fn is_ebook(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("fb2") | Some("fbd") | Some("zip")
    )
}

fn is_zip(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref() == Some("zip")
}

/// File size in bytes (0 if unknown).
fn file_size(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// File name without extension.
fn file_stem(p: &Path) -> Option<&str> {
    p.file_stem().and_then(|s| s.to_str())
}

/// The folder containing `path`, relative to `root` (`None` if directly under root or not under it).
fn relative_folder(root: &Path, path: &Path) -> Option<String> {
    let parent = path.parent()?;
    match parent.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => Some(rel.to_string_lossy().into_owned()),
        _ => None,
    }
}

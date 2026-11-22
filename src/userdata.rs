//! User data (rate / progress / review per book + reading groups) import/export — the original's
//! `TUserData` (`unit_UserData.pas`) and `acImportUserData` / `acExportUserData`.
//!
//! Format (byte-exact with the original): a **zlib-compressed** XML document (the original saves
//! through `TMSXMLHelper.SaveToZFile` = `TZCompressionStream`):
//!
//! ```xml
//! <UserData>
//!   <Extras>
//!     <Book id="5" libid="lib-5" rate="4" progress="30">
//!       <Review><![CDATA[...]]></Review>
//!     </Book>
//!   </Extras>
//!   <Groups>
//!     <Group id="1" name="Избранное">
//!       <Book id="5" libid="lib-5"/>
//!     </Group>
//!   </Groups>
//! </UserData>
//! ```
//!
//! Books are keyed by `libid` (stable across installations — `BookID` is not), exactly like the
//! original. Where the data lives: rate/progress/review are per-collection columns
//! (`Books.Rate/Progress/Review`); groups live in the **system DB** (`Groups`/`BookGroups`,
//! keyed `BookID + DatabaseID`) — see `SystemDb`'s group API.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::db::Db;
use crate::error::{AppError, Result};
use crate::systemdb::SystemDb;

/// One book's user data: rating (0-5), reading progress (0-100) and free-text review.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookExtra {
    pub book_id: Option<i64>,
    pub libid: Option<String>,
    pub rate: i32,
    pub progress: i32,
    pub review: Option<String>,
}

/// One group: registry name + member books (by id and/or libid).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupData {
    pub group_id: Option<i64>,
    pub name: String,
    pub books: Vec<(Option<i64>, Option<String>)>,
}

/// The exchange document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserData {
    pub extras: Vec<BookExtra>,
    pub groups: Vec<GroupData>,
}

impl UserData {
    /// Parse the XML body (the original's `TUserData.Load`).
    pub fn from_xml(text: &str) -> Result<Self> {
        let mut data = UserData::default();
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(true);

        let mut section = Section::None;
        let mut current_group: Option<GroupData> = None;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let name = e.name().as_ref().to_string();
                    match name.as_str() {
                        "Extras" => section = Section::Extras,
                        "Groups" => section = Section::Groups,
                        "Book" => match section {
                            Section::Extras => {
                                let extra = parse_extra(&e, &mut reader)?;
                                data.extras.push(extra);
                            }
                            Section::Groups => {
                                let (id, libid) = book_id_attrs(&e);
                                if let Some(g) = current_group.as_mut() {
                                    g.books.push((id, libid));
                                }
                            }
                            Section::None => {}
                        },
                        "Group" => {
                            let g = GroupData {
                                group_id: attr(&e, "id").and_then(|v| v.parse().ok()),
                                name: attr(&e, "name").unwrap_or_default(),
                                books: Vec::new(),
                            };
                            current_group = Some(g);
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    match e.name().as_ref() {
                        "Extras" | "Groups" => section = Section::None,
                        "Group" => {
                            if let Some(g) = current_group.take() {
                                data.groups.push(g);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    // Self-closing elements: a group's `<Book id= libid=/>`, or an empty group.
                    match e.name().as_ref() {
                        "Book" => {
                            let (id, libid) = book_id_attrs(&e);
                            if let Some(g) = current_group.as_mut() {
                                g.books.push((id, libid));
                            }
                        }
                        "Group" => {
                            data.groups.push(GroupData {
                                group_id: attr(&e, "id").and_then(|v| v.parse().ok()),
                                name: attr(&e, "name").unwrap_or_default(),
                                books: Vec::new(),
                            });
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(AppError::msg(format!("XML parse error: {e}"))),
                _ => {}
            }
        }
        Ok(data)
    }

    /// Serialize as XML (the original's `TUserData.Save`, before zlib compression).
    pub fn to_xml(&self) -> String {
        let mut xml = String::from("<?xml version=\"1.0\"?>\n<UserData>\n");
        if !self.extras.is_empty() {
            xml.push_str("  <Extras>\n");
            for e in &self.extras {
                xml.push_str("    <Book");
                if let Some(id) = e.book_id {
                    xml.push_str(&format!(" id=\"{id}\""));
                }
                if let Some(l) = &e.libid {
                    xml.push_str(&format!(" libid=\"{l}\""));
                }
                if e.rate != 0 {
                    xml.push_str(&format!(" rate=\"{}\"", e.rate));
                }
                if e.progress != 0 {
                    xml.push_str(&format!(" progress=\"{}\"", e.progress));
                }
                xml.push_str(">");
                if let Some(r) = &e.review {
                    if !r.is_empty() {
                        // CDATA can't contain the literal `]]>` — split it across two CDATA
                        // sections (the standard XML workaround: `]]]]><![CDATA[>`).
                        let safe = r.replace("]]>", "]]]]><![CDATA[>");
                        xml.push_str(&format!("\n      <Review><![CDATA[{safe}]]></Review>\n    ",));
                    }
                }
                xml.push_str("</Book>\n");
            }
            xml.push_str("  </Extras>\n");
        }
        if !self.groups.is_empty() {
            xml.push_str("  <Groups>\n");
            for g in &self.groups {
                xml.push_str("    <Group");
                if let Some(id) = g.group_id {
                    xml.push_str(&format!(" id=\"{id}\""));
                }
                xml.push_str(&format!(" name=\"{}\"", escape(&g.name)));
                xml.push_str(">\n");
                for (id, libid) in &g.books {
                    xml.push_str("      <Book");
                    if let Some(id) = id {
                        xml.push_str(&format!(" id=\"{id}\""));
                    }
                    if let Some(l) = libid {
                        xml.push_str(&format!(" libid=\"{l}\""));
                    }
                    xml.push_str("/>\n");
                }
                xml.push_str("    </Group>\n");
            }
            xml.push_str("  </Groups>\n");
        }
        xml.push_str("</UserData>\n");
        xml
    }

    // -- file I/O (zlib-compressed, like the original) -------------------------------------

    /// Load from the original's zlib-compressed user-data file. Plain (uncompressed) XML is
    /// accepted too — the zlib header (0x78) is detected.
    pub fn load(path: &Path) -> Result<Self> {
        let mut raw = Vec::new();
        File::open(path)
            .map_err(|e| AppError::msg(format!("open {}: {e}", path.display())))?
            .read_to_end(&mut raw)?;
        let text = if raw.first() == Some(&0x78) {
            let mut text = String::new();
            flate2::read::ZlibDecoder::new(&raw[..])
                .read_to_string(&mut text)
                .map_err(|e| AppError::msg(format!("zlib: {e}")))?;
            text
        } else {
            String::from_utf8(raw).map_err(|e| AppError::msg(format!("utf8: {e}")))?
        };
        Self::from_xml(&text)
    }

    /// Save as the original's zlib-compressed user-data file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = File::create(path)
            .map_err(|e| AppError::msg(format!("create {}: {e}", path.display())))?;
        let mut z = flate2::write::ZlibEncoder::new(file, flate2::Compression::default());
        z.write_all(self.to_xml().as_bytes())
            .map_err(|e| AppError::msg(format!("write: {e}")))?;
        z.finish().map_err(|e| AppError::msg(format!("zlib: {e}")))?;
        Ok(())
    }

    // -- collection round-trips --------------------------------------------------------------

    /// Gather user data from a collection DB: every book with a rating, progress or review set
    /// (the original's `FCollection.ExportUserData`). Groups are gathered separately by the
    /// caller via the system DB.
    pub fn from_collection(db: &Db) -> Result<Self> {
        let mut data = UserData::default();
        let conn = db.connection();
        let mut stmt = conn.prepare(
            "SELECT BookID, LibID, Rate, Progress, Review FROM Books \
             WHERE Rate > 0 OR Progress > 0 OR Review IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            // Review is a BLOB in the schema, but tolerate TEXT (hand-edited or other tools).
            let raw: Option<rusqlite::types::Value> = r.get(4)?;
            let review = raw.map(|v| match v {
                rusqlite::types::Value::Blob(b) => String::from_utf8_lossy(&b).into_owned(),
                rusqlite::types::Value::Text(s) => s,
                _ => String::new(),
            });
            Ok(BookExtra {
                book_id: Some(r.get(0)?),
                libid: Some(r.get(1)?),
                rate: r.get(2)?,
                progress: r.get(3)?,
                review: review.filter(|s| !s.is_empty()),
            })
        })?;
        for row in rows {
            data.extras.push(row?);
        }
        Ok(data)
    }

    /// Apply extras to a collection: books are matched by `libid` (falling back to `BookID`),
    /// and only fields present in the file are written (the original merges the same way).
    pub fn apply_to_collection(&self, db: &mut Db) -> Result<(usize, usize)> {
        let conn = db.connection();
        let mut applied = 0usize;
        let mut missing = 0usize;
        for e in &self.extras {
            let book_id = match (&e.libid, e.book_id) {
                (Some(libid), _) if !libid.is_empty() => {
                    conn.query_row("SELECT BookID FROM Books WHERE LibID = ?1", [libid], |r| {
                        r.get::<_, i64>(0)
                    })
                    .ok()
                }
                (None, Some(id)) => {
                    conn.query_row("SELECT BookID FROM Books WHERE BookID = ?1", [id], |r| {
                        r.get::<_, i64>(0)
                    })
                    .ok()
                }
                _ => None,
            };
            match book_id {
                Some(id) => {
                    // Review is a BLOB column — write UTF-8 bytes, or NULL when absent.
                    let review: Option<Vec<u8>> =
                        e.review.as_deref().filter(|r| !r.is_empty()).map(|r| r.as_bytes().to_vec());
                    conn.execute(
                        "UPDATE Books SET Rate = ?2, Progress = ?3, Review = ?4 WHERE BookID = ?1",
                        rusqlite::params![id, e.rate, e.progress, review],
                    )?;
                    applied += 1;
                }
                None => missing += 1,
            }
        }
        Ok((applied, missing))
    }
}

// -- groups round-trips (system DB ↔ UserData) ----------------------------------------------

/// Gather this collection's groups from the system DB into the exchange document's group list.
/// `database_id` is the collection's `Bases.DatabaseID` (books are keyed `BookID + DatabaseID`).
pub fn export_groups_from_system(
    data: &mut UserData,
    sys: &SystemDb,
    database_id: i64,
) -> Result<()> {
    data.groups.clear();
    for (gid, gname, book_id, libid) in sys.list_group_books(database_id)? {
        match data.groups.iter_mut().find(|g| g.group_id == Some(gid)) {
            Some(g) => g.books.push((Some(book_id), Some(libid))),
            None => data.groups.push(GroupData {
                group_id: Some(gid),
                name: gname,
                books: vec![(Some(book_id), Some(libid))],
            }),
        }
    }
    Ok(())
}

/// Import groups from the exchange document into the system DB, resolving each member book by
/// `libid` against the open collection. Returns `(linked, unresolved)`.
pub fn import_groups_to_system(
    groups: &[GroupData],
    sys: &SystemDb,
    db: &Db,
    database_id: i64,
) -> Result<(usize, usize)> {
    let conn = db.connection();
    let mut linked = 0usize;
    let mut unresolved = 0usize;
    for g in groups {
        if g.name.trim().is_empty() {
            continue;
        }
        let group_id = sys.upsert_group(g.name.trim())?;
        for (book_id, libid) in &g.books {
            // Resolve the collection-local BookID by libid, falling back to the exported id.
            let resolved: Option<(i64, String, String)> = match libid {
                Some(l) if !l.is_empty() => conn
                    .query_row(
                        "SELECT BookID, LibID, Title FROM Books WHERE LibID = ?1",
                        [l],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok(),
                None => match book_id {
                    Some(id) => conn
                        .query_row(
                            "SELECT BookID, LibID, Title FROM Books WHERE BookID = ?1",
                            [id],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .ok(),
                    None => None,
                },
                _ => None,
            };
            match resolved {
                Some((id, lib, title)) => {
                    sys.add_group_book(group_id, id, database_id, &lib, &title)?;
                    linked += 1;
                }
                None => unresolved += 1,
            }
        }
    }
    Ok((linked, unresolved))
}

#[derive(PartialEq, Clone, Copy)]
enum Section {
    None,
    Extras,
    Groups,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `id`/`libid` attribute pair off a `<Book>` element.
fn book_id_attrs(e: &quick_xml::events::BytesStart<'_>) -> (Option<i64>, Option<String>) {
    (
        attr(e, "id").and_then(|v| v.parse().ok()),
        attr(e, "libid"),
    )
}

/// Parse an Extras `<Book>`: attributes + a `<Review>` text (CDATA or plain) child.
fn parse_extra(
    e: &quick_xml::events::BytesStart<'_>,
    reader: &mut Reader<&[u8]>,
) -> Result<BookExtra> {
    let (id, libid) = book_id_attrs(e);
    let mut extra = BookExtra {
        book_id: id,
        libid,
        rate: attr(e, "rate").and_then(|v| v.parse().ok()).unwrap_or(0),
        progress: attr(e, "progress").and_then(|v| v.parse().ok()).unwrap_or(0),
        review: None,
    };
    // Read until </Book>, capturing the <Review> text (CDATA or plain).
    let mut in_review = false;
    let mut review = String::new();
    let mut raw = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(t)) if t.name().as_ref() == "Review" => in_review = true,
            Ok(Event::CData(t)) if in_review => {
                raw.push_str(t.as_ref());
            }
            Ok(Event::Text(t)) if in_review => {
                raw.push_str(t.as_ref());
            }
            Ok(Event::End(t)) => {
                if t.name().as_ref() == "Review" {
                    in_review = false;
                    review = std::mem::take(&mut raw);
                } else if t.name().as_ref() == "Book" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(AppError::msg(format!("XML parse error: {err}"))),
            _ => {}
        }
    }
    if !review.trim().is_empty() {
        extra.review = Some(review);
    }
    Ok(extra)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref().eq_ignore_ascii_case(name))
        .map(|a| a.value.clone().into_owned())
}

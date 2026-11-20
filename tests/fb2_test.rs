//! Phase 2 acceptance: FB2 parse/serialize round-trip is lossless, typed extraction of
//! title/authors/genre/annotation/cover works on a realistic fixture (Cyrillic, nested sections,
//! inline markup, base64 cover), and the FBD (ZIP) container round-trips.

use base64::Engine;
use myhomelib::fb2::{parse_fb2, serialize};

const COVER: &[u8] = b"\x89PNG-fake-cover-bytes";

fn sample_fb2(cover_b64: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2" xmlns:xlink="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <id>abc-123</id>
      <genre>prose_classic</genre>
      <author><last-name>Толстой</last-name><first-name>Лев</first-name><middle-name>Николаевич</middle-name></author>
      <book-title>Война и мир</book-title>
      <lang>ru</lang>
      <year>1869</year><month>01</month><day>01</day>
      <coverpage><image xlink:href="#cover"/></coverpage>
      <annotation>
        <p>Роман о войне.</p>
        <p>И мире.</p>
      </annotation>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Часть первая</p></title>
      <p>Живёт человек на свете, и вот однажды он узнал, что <a href="http://example.com">смысл жизни</a> прост.</p>
      <empty-line/>
      <section>
        <p>Вложенный раздел.</p>
      </section>
    </section>
  </body>
  <binary id="cover">{cover_b64}</binary>
</FictionBook>"##
    )
}

fn b64() -> String {
    base64::engine::general_purpose::STANDARD.encode(COVER)
}

#[test]
fn extract_title_authors_genre_lang() {
    let fb2 = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    assert_eq!(fb2.book_title().as_deref(), Some("Война и мир"));
    assert_eq!(fb2.lang().as_deref(), Some("ru"));
    assert_eq!(fb2.genre().as_deref(), Some("prose_classic"));

    let authors = fb2.authors();
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].last_name.as_deref(), Some("Толстой"));
    assert_eq!(authors[0].first_name.as_deref(), Some("Лев"));
    assert_eq!(authors[0].middle_name.as_deref(), Some("Николаевич"));
}

#[test]
fn extract_annotation() {
    let fb2 = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    let ann = fb2.annotation().expect("annotation present");
    assert!(ann.contains("Роман о войне."), "missing p1: {ann}");
    assert!(ann.contains("И мире."), "missing p2: {ann}");
}

#[test]
fn extract_cover() {
    let fb2 = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    assert_eq!(fb2.cover().as_deref(), Some(COVER));
}

#[test]
fn body_has_one_top_level_section() {
    let fb2 = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    assert_eq!(fb2.body_sections().len(), 1);
}

#[test]
fn round_trip_is_lossless() {
    let a = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    let bytes = serialize(&a);
    let b = parse_fb2(&bytes).unwrap();
    assert_eq!(a.root, b.root, "parse→serialize→parse changed the tree");
}

#[test]
fn fbd_round_trip() {
    use myhomelib::fb2::fbd::Fbd;
    let dir = std::env::temp_dir().join(format!("mhl_fbd_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.fbd");

    let book = parse_fb2(sample_fb2(&b64()).as_bytes()).unwrap();
    Fbd::write(&path, &book, Some(COVER)).unwrap();

    let fbd = Fbd::open(&path).unwrap();
    assert_eq!(fbd.books.len(), 1, "expected one fb2 in archive");
    assert_eq!(fbd.books[0].root, book.root, "fb2 inside fbd changed");
    assert!(
        fbd.attachments
            .iter()
            .any(|(n, c)| n.to_lowercase().ends_with(".png") && c.as_slice() == COVER),
        "cover attachment missing"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

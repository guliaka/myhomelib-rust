//! Phase 1 smoke CLI: proves the data layer can initialize a collection, seed it, query and
//! paginate books, and report stats. Against a *real* user MyHomeLib `.db`, `books`/`stats` work
//! unchanged (the schema is identical).

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};

fn main() {
    init_tracing();
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("init") => cmd_init(&args[1..]),
        Some("books") => cmd_books(&args[1..]),
        Some("stats") => cmd_stats(&args[1..]),
        _ => {
            print_usage();
            Ok(())
        }
    }
}

fn cmd_init(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.first().ok_or("usage: myhomelib init <db-path> [--demo]")?;
    let demo = args.iter().any(|a| a == "--demo");
    let mut db = Db::open(path)?;
    if demo {
        seed_demo(&mut db)?;
    }
    println!("initialized collection at {}", path);
    Ok(())
}

fn cmd_books(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut query = None;
    let mut author = None;
    let mut limit = 20i64;
    let mut offset = 0i64;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--query" => {
                i += 1;
                query = args.get(i).cloned();
            }
            "--author" => {
                i += 1;
                author = args.get(i).cloned();
            }
            "--limit" => {
                i += 1;
                limit = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(20);
            }
            "--offset" => {
                i += 1;
                offset = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            other => path = Some(other.to_string()),
        }
        i += 1;
    }

    let path = path.ok_or("usage: myhomelib books <db-path> [--query Q] [--author A] [--limit N] [--offset M]")?;
    let db = Db::open(&path)?;
    let q = BookQuery {
        title_like: query,
        author_name_like: author,
        ..Default::default()
    };

    let total = db.count_books(&q)?;
    let books = db.search_books(&q, limit, offset)?;
    println!("{} book(s) matching — showing {} from offset {}", total, books.len(), offset);
    for b in &books {
        let authors: Vec<String> = db.book_authors(b.book_id)?.iter().map(|a| a.last_name.clone()).collect();
        let author_str = if authors.is_empty() { "?".to_string() } else { authors.join(", ") };
        let lang = b.lang.as_deref().map(|l| format!(" [{}] ", l)).unwrap_or_default();
        let progress = if b.progress > 0 { format!(" ({}%)", b.progress) } else { String::new() };
        println!(
            "  [{}] {} — {}{}{}",
            b.book_id, author_str, b.title, lang, progress
        );
    }
    Ok(())
}

fn cmd_stats(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.first().ok_or("usage: myhomelib stats <db-path>")?;
    let db = Db::open(path)?;
    let s = db.stats()?;
    println!("books={} authors={} series={} genres={}", s.books, s.authors, s.series, s.genres);
    Ok(())
}

fn print_usage() {
    eprintln!(
        "myhomelib — Phase 1 data-layer smoke CLI\n\
         \n\
         usage:\n\
         \x20 myhomelib init  <db-path> [--demo]\n\
         \x20 myhomelib books <db-path> [--query Q] [--author A] [--limit N] [--offset M]\n\
         \x20 myhomelib stats <db-path>"
    );
}

/// Build a Book with sensible defaults for the demo/seed path.
fn demo_book(title: &str, lib_id: &str, lang: &str, progress: i32) -> Book {
    Book {
        book_id: 0,
        lib_id: lib_id.to_string(),
        title: title.to_string(),
        series_id: None,
        seq_number: None,
        update_date: "2026-11-20 12:00:00".to_string(),
        lib_rate: 0,
        lang: Some(lang.to_string()),
        folder: None,
        file_name: format!("{title}.fb2"),
        inside_no: 0,
        ext: Some("fb2".to_string()),
        book_size: Some(1_000_000),
        is_local: true,
        is_deleted: false,
        key_words: None,
        rate: 0,
        progress,
        annotation: None,
        review: None,
    }
}

/// Seed a small, deterministic demo dataset (including Cyrillic) so the smoke CLI has data to show.
fn seed_demo(db: &mut Db) -> Result<(), Box<dyn std::error::Error>> {
    let a_tolstoy = db.upsert_author("Толстой", Some("Лев"), Some("Николаевич"))?;
    let a_dostoevsky = db.upsert_author("Достоевский", Some("Фёдор"), Some("Михайлович"))?;
    let a_orwell = db.upsert_author("Orwell", Some("George"), None)?;

    let s_war = db.upsert_series("Война и мир")?;
    let s_crime = db.upsert_series("Преступление и наказание")?;

    db.upsert_genre("prose.classic", None, Some("classic"), "Классическая проза")?;

    let mut b1 = demo_book("Война и мир", "demo-1", "ru", 42);
    b1.series_id = Some(s_war);
    b1.seq_number = Some(1);
    db.insert_book(&b1, &[a_tolstoy], &["prose.classic"])?;

    let mut b2 = demo_book("Анна Каренина", "demo-2", "ru", 0);
    b2.series_id = None;
    db.insert_book(&b2, &[a_tolstoy], &["prose.classic"])?;

    let mut b3 = demo_book("Преступление и наказание", "demo-3", "ru", 10);
    b3.series_id = Some(s_crime);
    b3.seq_number = Some(1);
    db.insert_book(&b3, &[a_dostoevsky], &["prose.classic"])?;

    let b4 = demo_book("1984", "demo-4", "en", 0);
    db.insert_book(&b4, &[a_orwell], &[])?;

    println!("seeded demo data: 3 authors, 2 series, 4 books");
    Ok(())
}

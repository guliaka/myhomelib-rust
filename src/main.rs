//! Phase 1 smoke CLI: proves the data layer can initialize a collection, seed it, query and
//! paginate books, and report stats. Against a *real* user MyHomeLib `.db`, `books`/`stats` work
//! unchanged (the schema is identical).

use std::path::Path;

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};
use myhomelib::export::Exporter;
use myhomelib::db::props;
use myhomelib::import::Importer;
use myhomelib::inpx::{import_inpx, InpxOptions};
use myhomelib::jobs::{Job, Severity};
use myhomelib::sysdb::SystemDb;

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
        Some("import") => cmd_import(&args[1..]),
        Some("genres") => cmd_genres(&args[1..]),
        Some("export") => cmd_export(&args[1..]),
        Some("import-inpx") => cmd_import_inpx(&args[1..]),
        Some("collections") => cmd_collections(&args[1..]),
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

/// Folder import now also scans `.zip` archives of FB2 books (the no-INPX `fb2-…zip` collections).
fn cmd_import(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut dir = None;
    let mut register_genres = false;
    for a in args {
        match a.as_str() {
            "--register-genres" => register_genres = true,
            other => {
                if path.is_none() {
                    path = Some(other.to_string());
                } else if dir.is_none() {
                    dir = Some(other.to_string());
                }
            }
        }
    }
    let path = path.ok_or("usage: myhomelib import <db-path> <dir> [--register-genres]")?;
    let dir = dir.ok_or("usage: myhomelib import <db-path> <dir> [--register-genres]")?;
    let mut db = Db::open(&path)?;
    let mut importer = Importer::new(&mut db, &dir).with_register_genres(register_genres);
    let report = importer.import_dir(Path::new(&dir))?;
    println!(
        "imported={} existing={} errors={}{}",
        report.imported,
        report.existing,
        report.errors.len(),
        if register_genres { " (genres registered)" } else { "" }
    );
    for (p, e) in &report.errors {
        eprintln!("  error: {} — {e}", p.display());
    }
    Ok(())
}

fn cmd_genres(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut path = None;
    let mut seed = false;
    let mut file = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => seed = true,
            "--file" => {
                i += 1;
                file = args.get(i).cloned();
            }
            other => path = Some(other.to_string()),
        }
        i += 1;
    }
    let path = path.ok_or("usage: myhomelib genres <db-path> [--seed | --file <path>]")?;
    let mut db = Db::open(&path)?;

    if let Some(f) = file {
        let text = std::fs::read_to_string(&f).map_err(|e| format!("read {f}: {e}"))?;
        let rows = myhomelib::genres::parse_genre_lines(&text);
        let n = db.replace_genres(&rows)?;
        println!("replaced genre catalog from {f}: {n} genres");
    } else if seed {
        let n = db.seed_builtin_genres()?;
        println!("seeded built-in genre list: {n} genres");
    } else {
        let genres = db.list_genres()?;
        println!("{} genre(s) in catalog:", genres.len());
        for g in &genres {
            let fb2 = g.fb2_code.as_deref().unwrap_or("-");
            println!("  {}  [{}]  {}", g.genre_code, fb2, g.genre_alias);
        }
    }
    Ok(())
}

fn cmd_export(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.first().ok_or("usage: myhomelib export <db-path> <book-id> <out.fb2>")?;
    let id: i64 = args.get(1).ok_or("missing <book-id>")?.parse()?;
    let out = args.get(2).ok_or("missing <out.fb2>")?;
    let db = Db::open(path)?;
    let exporter = Exporter::new(&db);
    let bytes = exporter.export_book(id, Path::new(out))?;
    println!("exported book {id} to {out} ({} bytes)", bytes.len());
    Ok(())
}

/// Build a local collection from an INPX index. Progress goes to stderr.
fn cmd_import_inpx(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    const USAGE: &str = "usage: myhomelib import-inpx <db-path> <file.inpx> [--root <books-dir>]";
    let mut pos: Vec<&String> = Vec::new();
    let mut root = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" {
            i += 1;
            root = args.get(i).map(std::path::PathBuf::from);
        } else {
            pos.push(&args[i]);
        }
        i += 1;
    }
    let (db_path, inpx) = match pos.as_slice() {
        [d, f] => (d.to_string(), std::path::PathBuf::from(f)),
        _ => return Err(USAGE.into()),
    };
    let started = std::time::Instant::now();
    let mut job = Job::spawn("import-inpx", move |p| {
        let db = Db::open(&db_path)?;
        let opts = InpxOptions { root, ..Default::default() };
        Ok(import_inpx(&db, &inpx, &opts, p)?.summary())
    });
    let mut last = -1i64;
    while !job.is_done() {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let pct = job.progress.percent() as i64;
        if pct != last {
            last = pct;
            eprint!("\r{pct:3}%  {}        ", job.progress.comment());
        }
    }
    job.join();
    eprintln!();
    let errors = job.progress.error_count();
    for l in job.progress.log_lines().iter().filter(|l| l.severity == Severity::Error).take(20) {
        eprintln!("  error: {}", l.text);
    }
    match job.progress.result() {
        Some(Ok(summary)) => {
            println!("{summary} — {:.1} s, ошибок: {errors}", started.elapsed().as_secs_f64());
            Ok(())
        }
        Some(Err(e)) => Err(e.into()),
        None => Err("import job ended without a result".into()),
    }
}

/// Manage the collection registry in a system database (`user.dbs2`).
fn cmd_collections(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    const USAGE: &str = "usage: myhomelib collections <user.dbs2> [list]\n\
       \x20      myhomelib collections <user.dbs2> create <name> <db-file> <books-dir> [--inpx <file.inpx>]\n\
       \x20      myhomelib collections <user.dbs2> register <name> <db-file> <books-dir>\n\
       \x20      myhomelib collections <user.dbs2> delete <id> [--remove-file]";
    let sys_path = args.first().ok_or(USAGE)?;
    let sys = SystemDb::open(sys_path)?;
    match args.get(1).map(String::as_str) {
        None | Some("list") => {
            let all = sys.collections()?;
            println!("{} collection(s):", all.len());
            for c in &all {
                println!(
                    "  [{}] {} — {}\n        db: {}\n        books: {}",
                    c.id,
                    c.name,
                    props::code_label(c.code),
                    c.db_file.display(),
                    c.root_folder.display()
                );
            }
            Ok(())
        }
        Some("create") => {
            let name = args.get(2).ok_or(USAGE)?;
            let file = Path::new(args.get(3).ok_or(USAGE)?);
            let root = Path::new(args.get(4).ok_or(USAGE)?);
            let inpx = match args.get(5).map(String::as_str) {
                Some("--inpx") => Some(args.get(6).ok_or(USAGE)?.clone()),
                Some(_) => return Err(USAGE.into()),
                None => None,
            };
            // An INPX-backed collection is an external local library unless its header says otherwise.
            let code = match &inpx {
                Some(f) => myhomelib::inpx::read_header(Path::new(f))?
                    .map(|h| h.content_type)
                    .filter(|c| *c != props::CT_PRIVATE_FB)
                    .unwrap_or(props::CT_EXTERNAL_LOCAL_FB),
                None => props::CT_PRIVATE_FB,
            };
            let id = sys.create_collection(name, root, file, code, None)?;
            println!("created collection [{id}] {name} ({})", props::code_label(code));
            if let Some(f) = inpx {
                cmd_import_inpx(&[file.to_string_lossy().into_owned(), f, "--root".into(), root.to_string_lossy().into_owned()])?;
                let db = Db::open(file)?;
                sys.set_data_version(id, db.get_prop_int(props::PROP_DATAVERSION)?)?;
            }
            Ok(())
        }
        Some("register") => {
            let name = args.get(2).ok_or(USAGE)?;
            let file = Path::new(args.get(3).ok_or(USAGE)?);
            let root = Path::new(args.get(4).ok_or(USAGE)?);
            let id = sys.register_collection(file, name, root)?;
            println!("registered collection [{id}] {name}");
            Ok(())
        }
        Some("delete") => {
            let id: i64 = args.get(2).ok_or(USAGE)?.parse()?;
            let remove = args.iter().any(|a| a == "--remove-file");
            if sys.collection(id)?.is_none() {
                return Err(format!("no collection with id {id}").into());
            }
            sys.delete_collection(id, remove)?;
            println!("deleted collection [{id}]{}", if remove { " and its database file" } else { "" });
            Ok(())
        }
        Some(_) => Err(USAGE.into()),
    }
}

fn print_usage() {
    eprintln!(
        "myhomelib — data-layer + FB2/FBD CLI\n\
         \n\
         usage:\n\
         \x20 myhomelib init    <db-path> [--demo]\n\
         \x20 myhomelib books   <db-path> [--query Q] [--author A] [--limit N] [--offset M]\n\
         \x20 myhomelib stats   <db-path>\n\
         \x20 myhomelib import  <db-path> <dir> [--register-genres]  (scans .fb2/.fbd/.zip)\n\
         \x20 myhomelib genres  <db-path> [--seed | --file <path>]\n\
         \x20 myhomelib export  <db-path> <book-id> <out.fb2>\n\
         \x20 myhomelib import-inpx <db-path> <file.inpx> [--root <books-dir>]\n\
         \x20 myhomelib collections <user.dbs2> [list | create … | register … | delete …]"
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

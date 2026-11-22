//! Phase 1 smoke CLI: proves the data layer can initialize a collection, seed it, query and
//! paginate books, and report stats. Against a *real* user MyHomeLib `.db`, `books`/`stats` work
//! unchanged (the schema is identical).

use std::path::Path;

use myhomelib::core::Book;
use myhomelib::db::{BookQuery, Db};
use myhomelib::export::Exporter;
use myhomelib::import::Importer;

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
        Some("collections") => cmd_collections(&args[1..]),
        Some("maintenance") => cmd_maintenance(&args[1..]),
        Some("inpx") => cmd_inpx(&args[1..]),
        Some("userdata") => cmd_userdata(&args[1..]),
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

/// `myhomelib collections <system-db> <subcmd> …` — manage the collection registry
/// (list / add / remove), mirroring the GUI's collection manager.
fn cmd_collections(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use myhomelib::systemdb::{CollectionInfo, SystemDb};
    let path = args.first().ok_or("usage: myhomelib collections <system-db> list|add|remove …")?.clone();
    let cmd = args.get(1).map(String::as_str).ok_or("usage: myhomelib collections <system-db> list|add|remove …")?;
    let sys = SystemDb::open(&path)?;

    match cmd {
        "list" => {
            let rows = sys.list()?;
            if rows.is_empty() {
                println!("(no collections registered)");
            }
            for c in &rows {
                println!(
                    "[{}] {}\n    db: {}{}\n    type: {}, created {}",
                    c.id,
                    c.name,
                    c.db_file_name,
                    if c.file_exists() { "" } else { "  (FILE MISSING)" },
                    if c.is_online() { "online" } else { "local" },
                    c.creation_date
                );
            }
        }
        "add" => {
            // collections <sys> add <name> <db-file> [--root DIR] [--notes N] [--online]
            let name = args.get(2).cloned().ok_or("missing <name>")?;
            let db_file = args.get(3).cloned().ok_or("missing <db-file>")?;
            let mut info = CollectionInfo {
                name,
                db_file_name: db_file.clone(),
                ..Default::default()
            };
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--root" => {
                        i += 1;
                        info.root_folder = args.get(i).cloned().unwrap_or_default();
                    }
                    "--notes" => {
                        i += 1;
                        info.notes = args.get(i).cloned().unwrap_or_default();
                    }
                    "--online" => info.code = 0x0801_0000,
                    other => return Err(format!("unknown option {other}").into()),
                }
                i += 1;
            }
            // Create the collection file if it doesn't exist yet (the wizard's behavior).
            myhomelib::db::Db::open(&info.db_file_name)?;
            let id = sys.add(&info)?;
            println!("registered collection #{id}: {} ({})", info.name, info.db_file_name);
        }
        "remove" => {
            // collections <sys> remove <id> [--purge]
            let id: i64 = args.get(2).ok_or("missing <id>")?.parse()?;
            let purge = args.iter().any(|a| a == "--purge");
            let file = sys.get(id)?.map(|c| c.db_file_name);
            sys.delete(id)?;
            if purge {
                if let Some(f) = &file {
                    SystemDb::purge_files(f)?;
                    println!("removed collection #{id} and deleted {f}");
                }
            } else {
                println!("removed collection #{id} from registry (files kept)");
            }
        }
        other => return Err(format!("unknown collections subcommand {other}").into()),
    }
    Ok(())
}

/// `myhomelib maintenance <db-path> check|compact|repair` — collection upkeep (the GUI menu actions).
fn cmd_maintenance(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.first().ok_or("usage: myhomelib maintenance <db-path> check|compact|repair")?;
    let cmd = args.get(1).map(String::as_str).ok_or("missing subcommand: check|compact|repair")?;
    let mut db = myhomelib::db::Db::open(path)?;
    match cmd {
        "check" => {
            db.integrity_check()?;
            println!("integrity check passed");
        }
        "compact" => {
            let before = db.file_size_bytes();
            db.compact()?;
            println!(
                "vacuumed: {} -> {} bytes",
                before.unwrap_or(0),
                db.file_size_bytes().unwrap_or(0)
            );
        }
        "repair" => {
            let n = db.repair()?;
            println!("integrity ok, search columns rebuilt for {n} row(s)");
        }
        other => return Err(format!("unknown maintenance subcommand {other}").into()),
    }
    Ok(())
}

/// `myhomelib inpx import <db> <file.inpx>` / `myhomelib inpx export <db> <out.inpx> [--system sys.db]`.
fn cmd_inpx(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use myhomelib::inpx::{export_inpx, import_inpx, InpxHeader};
    let cmd = args.first().ok_or("usage: myhomelib inpx import|export <db-path> <file>")?;
    let db_path = args.get(1).ok_or("missing <db-path>")?;
    let file = args.get(2).ok_or("missing <file>")?;
    match cmd.as_str() {
        "import" => {
            let mut db = myhomelib::db::Db::open(db_path)?;
            let report = import_inpx(&mut db, Path::new(file))?;
            println!(
                "INPX import: {} imported, {} already present, {} error(s)",
                report.imported,
                report.existing,
                report.errors.len()
            );
            for (f, e) in report.errors.iter().take(20) {
                eprintln!("  {f}: {e}");
            }
        }
        "export" => {
            let db = myhomelib::db::Db::open(db_path)?;
            // Registry fields for collection.info: from the system DB when the collection is
            // registered, else from the file name alone.
            let sys = args
                .iter()
                .position(|a| a == "--system")
                .and_then(|p| args.get(p + 1))
                .map(|s| myhomelib::systemdb::SystemDb::open(s))
                .transpose()?;
            let header = match sys {
                Some(sys) => {
                    let current = Path::new(db_path).canonicalize()?.to_string_lossy().to_string();
                    sys.list()?
                        .into_iter()
                        .find(|c| {
                            std::path::Path::new(&c.db_file_name)
                                .canonicalize()
                                .map(|p| p.to_string_lossy().to_string() == current)
                                .unwrap_or(false)
                        })
                        .map(|c| InpxHeader {
                            name: c.name,
                            file_name: c.db_file_name,
                            content_type: c.code,
                            notes: c.notes,
                            url: c.url,
                            script: c.connection_script,
                        })
                        .unwrap_or_default()
                }
                None => InpxHeader {
                    name: Path::new(db_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    ..Default::default()
                },
            };
            let n = export_inpx(&db, Path::new(file), &header, "0")?;
            println!("exported {n} book(s) to {file}");
        }
        other => return Err(format!("unknown inpx subcommand {other}").into()),
    }
    Ok(())
}

/// `myhomelib userdata export|import <db> <file> [--system sys.db]` — the original's
/// acExportUserData / acImportUserData (rate/progress/review + groups, zlib-compressed XML).
fn cmd_userdata(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use myhomelib::userdata::{
        export_groups_from_system, import_groups_to_system, UserData,
    };
    let cmd = args.first().ok_or("usage: myhomelib userdata export|import <db-path> <file> [--system <sys.db>]")?;
    let db_path = args.get(1).ok_or("missing <db-path>")?;
    let file = args.get(2).ok_or("missing <file>")?;
    let sys_path = args
        .iter()
        .position(|a| a == "--system")
        .and_then(|p| args.get(p + 1));
    // The collection's DatabaseID in the registry (needed for groups); 0 = unregistered.
    let mut database_id = 0i64;

    match cmd.as_str() {
        "export" => {
            let db = myhomelib::db::Db::open(db_path)?;
            let mut data = UserData::from_collection(&db)?;
            if let Some(sp) = sys_path {
                let sys = myhomelib::systemdb::SystemDb::open(sp)?;
                let current = Path::new(db_path).canonicalize()?.to_string_lossy().to_string();
                if let Some(c) = sys.list()?.into_iter().find(|c| {
                    std::path::Path::new(&c.db_file_name)
                        .canonicalize()
                        .map(|p| p.to_string_lossy().to_string() == current)
                        .unwrap_or(false)
                }) {
                    database_id = c.id;
                    export_groups_from_system(&mut data, &sys, database_id)?;
                }
            }
            data.save(Path::new(file))?;
            println!(
                "user data exported to {file}: {} extra(s), {} group(s)",
                data.extras.len(),
                data.groups.len()
            );
        }
        "import" => {
            let mut db = myhomelib::db::Db::open(db_path)?;
            let data = UserData::load(Path::new(file))?;
            let (applied, missing) = data.apply_to_collection(&mut db)?;
            let mut group_msg = String::new();
            if let Some(sp) = sys_path {
                let sys = myhomelib::systemdb::SystemDb::open(sp)?;
                let current = Path::new(db_path).canonicalize()?.to_string_lossy().to_string();
                if let Some(c) = sys.list()?.into_iter().find(|c| {
                    std::path::Path::new(&c.db_file_name)
                        .canonicalize()
                        .map(|p| p.to_string_lossy().to_string() == current)
                        .unwrap_or(false)
                }) {
                    database_id = c.id;
                }
                if database_id > 0 {
                    let (linked, unresolved) =
                        import_groups_to_system(&data.groups, &sys, &db, database_id)?;
                    group_msg = format!(", {linked} group link(s) ({unresolved} unresolved)");
                }
            }
            println!(
                "user data imported: {applied} book(s) updated, {missing} not found{group_msg}"
            );
        }
        other => return Err(format!("unknown userdata subcommand {other}").into()),
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "myhomelib — data-layer + FB2/FBD CLI\n\
         \n\
         usage:\n\
         \x20 myhomelib init    <db-path> [--demo]\n\
         \x20 myhomelib books   <db-path> [--query Q] [--author A] [--limit N] [--offset M]\n\
         \x20 myhomelib stats   <db-path>\n\
         \x20 myhomelib import  <db-path> <dir> [--register-genres]\n\
         \x20 myhomelib genres  <db-path> [--seed | --file <path>]\n\
         \x20 myhomelib export  <db-path> <book-id> <out.fb2>\n\
         \x20 myhomelib collections <system-db> list | add <name> <db-file> [--root DIR] [--online] | remove <id> [--purge]\n\
         \x20 myhomelib maintenance <db-path> check | compact | repair\n\
         \x20 myhomelib inpx import|export <db-path> <file.inpx> [--system <sys.db>]\n\
         \x20 myhomelib userdata export|import <db-path> <file> [--system <sys.db>]"
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

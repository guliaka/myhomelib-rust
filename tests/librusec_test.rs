//! Phase 6 tests: connection-script parsing, INPX catalog parse/search, download-queue state
//! machine, and the downloader executed against a mock transport (no network).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use myhomelib::error::{AppError, Result};
use myhomelib::librusec::catalog::parse_inpx;
use myhomelib::librusec::client::{Downloader, Response, Transport};
use myhomelib::librusec::command::{parse_commands, Cmd};
use myhomelib::librusec::queue::{DownloadQueue, ItemStatus};

// ---------- command ----------

#[test]
fn parse_commands_substitutes_macros_and_keeps_resurl() {
    let macros = [
        ("%USER%".to_string(), "alice".to_string()),
        ("%PASS%".to_string(), "secret".to_string()),
        ("%URL%".to_string(), "http://lib.rus.ec".to_string()),
    ];
    let script = "GET %URL%/login?u=%USER%\nPAUSE 100\nADD key abc\nPOST %URL%/go\nCHECK\n\nBOGUS line here";
    let cmds = parse_commands(script, &macros);

    assert_eq!(cmds[0], Cmd::Get("http://lib.rus.ec/login?u=alice".into()));
    assert_eq!(cmds[1], Cmd::Pause(100));
    assert_eq!(cmds[2], Cmd::Add("key".into(), "abc".into()));
    assert_eq!(cmds[3], Cmd::Post("http://lib.rus.ec/go".into()));
    assert_eq!(cmds[4], Cmd::Check);
    assert_eq!(cmds[5], Cmd::Skip, "blank line is a no-op");
    assert_eq!(cmds[6], Cmd::Skip, "unknown command is a no-op");

    // %RESURL% is not in the macro table, so it survives parsing for the executor to fill in.
    let c2 = parse_commands("GET http://x/a\nGET %RESURL%", &macros);
    assert_eq!(c2[1], Cmd::Get("%RESURL%".into()));
}

// ---------- catalog ----------

#[test]
fn parse_inpx_records_and_search() {
    let f = "\u{04}"; // FIELD_DELIM
    let rec1 = ["Толстой,Лев", "prose_classic", "Война и мир", "", "lib001", "ru", "fb2", "tw/voyna.fb2.zip"].join(f);
    let rec2 = ["Достоевский,Фёдор", "sf", "Преступление и наказание", "", "lib002", "ru", "fb2", "ds/pn.fb2.zip"].join(f);
    let inpx = format!(
        "INPX;1.0\nStructure: AUTHOR;GENRE;TITLE;SERIES;LIBID;LANG;EXT;URL\nBase URL:http://librusec.ru/download/\n\n{rec1}\n{rec2}"
    );

    let cat = parse_inpx(&inpx);
    assert_eq!(cat.len(), 2);
    assert_eq!(cat.base_url.as_deref(), Some("http://librusec.ru/download/"));

    let b0 = &cat.books[0];
    assert_eq!(b0.title, "Война и мир");
    assert_eq!(b0.authors, vec![("Толстой".into(), "Лев".into(), String::new())]);
    assert_eq!(b0.genres, vec!["prose_classic".to_string()]);
    assert_eq!(b0.libid.as_deref(), Some("lib001"));
    assert_eq!(b0.lang.as_deref(), Some("ru"));
    assert_eq!(b0.url.as_deref(), Some("tw/voyna.fb2.zip"));

    // Relative URL resolves onto the base; an absolute one would pass through unchanged.
    assert_eq!(cat.resolve_url(b0).as_deref(), Some("http://librusec.ru/download/tw/voyna.fb2.zip"));

    // Search over title and author (case-insensitive, Cyrillic).
    assert_eq!(cat.search("война"), vec![0]);
    assert_eq!(cat.search("достоевский"), vec![1]);
    assert_eq!(cat.search(""), vec![0, 1]);
}

// ---------- queue ----------

#[test]
fn queue_lifecycle_and_cancel() {
    let mut q = DownloadQueue::new();
    let a = q.enqueue("A", "http://a");
    let b = q.enqueue("B", "http://b");
    assert_eq!(q.len(), 2);
    assert_eq!(q.next_queued().unwrap().id, a, "FIFO order");

    q.set_progress(a, 50, Some(100));
    assert!((q.get(a).unwrap().status.fraction() - 0.5).abs() < 1e-9);
    assert_eq!(q.pending(), 2, "active + queued both pending");

    q.mark_done(a);
    q.mark_failed(b, "boom");
    assert!(q.all_finished());
    assert_eq!(q.get(b).unwrap().status, ItemStatus::Failed("boom".into()));

    // cancel_all flips every unfinished item to Cancelled.
    let mut q2 = DownloadQueue::new();
    q2.enqueue("x", "u");
    q2.enqueue("y", "v");
    q2.cancel_all();
    assert!(q2.all_finished());
    assert_eq!(q2.get(0).unwrap().status, ItemStatus::Cancelled);
}

// ---------- client (mock transport) ----------

struct Mock {
    /// url -> (status, body, final_url)
    routes: HashMap<String, (u16, Vec<u8>, Option<String>)>,
}

impl Transport for Mock {
    fn get(&self, url: &str, _p: Option<&dyn Fn(u64, Option<u64>)>) -> Result<Response> {
        self.route(url)
    }
    fn post(
        &self,
        url: &str,
        _form: &[(String, String)],
        _p: Option<&dyn Fn(u64, Option<u64>)>,
    ) -> Result<Response> {
        self.route(url)
    }
}

impl Mock {
    fn route(&self, url: &str) -> Result<Response> {
        match self.routes.get(url) {
            Some((status, body, fu)) => Ok(Response {
                status: *status,
                body: body.clone(),
                final_url: fu.clone(),
                content_length: None,
            }),
            None => Err(AppError::Http(format!("no route for {url}"))),
        }
    }
}

#[test]
fn client_captures_redirect_and_writes_file() {
    let fb2 = b"<FictionBook></FictionBook>";
    let mut routes = HashMap::new();
    // Login GET redirects to the real file (a .zip target) -> captured into %RESURL%.
    routes.insert(
        "http://x/login".into(),
        (200, b"ok".to_vec(), Some("http://x/files/book.fb2.zip".into())),
    );
    // The file GET returns the book bytes.
    routes.insert(
        "http://x/files/book.fb2.zip".into(),
        (200, fb2.to_vec(), Some("http://x/files/book.fb2.zip".into())),
    );
    let mock = Mock { routes };

    let cmds = parse_commands("GET http://x/login\nGET %RESURL%\nCHECK", &[]);
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("book.fb2"); // not .zip -> skips the archive-integrity check here
    Downloader::new(&mock).run(&cmds, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), fb2);
}

#[test]
fn client_redir_without_target_is_wrong_credentials() {
    let mut routes = HashMap::new();
    // GET lands on a non-archive page -> no %RESURL% captured.
    routes.insert(
        "http://x/noredir".into(),
        (200, b"x".to_vec(), Some("http://x/page.html".into())),
    );
    let mock = Mock { routes };

    let cmds = parse_commands("GET http://x/noredir\nREDIR", &[]);
    let tmp = tempfile::tempdir().unwrap();
    let err = Downloader::new(&mock).run(&cmds, &tmp.path().join("o.fb2")).unwrap_err();
    assert!(matches!(err, AppError::WrongCredentials));
}

#[test]
fn client_check_rejects_error_page() {
    let mut routes = HashMap::new();
    routes.insert(
        "http://x/err".into(),
        (200, b"<!DOCTYPE html><html>overload</html>".to_vec(), Some("http://x/err".into())),
    );
    let mock = Mock { routes };

    let cmds = parse_commands("GET http://x/err\nCHECK", &[]);
    let tmp = tempfile::tempdir().unwrap();
    let err = Downloader::new(&mock).run(&cmds, &tmp.path().join("o.fb2")).unwrap_err();
    assert!(matches!(err, AppError::BlockedByServer));
}

#[test]
fn client_honours_cancel_flag() {
    let mut routes = HashMap::new();
    routes.insert("http://x/login".into(), (200, b"ok".to_vec(), None));
    let mock = Mock { routes };

    let flag = Arc::new(AtomicBool::new(true)); // pre-cancelled
    let cmds = parse_commands("GET http://x/login", &[]);
    let tmp = tempfile::tempdir().unwrap();
    let err = Downloader::new(&mock)
        .with_cancel(flag)
        .run(&cmds, &tmp.path().join("o.fb2"))
        .unwrap_err();
    assert!(matches!(err, AppError::Cancelled));

    // Sanity: Ordering is used by the executor; keep the import exercised.
    let _ = Ordering::SeqCst;
}

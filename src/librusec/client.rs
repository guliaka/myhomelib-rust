//! Downloader: executes a parsed connection script over a pluggable HTTP transport.
//!
//! Production uses [`HttpTransport`] (reqwest, blocking — drive it from a worker thread). Tests use a
//! mock transport so the script engine is exercised with no network.

use std::io::Read as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::error::{AppError, Result};

use super::command::Cmd;

/// A completed HTTP response (transport-agnostic).
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// The URL after any redirects were followed.
    pub final_url: Option<String>,
    pub content_length: Option<u64>,
}

/// Progress callback: `(bytes_done, total_bytes_or_none)`.
pub type ProgressFn = Box<dyn Fn(u64, Option<u64>)>;

/// Abstract HTTP transport so the connection-script engine is testable without network.
pub trait Transport {
    fn get(&self, url: &str, on_progress: Option<&dyn Fn(u64, Option<u64>)>) -> Result<Response>;
    fn post(
        &self,
        url: &str,
        form: &[(String, String)],
        on_progress: Option<&dyn Fn(u64, Option<u64>)>,
    ) -> Result<Response>;
}

/// reqwest-backed transport (blocking client; safe to drive from a worker thread).
#[derive(Default)]
pub struct HttpTransport {
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");
        HttpTransport { client }
    }

    /// Consume a response: capture status/final-URL/content-length, then stream the body reporting
    /// progress as bytes arrive.
    fn finish(resp: reqwest::blocking::Response, on_progress: Option<&dyn Fn(u64, Option<u64>)>) -> Result<Response> {
        let status = resp.status().as_u16();
        let final_url = Some(resp.url().to_string());
        let content_length = resp.content_length();

        let mut body: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        let mut done: u64 = 0;
        let mut reader = resp;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    done += n as u64;
                    body.extend_from_slice(&buf[..n]);
                    if let Some(cb) = on_progress {
                        cb(done, content_length);
                    }
                }
                Err(e) => return Err(AppError::Http(e.to_string())),
            }
        }
        Ok(Response { status, body, final_url, content_length })
    }
}

impl Transport for HttpTransport {
    fn get(&self, url: &str, on_progress: Option<&dyn Fn(u64, Option<u64>)>) -> Result<Response> {
        let resp = self.client.get(url).send().map_err(|e| AppError::Http(e.to_string()))?;
        Self::finish(resp, on_progress)
    }

    fn post(
        &self,
        url: &str,
        form: &[(String, String)],
        on_progress: Option<&dyn Fn(u64, Option<u64>)>,
    ) -> Result<Response> {
        let fields: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let resp = self
            .client
            .post(url)
            .form(&fields)
            .send()
            .map_err(|e| AppError::Http(e.to_string()))?;
        Self::finish(resp, on_progress)
    }
}

/// Executes a connection script against a transport.
pub struct Downloader<'a> {
    transport: &'a dyn Transport,
    cancel: Arc<AtomicBool>,
    progress: Option<ProgressFn>,
}

impl<'a> Downloader<'a> {
    pub fn new(transport: &'a dyn Transport) -> Self {
        Self {
            transport,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
        }
    }

    /// Share a cancellation flag with the caller (e.g. a "Stop" button).
    pub fn with_cancel(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = flag;
        self
    }

    /// Report download progress `(bytes_done, total_or_none)` to `f`.
    pub fn with_progress(mut self, f: ProgressFn) -> Self {
        self.progress = Some(f);
        self
    }

    /// Run the script. On success, `out_path` holds the downloaded file (written by `CHECK`).
    pub fn run(&self, commands: &[Cmd], out_path: &Path) -> Result<()> {
        let mut res_url = String::new();
        let mut form: Vec<(String, String)> = Vec::new();
        let mut last_body: Option<Vec<u8>> = None;

        for cmd in commands {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(AppError::Cancelled);
            }
            match cmd {
                Cmd::Check => check_response(last_body.as_deref(), out_path)?,
                Cmd::Redir => {
                    if res_url.is_empty() {
                        return Err(AppError::WrongCredentials);
                    }
                }
                Cmd::Pause(ms) => thread::sleep(Duration::from_millis((*ms) as u64)),
                Cmd::Get(tmpl) => {
                    let url = tmpl.replace("%RESURL%", &res_url);
                    let resp = self.transport.get(&url, self.progress_cb())?;
                    if let Some(fu) = &resp.final_url {
                        if is_archive_target(fu) {
                            res_url = fu.clone();
                        }
                    }
                    last_body = Some(resp.body);
                }
                Cmd::Post(tmpl) => {
                    let url = tmpl.replace("%RESURL%", &res_url);
                    let resp = self.transport.post(&url, &form, self.progress_cb())?;
                    form.clear();
                    if let Some(fu) = &resp.final_url {
                        if is_archive_target(fu) {
                            res_url = fu.clone();
                        }
                    }
                    last_body = Some(resp.body);
                }
                Cmd::Add(name, value) => form.push((name.clone(), value.clone())),
                Cmd::Skip => {}
            }
        }
        Ok(())
    }

    fn progress_cb(&self) -> Option<&dyn Fn(u64, Option<u64>)> {
        self.progress.as_deref()
    }
}

/// `CHECK`: validate the last response (error-page detection) and write it to disk.
fn check_response(body: Option<&[u8]>, out_path: &Path) -> Result<()> {
    let body = body.ok_or(AppError::NoResponse)?;
    if body.is_empty() {
        return Err(AppError::NoResponse);
    }
    // Faithful blocked-by-server detection: the response is an HTML error / overload / not-found page.
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    let first = head.lines().next().unwrap_or("").to_lowercase();
    if first.contains("<!doctype") || first.contains("overload") || first.contains("not found") {
        return Err(AppError::BlockedByServer);
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out_path, body)?;

    // Sanity-check zip containers (the original tests archive integrity before accepting the file).
    if out_path.to_string_lossy().to_lowercase().ends_with(".zip") {
        let f = std::fs::File::open(out_path)?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| AppError::Other(format!("corrupt archive: {e}")))?;
        for i in 0..zip.len() {
            zip.by_index(i)
                .map_err(|e| AppError::Other(format!("corrupt archive entry {i}: {e}")))?;
        }
    }
    Ok(())
}

/// A redirect target counts as the downloadable file if it's a zip/fb2 container.
fn is_archive_target(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_lowercase().ends_with(".zip")
}

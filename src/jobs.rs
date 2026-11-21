//! Background jobs with progress, a log ("teletype") and cooperative cancellation — the port of the
//! original's `TWorker` + progress form pair (`unit_WorkerThread.pas`, `frm_BaseProgressForm.pas`).
//!
//! A job runs on its own thread and reports through a cheap, cloneable [`JobProgress`] handle. The GUI
//! polls that handle once per frame; nothing here depends on egui, so importers stay usable from the
//! CLI and from tests (pass `&JobProgress::default()` when nobody is listening).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::error::Result;

/// Severity of a log line (the original's `TTeletypeSeverity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub severity: Severity,
    pub text: String,
}

/// The log keeps at most this many lines so a broken million-record import can't eat memory; the
/// error counter keeps counting past the cap.
const MAX_LOG_LINES: usize = 2000;

#[derive(Default)]
struct Shared {
    comment: String,
    log: Vec<LogLine>,
    /// Set exactly once, when the job function returns.
    result: Option<std::result::Result<String, String>>,
}

#[derive(Default)]
struct Inner {
    /// Progress in hundredths of a percent (0..=10000).
    permyriad: AtomicU32,
    cancel: AtomicBool,
    done: AtomicBool,
    errors: AtomicUsize,
    shared: Mutex<Shared>,
}

/// Shared progress/cancel handle for one job. Cloning is cheap (an `Arc`).
#[derive(Clone, Default)]
pub struct JobProgress {
    inner: Arc<Inner>,
}

impl JobProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set overall progress, in percent (clamped to 0..=100).
    pub fn set_percent(&self, percent: f64) {
        let p = if percent.is_finite() { percent.clamp(0.0, 100.0) } else { 0.0 };
        self.inner.permyriad.store((p * 100.0) as u32, Ordering::Relaxed);
    }

    pub fn percent(&self) -> f64 {
        self.inner.permyriad.load(Ordering::Relaxed) as f64 / 100.0
    }

    /// One-line "what is happening now" text shown above the progress bar.
    pub fn set_comment(&self, text: impl Into<String>) {
        if let Ok(mut g) = self.inner.shared.lock() {
            g.comment = text.into();
        }
    }

    pub fn comment(&self) -> String {
        self.inner.shared.lock().map(|g| g.comment.clone()).unwrap_or_default()
    }

    /// Append a log line. Errors are always counted, even once the visible log is full.
    pub fn log(&self, severity: Severity, text: impl Into<String>) {
        if severity == Severity::Error {
            self.inner.errors.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut g) = self.inner.shared.lock() {
            if g.log.len() < MAX_LOG_LINES {
                g.log.push(LogLine { severity, text: text.into() });
            } else if g.log.len() == MAX_LOG_LINES {
                g.log.push(LogLine {
                    severity: Severity::Warning,
                    text: "… (журнал обрезан, дальнейшие сообщения не показаны)".into(),
                });
            }
        }
    }

    pub fn info(&self, text: impl Into<String>) {
        self.log(Severity::Info, text);
    }

    pub fn error(&self, text: impl Into<String>) {
        self.log(Severity::Error, text);
    }

    /// A snapshot of the log (bounded by the cap, so cloning it per frame is fine).
    pub fn log_lines(&self) -> Vec<LogLine> {
        self.inner.shared.lock().map(|g| g.log.clone()).unwrap_or_default()
    }

    pub fn error_count(&self) -> usize {
        self.inner.errors.load(Ordering::Relaxed)
    }

    /// Ask the job to stop at its next checkpoint.
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancel.load(Ordering::SeqCst)
    }

    pub fn is_done(&self) -> bool {
        self.inner.done.load(Ordering::SeqCst)
    }

    /// The job's final outcome once it is done: `Ok(summary)` or `Err(message)`.
    pub fn result(&self) -> Option<std::result::Result<String, String>> {
        self.inner.shared.lock().ok().and_then(|g| g.result.clone())
    }

    fn finish(&self, result: std::result::Result<String, String>) {
        if let Ok(mut g) = self.inner.shared.lock() {
            g.result = Some(result);
        }
        self.inner.done.store(true, Ordering::SeqCst);
    }
}

/// A running (or finished) background job.
pub struct Job {
    pub title: String,
    pub progress: JobProgress,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    /// Run `work` on a new thread. Its `Ok(summary)` / `Err` becomes [`JobProgress::result`]; a panic
    /// inside `work` is reported as an error instead of leaving the job "running" forever.
    pub fn spawn<F>(title: impl Into<String>, work: F) -> Self
    where
        F: FnOnce(&JobProgress) -> Result<String> + Send + 'static,
    {
        let progress = JobProgress::new();
        let p = progress.clone();
        let handle = std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&p)));
            let result = match outcome {
                Ok(Ok(summary)) => Ok(summary),
                Ok(Err(e)) => {
                    p.error(e.to_string());
                    Err(e.to_string())
                }
                Err(_) => {
                    p.error("внутренняя ошибка: фоновая задача аварийно завершилась");
                    Err("background job panicked".to_string())
                }
            };
            p.finish(result);
        });
        Self { title: title.into(), progress, handle: Some(handle) }
    }

    pub fn is_done(&self) -> bool {
        self.progress.is_done()
    }

    /// Block until the job thread has exited (used when the job is done, and by tests).
    pub fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_reports_progress_and_result() {
        let mut job = Job::spawn("t", |p| {
            p.set_percent(50.0);
            p.info("half way");
            p.error("one bad record");
            Ok("done".to_string())
        });
        job.join();
        assert!(job.is_done());
        assert_eq!(job.progress.percent(), 50.0);
        assert_eq!(job.progress.error_count(), 1);
        assert_eq!(job.progress.log_lines().len(), 2);
        assert_eq!(job.progress.result(), Some(Ok("done".to_string())));
    }

    #[test]
    fn job_error_and_panic_are_reported() {
        let mut failing = Job::spawn("t", |_| Err(crate::error::AppError::Other("boom".into())));
        failing.join();
        assert_eq!(failing.progress.result(), Some(Err("boom".to_string())));
        assert_eq!(failing.progress.error_count(), 1);

        let mut panicking = Job::spawn("t", |_| -> Result<String> { panic!("kaboom") });
        panicking.join();
        assert!(panicking.is_done());
        assert!(panicking.progress.result().unwrap().is_err());
    }

    #[test]
    fn log_is_capped_but_errors_keep_counting() {
        let p = JobProgress::new();
        for i in 0..(MAX_LOG_LINES + 500) {
            p.error(format!("e{i}"));
        }
        assert_eq!(p.error_count(), MAX_LOG_LINES + 500);
        assert_eq!(p.log_lines().len(), MAX_LOG_LINES + 1); // cap + the "truncated" marker
    }
}

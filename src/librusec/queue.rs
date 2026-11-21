//! Download-queue state machine (pure Rust; mirrors the original's download-manager thread).
//!
//! The queue is in-memory for a session (the original keeps it in the manager thread, not on disk).
//! "Resume" in the original means *skip-if-already-present* plus retrying a failed item — not
//! HTTP byte-range resume — so this model tracks per-item status rather than partial offsets.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    Queued,
    Active { done: u64, total: Option<u64> },
    Done,
    Failed(String),
    Cancelled,
}

impl ItemStatus {
    pub fn is_finished(&self) -> bool {
        matches!(self, ItemStatus::Done | ItemStatus::Failed(_) | ItemStatus::Cancelled)
    }

    /// Progress in `[0.0, 1.0]`; `1.0` once finished, `0.0` when queued or total unknown.
    pub fn fraction(&self) -> f64 {
        match self {
            ItemStatus::Queued | ItemStatus::Cancelled => 0.0,
            ItemStatus::Done | ItemStatus::Failed(_) => 1.0,
            ItemStatus::Active { done, total } => match total {
                Some(t) if *t > 0 => (*done as f64) / (*t as f64),
                _ => 0.0,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: u64,
    /// Human label (e.g. `"Толстой Л. — Война и мир"`).
    pub label: String,
    /// The connection-script target / book download reference.
    pub url: String,
    pub status: ItemStatus,
}

#[derive(Debug, Default)]
pub struct DownloadQueue {
    items: VecDeque<QueueItem>,
    next_id: u64,
}

impl DownloadQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a book to the queue; returns its id.
    pub fn enqueue(&mut self, label: impl Into<String>, url: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push_back(QueueItem {
            id,
            label: label.into(),
            url: url.into(),
            status: ItemStatus::Queued,
        });
        id
    }

    /// The next queued item (FIFO), if any.
    pub fn next_queued(&self) -> Option<&QueueItem> {
        self.items.iter().find(|i| matches!(i.status, ItemStatus::Queued))
    }

    pub fn get(&self, id: u64) -> Option<&QueueItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Mark an item as actively downloading (or update its progress if already active).
    pub fn set_progress(&mut self, id: u64, done: u64, total: Option<u64>) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = ItemStatus::Active { done, total };
        }
    }

    pub fn mark_done(&mut self, id: u64) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = ItemStatus::Done;
        }
    }

    pub fn mark_failed(&mut self, id: u64, err: impl Into<String>) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = ItemStatus::Failed(err.into());
        }
    }

    /// Cancel one item (queued or active). Finished items are left as-is.
    pub fn cancel(&mut self, id: u64) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            if !item.status.is_finished() {
                item.status = ItemStatus::Cancelled;
            }
        }
    }

    /// Cancel every item that is not already finished.
    pub fn cancel_all(&mut self) {
        for item in &mut self.items {
            if !item.status.is_finished() {
                item.status = ItemStatus::Cancelled;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Items still to process (queued or actively downloading).
    pub fn pending(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.status, ItemStatus::Queued | ItemStatus::Active { .. }))
            .count()
    }

    /// True when nothing is queued or active.
    pub fn all_finished(&self) -> bool {
        self.pending() == 0
    }

    /// Iterate all items in enqueue order.
    pub fn iter(&self) -> impl Iterator<Item = &QueueItem> {
        self.items.iter()
    }
}

//! Background herdr work for the dashboard. The UI thread only draws, reads
//! keys and swaps in results: the fleet refresh runs on its own thread, and each
//! key press that calls herdr runs on a short-lived thread of its own. A slow
//! herdr call can therefore delay its own result, never the keyboard.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::herdr::Herdr;
use crate::model::{AgentRow, Fleet, Options};
use crate::transcript::TranscriptTracker;

/// A finished piece of background work, delivered to the UI thread.
pub enum Done {
    /// A fleet snapshot, or why it could not be taken.
    Fleet(Result<Fleet, String>),
    /// The text of a pane, for the detail view.
    Tail {
        pane: String,
        text: Result<String, String>,
    },
    /// A one-line status for the footer. Empty means nothing to show.
    Note(String),
}

/// Handle the UI thread keeps: it sends work and polls for what came back.
pub struct Worker {
    herdr: Herdr,
    refresh_tx: Sender<()>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
}

impl Worker {
    /// Start the refresh thread. It owns the transcript cache and the previous
    /// snapshot, so a stale pane can keep its last values.
    pub fn spawn(herdr: Herdr, queue_dir: Option<PathBuf>, options: Options) -> Self {
        let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let tx = done_tx.clone();
        let refresh_herdr = herdr.clone();
        thread::spawn(move || {
            let mut tracker = TranscriptTracker::default();
            let mut prev: HashMap<String, AgentRow> = HashMap::new();
            while refresh_rx.recv().is_ok() {
                // Several requests that piled up while a refresh ran are one refresh.
                while refresh_rx.try_recv().is_ok() {}
                let result = Fleet::collect_with(
                    &refresh_herdr,
                    queue_dir.as_deref(),
                    &options,
                    &mut tracker,
                    &prev,
                )
                .map_err(|e| format!("{e:#}"));
                if let Ok(fleet) = &result {
                    prev = fleet
                        .agents
                        .iter()
                        .map(|a| (a.pane_id.clone(), a.clone()))
                        .collect();
                }
                if tx.send(Done::Fleet(result)).is_err() {
                    break;
                }
            }
        });
        Self {
            herdr,
            refresh_tx,
            done_tx,
            done_rx,
        }
    }

    /// Ask for a fresh snapshot. Returns at once; the result arrives via `try_recv`.
    pub fn request_refresh(&self) {
        let _ = self.refresh_tx.send(());
    }

    /// Run one herdr call on its own thread. Its result arrives via `try_recv`.
    pub fn action(&self, job: impl FnOnce(&Herdr) -> Done + Send + 'static) {
        let herdr = self.herdr.clone();
        let tx = self.done_tx.clone();
        thread::spawn(move || {
            let _ = tx.send(job(&herdr));
        });
    }

    /// The next finished job, if any. Never blocks.
    pub fn try_recv(&self) -> Option<Done> {
        self.done_rx.try_recv().ok()
    }

    /// Wait up to `wait` for the next finished job.
    pub fn recv_timeout(&self, wait: Duration) -> Option<Done> {
        self.done_rx.recv_timeout(wait).ok()
    }
}

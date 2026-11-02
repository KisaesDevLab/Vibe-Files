//! Runs a job on a `std::thread` and wakes the UI when it finishes.

use std::sync::mpsc::{Receiver, channel};

/// Runs `job` on a new thread. Poll the receiver with `try_recv`; the UI is repainted when the
/// result is ready.
pub fn background<T: Send + 'static>(
    ctx: &egui::Context,
    job: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
    rx
}

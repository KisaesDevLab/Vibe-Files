//! Work a button starts that may take a while (owner 2026-11-12: "when we have actions that
//! might take time to complete add a spinner"): it runs on its own thread, the page shows a
//! spinner with what is going on, and the result is taken on a later frame. Tests run the work
//! at once on their own thread, so a test sees the result on the next frame.

use std::sync::mpsc::{Receiver, TryRecvError};

/// One piece of work running, with what the page says meanwhile.
pub struct Job<T> {
    what: String,
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    /// Starts `work`; `what` is shown beside the spinner ("Scanning the cabinet's folders…").
    pub fn start(
        ctx: &egui::Context,
        what: impl Into<String>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let run = move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        };
        if cfg!(test) {
            run();
        } else {
            std::thread::Builder::new()
                .name("ui-job".into())
                .spawn(run)
                .map_err(|e| format!("could not start: {e}"))?;
        }
        Ok(Self {
            what: what.into(),
            rx,
        })
    }

    /// The result once the work is done; `Err` when its thread died without one.
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(Ok(r)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the work stopped unexpectedly".into())),
        }
    }

    /// The spinner and what is going on, on one line; repaints until the work is done.
    pub fn show(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(&self.what);
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Takes the result of `job` once it is done (and clears it).
pub fn take<T: Send + 'static>(job: &mut Option<Job<T>>) -> Option<Result<T, String>> {
    let r = job.as_ref()?.poll()?;
    *job = None;
    Some(r)
}

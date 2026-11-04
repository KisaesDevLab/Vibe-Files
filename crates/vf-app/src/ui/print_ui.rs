//! The Print flow shared by the Viewer and Browse (PHASES PA-13): the SSN/EIN warning first,
//! then the Windows print dialog, then the documents printed on their own thread, one spooled
//! job each; a failed document is reported and the rest carry on. One `print` audit row per
//! document, written together (no content: pages, copies, printer).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use vf_core::db::Connection;

use crate::ui::widgets;

/// A document to print.
#[derive(Debug, Clone)]
pub struct PrintDoc {
    pub doc_id: String,
    pub name: String,
    pub path: PathBuf,
    pub hash: String,
}

/// Per document: (doc id, name, pages printed or why not).
type Outcome = Vec<(String, String, Result<usize, String>)>;

/// Documents waiting for "Print anyway": (documents, how many flagged, page count, current page).
type Asking = (Vec<PrintDoc>, usize, Option<u32>, Option<u32>);

#[derive(Default)]
pub struct PrintState {
    asking: Option<Asking>,
    job: Option<(String, u32, Receiver<Outcome>)>,
}

/// How many of `docs` have text that looks like an SSN or an EIN (P03's page text).
pub fn flagged(conn: &Connection, docs: &[PrintDoc]) -> usize {
    docs.iter()
        .filter(|d| {
            conn.query_row(
                "SELECT 1 FROM page_text WHERE hash = ?1 AND (
                   text GLOB '*[0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][0-9][0-9]*'
                   OR text GLOB '*[0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9][0-9]*') LIMIT 1",
                [&d.hash],
                |_| Ok(()),
            )
            .is_ok()
        })
        .count()
}

impl PrintState {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Print pressed. `pages` / `current`: the Viewer's page count and page (a page range can
    /// be chosen); `None` from Browse.
    pub fn start(
        &mut self,
        conn: &Connection,
        docs: Vec<PrintDoc>,
        pages: Option<u32>,
        current: Option<u32>,
    ) -> Result<(), String> {
        if docs.is_empty() || self.running() {
            return Ok(());
        }
        let n = flagged(conn, &docs);
        if n > 0 {
            self.asking = Some((docs, n, pages, current));
            return Ok(());
        }
        self.go(docs, pages, current)
    }

    fn go(
        &mut self,
        docs: Vec<PrintDoc>,
        pages: Option<u32>,
        current: Option<u32>,
    ) -> Result<(), String> {
        let Some(choice) = crate::print::dialog(pages, current)? else {
            return Ok(()); // cancelled
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let printer = choice.printer.clone();
        let copies = choice.copies;
        std::thread::Builder::new()
            .name("print".into())
            .spawn(move || {
                let mut out: Outcome = Vec::new();
                for d in docs {
                    let r = if d.name.to_lowercase().ends_with(".pdf") {
                        crate::print::print_pdf(&choice, &d.path, &d.name, None)
                    } else {
                        crate::print::print_with_its_app(&d.path).map(|()| 0)
                    };
                    out.push((d.doc_id, d.name, r));
                }
                drop(choice);
                let _ = tx.send(out);
            })
            .map_err(|e| e.to_string())?;
        self.job = Some((printer, copies, rx));
        Ok(())
    }

    /// The SSN/EIN question, while asked (a modal). Returns an error to show.
    pub fn ask(&mut self, ctx: &egui::Context) -> Option<String> {
        let (docs, n, pages, current) = self.asking.clone()?;
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("print-pii")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Print documents with taxpayer IDs?");
            let total = docs.len();
            ui.label(if total == 1 {
                "This document has what looks like an SSN or an EIN.".to_string()
            } else {
                format!("{n} of the {total} documents have what looks like an SSN or an EIN.")
            });
            ui.label(widgets::caption(
                "Printed copies leave the cabinet's protections; the print is recorded in the audit trail.",
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(widgets::primary_button("Print anyway")).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => {
                self.asking = None;
                self.go(docs, pages, current).err()
            }
            Some(false) => {
                self.asking = None;
                None
            }
            None => None,
        }
    }

    /// Takes a finished job: audits it and says how it went.
    pub fn poll(&mut self, conn: &mut Connection, device_id: &str) -> Option<String> {
        let (_, _, rx) = self.job.as_ref()?;
        let out = match rx.try_recv() {
            Ok(o) => o,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                return Some("Printing stopped unexpectedly.".into());
            }
        };
        let (printer, copies, _) = self.job.take().expect("checked");
        Some(record(conn, device_id, &printer, copies, &out))
    }

    /// "Printing…" while a job runs.
    pub fn status(&self, ui: &mut egui::Ui) {
        if let Some((printer, _, _)) = &self.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Printing to {printer}…")).small());
            });
        }
    }
}

/// One `print` audit row per printed document, in one transaction; the message to show.
pub fn record(
    conn: &mut Connection,
    device_id: &str,
    printer: &str,
    copies: u32,
    out: &[(String, String, Result<usize, String>)],
) -> String {
    let mut tx = vf_core::tx::Transaction::new("print");
    for (id, _, r) in out {
        if let Ok(pages) = r {
            tx = tx.audit(vf_core::audit::AuditEntry::new(
                "print",
                Some(id.as_str()),
                serde_json::json!({ "pages": pages, "copies": copies, "printer": printer }),
            ));
        }
    }
    let printed = out.iter().filter(|o| o.2.is_ok()).count();
    if printed > 0 {
        let _ = tx.commit(
            conn,
            &mut vf_core::tx::NoFileOps,
            &vf_core::tx::Actor::device(device_id),
        );
    }
    let failed: Vec<String> = out
        .iter()
        .filter_map(|(_, n, r)| r.as_ref().err().map(|e| format!("{n}: {e}")))
        .collect();
    if failed.is_empty() {
        format!(
            "Sent {printed} document{} to {printer}.",
            if printed == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Sent {printed} to {printer}; not printed: {}",
            failed.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Browse print of several documents writes one audit batch (PA-13 done-when 3), and the
    /// SSN/EIN check finds a flagged one (done-when 2).
    #[test]
    fn audit_batch_and_pii_check() {
        let t = tempfile::tempdir().unwrap();
        let mut conn =
            vf_core::db::open(&t.path().join("c.db"), vf_core::db::Storage::Local).unwrap();
        conn.execute(
            "INSERT INTO page_text (hash, page, source, text) VALUES ('h1', 0, 'pdf', 'SSN 123-45-6789 invented')",
            [],
        )
        .unwrap();
        let doc = |id: &str, h: &str| PrintDoc {
            doc_id: id.into(),
            name: format!("{id}.pdf"),
            path: PathBuf::from(format!("{id}.pdf")),
            hash: h.into(),
        };
        assert_eq!(flagged(&conn, &[doc("a", "h1"), doc("b", "h2")]), 1);
        let out: Vec<(String, String, Result<usize, String>)> = (0..10)
            .map(|i| (format!("d{i}"), format!("d{i}.pdf"), Ok(2)))
            .collect();
        let msg = record(&mut conn, "dev", "Office Printer", 1, &out);
        assert_eq!(msg, "Sent 10 documents to Office Printer.");
        let (rows, batches): (i64, i64) = conn
            .query_row(
                "SELECT count(*), count(DISTINCT ts) FROM audit WHERE action = 'print'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, batches), (10, 1));
    }
}

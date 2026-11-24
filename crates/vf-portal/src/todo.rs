//! To-do lists (PA-103; Vibe-Portal P109): the firm asks a client for documents and answers, and
//! the client answers on the portal. The portal keeps the lists and every answer with its time,
//! internet address, browser and person; Vibe Files makes, reads, changes and deletes them
//! through the firm API, and keeps a copy of each list it reads in the client's folder
//! (`<client>/_Vibe/todo/<list id>.jsonl`, owner 2026-11-23): plain JSON, one line per version
//! as read, appended only when the list changed, so the copy opens without Vibe Files and
//! travels with the client folder.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::{Portal, PortalError, Result};

/// Item kinds: documents (files) or an answer in words.
pub const KIND_DOCUMENT: &str = "document";
pub const KIND_TEXT: &str = "text";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoPerson {
    pub identity_id: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

/// One answer, with its evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoAnswer {
    pub id: u64,
    /// `upload`, `upload_removed`, `text`, `not_applicable`.
    pub kind: String,
    #[serde(default)]
    pub text: String,
    pub upload_id: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub file_sha256: Option<String>,
    pub at: String,
    pub ip: String,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub session_ref: String,
    pub person: TodoPerson,
}

impl TodoAnswer {
    /// "Sent W2.pdf", "Answered: “…”", in words.
    pub fn words(&self) -> String {
        let file = self.file_name.as_deref().unwrap_or("a file");
        match self.kind.as_str() {
            "upload" => format!("Sent {file}"),
            "upload_removed" => format!("Took back {file}"),
            "text" => format!("Answered: “{}”", self.text),
            "not_applicable" if self.text.is_empty() => "Does not apply".into(),
            "not_applicable" => format!("Does not apply: “{}”", self.text),
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub position: u32,
    pub kind: String,
    pub prompt: String,
    #[serde(default)]
    pub help: String,
    pub required: bool,
    #[serde(default)]
    pub doc_type: String,
    /// `open`, `answered`, `not_applicable`.
    pub state: String,
    #[serde(default)]
    pub responses: Vec<TodoAnswer>,
}

/// A list as the firm sees it. A list read from `GET /request-lists` has no items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoList {
    pub id: String,
    pub client_id: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    pub due_date: Option<String>,
    /// `open`, `closed`, `cancelled`.
    pub status: String,
    pub complete: bool,
    pub done: u32,
    pub total: u32,
    pub created_at: String,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub created_by_kind: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub closed_at: Option<String>,
    #[serde(default)]
    pub closed_by: Option<String>,
    #[serde(default)]
    pub reminded_at: Option<String>,
    #[serde(default)]
    pub remind_every_days: u32,
    #[serde(default)]
    pub assigned_staff_id: Option<String>,
    #[serde(default)]
    pub items: Vec<TodoItem>,
}

impl TodoList {
    pub fn status_words(&self) -> &'static str {
        match (self.status.as_str(), self.complete) {
            ("closed", _) => "Closed",
            ("cancelled", _) => "Cancelled",
            (STATUS_DELETED, _) => "Deleted",
            (_, true) => "Open, every required item answered",
            _ => "Open",
        }
    }

    /// Nobody has answered: it may be deleted (else it is cancelled).
    pub fn unanswered(&self) -> bool {
        self.items.iter().all(|i| i.responses.is_empty())
    }
}

/// An item as written: in a template, a new list, or a list's items being edited (`id` set for
/// one already on the list).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemIn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: String,
    pub prompt: String,
    #[serde(default)]
    pub help: String,
    pub required: bool,
    #[serde(default)]
    pub doc_type: String,
}

impl ItemIn {
    pub fn blank() -> Self {
        Self {
            id: None,
            kind: KIND_DOCUMENT.into(),
            prompt: String::new(),
            help: String::new(),
            required: true,
            doc_type: String::new(),
        }
    }

    pub fn from_item(i: &TodoItem) -> Self {
        Self {
            id: Some(i.id.clone()),
            kind: i.kind.clone(),
            prompt: i.prompt.clone(),
            help: i.help.clone(),
            required: i.required,
            doc_type: i.doc_type.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub items: Vec<ItemIn>,
}

/// A staff member a list may be assigned to (told by email when the client answers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StaffChoice {
    pub id: String,
    pub name: String,
}

/// A new list, sent to one client or (from a template) to many.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewList {
    pub client_ids: Vec<String>,
    pub title: String,
    pub note: String,
    pub due_date: Option<String>,
    pub remind_every_days: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    pub items: Vec<ItemIn>,
    /// Tell the client's people now.
    pub notify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_staff_id: Option<String>,
}

/// A change to a list: only what is `Some` is sent. `due_date: Some(None)` clears the day
/// (sent as null); `assigned_staff_id: Some(None)` leaves nobody assigned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_every_days: Option<u32>,
    /// `open`, `closed`, `cancelled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_staff_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ItemIn>>,
}

/// Largest report read (a list with hundreds of answers is far smaller).
const REPORT_LIMIT: u64 = 64 * 1024 * 1024;

impl Portal {
    pub fn todo_templates(&self) -> Result<Vec<Template>> {
        #[derive(Deserialize)]
        struct R {
            templates: Vec<Template>,
        }
        self.get::<R>("/request-templates", &[])
            .map(|r| r.templates)
    }

    /// Saves a template: a new one when `t.id` is empty.
    pub fn save_template(&self, t: &Template) -> Result<Template> {
        #[derive(Deserialize)]
        struct R {
            template: Template,
        }
        let body = serde_json::json!({ "name": t.name, "note": t.note, "items": t.items });
        let r: R = if t.id.is_empty() {
            self.send_json("POST", "/request-templates", &body)?
        } else {
            self.send_json("PUT", &format!("/request-templates/{}", t.id), &body)?
        };
        Ok(r.template)
    }

    pub fn delete_template(&self, id: &str) -> Result<()> {
        self.delete::<serde_json::Value>(&format!("/request-templates/{id}"))
            .map(|_| ())
    }

    /// A client's lists, newest first, without their items.
    pub fn todo_lists(&self, client_id: &str) -> Result<Vec<TodoList>> {
        #[derive(Deserialize)]
        struct R {
            lists: Vec<TodoList>,
        }
        self.get::<R>("/request-lists", &[("clientId", client_id)])
            .map(|r| r.lists)
    }

    /// One list with its items and every answer.
    pub fn todo_list(&self, id: &str) -> Result<TodoList> {
        #[derive(Deserialize)]
        struct R {
            list: TodoList,
        }
        self.get::<R>(&format!("/request-lists/{id}"), &[])
            .map(|r| r.list)
    }

    /// Sends a list: one for each client.
    pub fn create_lists(&self, l: &NewList) -> Result<Vec<TodoList>> {
        #[derive(Deserialize)]
        struct R {
            lists: Vec<TodoList>,
        }
        let body = serde_json::to_value(l).expect("serializes");
        self.send_json::<R>("POST", "/request-lists", &body)
            .map(|r| r.lists)
    }

    pub fn change_list(&self, id: &str, c: &ListChange) -> Result<TodoList> {
        #[derive(Deserialize)]
        struct R {
            list: TodoList,
        }
        self.send_json::<R>(
            "PATCH",
            &format!("/request-lists/{id}"),
            &serde_json::to_value(c).expect("a change serializes"),
        )
        .map(|r| r.list)
    }

    /// Deletes a list nobody has answered (else 409 `list_has_answers`: cancel it).
    pub fn delete_list(&self, id: &str) -> Result<()> {
        self.delete::<serde_json::Value>(&format!("/request-lists/{id}"))
            .map(|_| ())
    }

    /// Tells the client's people what is still open; how many were told.
    pub fn remind(&self, id: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            told: u32,
        }
        self.post_empty::<R>(&format!("/request-lists/{id}/remind"))
            .map(|r| r.told)
    }

    /// Staff a list of this client may be assigned to.
    pub fn todo_staff(&self, client_id: &str) -> Result<Vec<StaffChoice>> {
        #[derive(Deserialize)]
        struct R {
            staff: Vec<StaffChoice>,
        }
        self.get::<R>("/request-staff", &[("clientId", client_id)])
            .map(|r| r.staff)
    }

    /// A list's report as one Portal inbox arrival for the client (`<Client>_<ID>_<file_name>`,
    /// under a free name, never over a file there): Intake files it into the client's folder.
    /// Written whole beside the cabinet's other portal downloads first; nothing is left on
    /// failure. Where it landed.
    pub fn file_report(
        &self,
        root: &Path,
        client_name: &str,
        client_code: &str,
        file_name: &str,
        id: &str,
    ) -> Result<PathBuf> {
        let bytes = self.report_pdf(id)?;
        let local = |e: std::io::Error| PortalError::Local(e.to_string());
        let tmp = crate::inbox::tmp_dir(root);
        std::fs::create_dir_all(&tmp).map_err(local)?;
        let part = tmp.join(format!(
            "todo-report-{}-{}.part",
            safe_id(id),
            crate::inbox::random_tag()
        ));
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .and_then(|mut f| f.write_all(&bytes).and_then(|()| f.sync_all()))
            .and_then(|()| {
                let name = crate::inbox::inbox_name(client_name, client_code, file_name);
                crate::inbox::move_into(&part, &crate::inbox::inbox(root), &name)
            });
        if written.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        written.map_err(local)
    }

    /// The report PDF of a list.
    pub fn report_pdf(&self, id: &str) -> Result<Vec<u8>> {
        let mut res = self
            .raw_get(&format!("/request-lists/{id}/report.pdf"))
            .map_err(|e| PortalError::Network(e.to_string()))?;
        let status = res.status().as_u16();
        if status != 200 {
            let text = res.body_mut().read_to_string().unwrap_or_default();
            return Err(crate::api::http_error(status, &text));
        }
        let mut bytes = Vec::new();
        res.body_mut()
            .with_config()
            .limit(REPORT_LIMIT)
            .reader()
            .read_to_end(&mut bytes)
            .map_err(|e| PortalError::Network(e.to_string()))?;
        if !bytes.starts_with(b"%PDF") {
            return Err(PortalError::Format("the report is not a PDF".into()));
        }
        Ok(bytes)
    }
}

// ---- The copy in the client's folder ----------------------------------------------------------

/// Folder in a client folder's `_Vibe` holding one file per list.
pub const COPY_DIR: &str = "todo";

/// `<cabinet>/<client folder>/_Vibe/todo`.
fn copy_dir(root: &Path, folder_rel: &str) -> PathBuf {
    root.join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_DIR)
}

/// A list id as it may appear in a file name (letters, digits, `-`).
pub fn safe_id(list_id: &str) -> String {
    list_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// `<cabinet>/<client folder>/_Vibe/todo/<list id>.jsonl`.
pub fn copy_path(root: &Path, folder_rel: &str, list_id: &str) -> PathBuf {
    copy_dir(root, folder_rel).join(format!("{}.jsonl", safe_id(list_id)))
}

/// The last line of a copy's text, read.
fn last_copy(text: &str) -> Option<CopyLine> {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str::<CopyLine>(l).ok())
}

/// The newest line of the copy at `path`, reading only the end of the file (it grows with every
/// change of the list; its newest line is far smaller than the tail read). `None` when there is
/// no copy yet.
pub fn newest_copy(path: &Path) -> Option<CopyLine> {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 4 * 1024 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    last_copy(&String::from_utf8_lossy(&bytes))
}

/// One line of the copy: the list as read, and when and by whom.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyLine {
    pub v: u32,
    pub read_at: String,
    pub read_by: String,
    pub list: TodoList,
}

/// The line to append to the list's copy, or `None` when the copy already ends with this list
/// (nothing changed since it was last read). `last` is the copy's newest line ([`newest_copy`]).
pub fn copy_line(
    last: Option<&CopyLine>,
    list: &TodoList,
    read_at: &str,
    read_by: &str,
) -> Option<String> {
    if last.is_some_and(|l| l.list == *list) {
        return None;
    }
    Some(
        serde_json::to_string(&CopyLine {
            v: 1,
            read_at: read_at.into(),
            read_by: read_by.into(),
            list: list.clone(),
        })
        .expect("serializes"),
    )
}

/// The status a list's last copy line is given when the list is deleted from the portal (the
/// portal keeps no deleted list; the copy says it is gone).
pub const STATUS_DELETED: &str = "deleted";

/// The latest copy of each list in a client folder (for when the portal cannot be reached).
pub fn read_copies(root: &Path, folder_rel: &str) -> Vec<CopyLine> {
    let Ok(entries) = std::fs::read_dir(copy_dir(root, folder_rel)) else {
        return Vec::new();
    };
    let mut out: Vec<CopyLine> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| newest_copy(&e.path()))
        // A list deleted from the portal (nobody had answered it) is not shown as if it were
        // still sent; its file keeps the record (walkthrough 2026-11-23).
        .filter(|c| c.list.status != STATUS_DELETED)
        .collect();
    out.sort_by(|a, b| b.list.created_at.cmp(&a.list.created_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list as the portal's tests answer it (Vibe-Portal requests.test.ts).
    pub(crate) const LIST_JSON: &str = r#"{
      "id":"5d3c0f3e-1111-4111-8111-111111111111","clientId":"11111111-1111-4111-8111-111111111111",
      "title":"2026 tax documents","note":"","dueDate":"2027-03-15","status":"open",
      "complete":false,"done":1,"total":3,"createdAt":"2026-11-09T15:00:00.000Z",
      "templateId":null,"createdByKind":"firm_key","createdBy":"PC1\\kurt","closedAt":null,
      "closedBy":null,"remindedAt":"2026-11-09T15:00:00.000Z","remindEveryDays":7,
      "assignedStaffId":null,
      "items":[
        {"id":"a1","position":1,"kind":"document","prompt":"W-2 from each employer","help":"",
         "required":true,"docType":"W-2","state":"open","responses":[]},
        {"id":"a2","position":2,"kind":"text","prompt":"Did you sell or buy any property?","help":"",
         "required":true,"docType":"","state":"answered","responses":[
           {"id":7,"kind":"text","text":"Yes, the rental.","uploadId":null,"fileName":null,
            "fileSize":null,"fileSha256":null,"at":"2026-11-09T15:00:00.000Z","ip":"203.0.113.24",
            "userAgent":"Test/1.0","sessionRef":"0123456789abcdef",
            "person":{"identityId":"p1","name":"a@example.test","email":"a@example.test","phone":null}}]}
      ]}"#;

    #[test]
    fn reads_the_portal_list_with_its_evidence() {
        let l: TodoList = serde_json::from_str(LIST_JSON).unwrap();
        assert_eq!(l.items.len(), 2);
        let a = &l.items[1].responses[0];
        assert_eq!(a.ip, "203.0.113.24");
        assert_eq!(a.words(), "Answered: “Yes, the rental.”");
        assert_eq!(l.remind_every_days, 7);
        assert!(!l.unanswered());
        // A list without items (GET /request-lists) reads too.
        let short: TodoList = serde_json::from_str(
            r#"{"id":"x","clientId":"c","title":"t","dueDate":null,"status":"open","complete":false,
                "done":0,"total":2,"createdAt":"2026-11-09T15:00:00Z"}"#,
        )
        .unwrap();
        assert!(short.items.is_empty());
    }

    #[test]
    fn a_change_sends_only_what_changed_and_null_to_clear() {
        let c = ListChange {
            due_date: Some(None),
            assigned_staff_id: Some(Some("s1".into())),
            status: Some("closed".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({ "dueDate": null, "assignedStaffId": "s1", "status": "closed" })
        );
        let items = ListChange {
            items: Some(vec![ItemIn::blank()]),
            ..Default::default()
        };
        // A new item has no id.
        assert!(
            serde_json::to_value(&items).unwrap()["items"][0]
                .get("id")
                .is_none()
        );
    }

    #[test]
    fn the_copy_gets_a_line_only_when_the_list_changed() {
        let t = tempfile::tempdir().unwrap();
        let l: TodoList = serde_json::from_str(LIST_JSON).unwrap();
        let p = copy_path(t.path(), "Smith, John (1001)", &l.id);
        assert!(
            p.ends_with("Smith, John (1001)/_Vibe/todo/5d3c0f3e-1111-4111-8111-111111111111.jsonl")
        );
        let first = copy_line(None, &l, "2026-11-09T16:00:00Z", "PC1\\kurt").unwrap();
        let last = last_copy(&first);
        assert_eq!(copy_line(last.as_ref(), &l, "later", "PC2\\ann"), None);
        let mut changed = l.clone();
        changed.status = "closed".into();
        let second = copy_line(last.as_ref(), &changed, "later", "PC2\\ann").unwrap();
        // The copy can be read back from the folder: the latest line of each list.
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("{first}\n{second}\n")).unwrap();
        let back = read_copies(t.path(), "Smith, John (1001)");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].list.status, "closed");
        assert_eq!(back[0].read_by, "PC2\\ann");
        // Deleted from the portal: the last line says so, and the list is no longer shown.
        let mut gone = changed.clone();
        gone.status = STATUS_DELETED.into();
        let third = copy_line(last_copy(&second).as_ref(), &gone, "later", "PC2\\ann").unwrap();
        std::fs::write(&p, format!("{first}\n{second}\n{third}\n")).unwrap();
        assert!(read_copies(t.path(), "Smith, John (1001)").is_empty());
        assert_eq!(gone.status_words(), "Deleted");
    }
}

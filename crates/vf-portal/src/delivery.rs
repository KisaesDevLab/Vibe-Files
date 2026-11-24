//! Tax return delivery (ADDENDUM-tax-return-delivery.md §12, owner 2026-11-24; Vibe-Portal
//! D1/D2): Vibe Files builds a delivery (a card of steps on the client's portal home page) and
//! the portal runs it, logs it, and files a PDF of each finished step back as an upload tagged
//! with its delivery. Each delivery read is copied into the client's folder
//! (`<client>/_Vibe/delivery/<id>.jsonl`, one line per changed version, as to-do lists), shown
//! when the portal is out of reach.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::{Portal, PortalError, Result, Status};

pub const RETURN_TYPES: [&str; 4] = ["1040", "1120", "1120-S", "1065"];

/// Step kinds, in their order on the card (§1).
pub const LETTERS: &str = "letters";
pub const RETURN: &str = "return";
pub const PACKAGE: &str = "package";
pub const FEE: &str = "fee";
pub const ESIGN: &str = "esign_8879";

/// The doc type each step's PDF is filed as (§12.5), seeded when deliveries are turned on.
pub const STEP_DOC_TYPES: [(&str, &str); 5] = [
    (LETTERS, "Delivery — letters"),
    (RETURN, "Delivery — return record"),
    (PACKAGE, "Delivery — package selection"),
    (FEE, "Delivery — fee receipt"),
    (ESIGN, "Delivery — 8879 record"),
];
/// Where the step PDFs are filed in the client's folder.
pub const DELIVERY_FOLDER: &str = "Income Tax/{Year}/Delivery";

pub fn doc_type_of_step(step: &str) -> Option<&'static str> {
    STEP_DOC_TYPES
        .iter()
        .find(|(k, _)| *k == step)
        .map(|(_, n)| *n)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Locks {
    pub return_after_letters: bool,
    pub esign_after_letters: bool,
    pub esign_after_fee: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryStep {
    pub kind: String,
    pub n: u8,
    pub name: String,
    pub present: bool,
    /// `absent`, `coming`, `locked`, `open`, `waiting`, `done`.
    pub state: String,
    #[serde(default)]
    pub after: Vec<u8>,
    pub said_at: Option<String>,
    pub completed_at: Option<String>,
    pub completed_by: Option<String>,
    #[serde(default)]
    pub rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryEvent {
    pub at: String,
    #[serde(default)]
    pub step: String,
    pub kind: String,
    #[serde(default)]
    pub words: String,
    #[serde(default)]
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReturnFile {
    pub id: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub client_id: String,
    pub tax_year: i32,
    pub return_type: String,
    pub title: String,
    /// `open`, `complete`, `cancelled`.
    pub status: String,
    pub locks: Locks,
    pub assigned_staff_id: Option<String>,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub remind_days: u32,
    pub reminded_at: Option<String>,
    pub return_file: Option<ReturnFile>,
    pub esign_link: Option<String>,
    #[serde(default)]
    pub created_by: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub steps: Vec<DeliveryStep>,
    /// Only on one delivery's read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<DeliveryEvent>,
}

impl Delivery {
    pub fn step(&self, kind: &str) -> Option<&DeliveryStep> {
        self.steps.iter().find(|s| s.kind == kind)
    }

    /// The steps included, done and in all: (3, 5).
    pub fn progress(&self) -> (usize, usize) {
        let present: Vec<&DeliveryStep> = self.steps.iter().filter(|s| s.present).collect();
        (
            present.iter().filter(|s| s.state == "done").count(),
            present.len(),
        )
    }

    /// The step waiting on someone: the first included and not done, in card order.
    pub fn waiting(&self) -> Option<&DeliveryStep> {
        self.steps.iter().find(|s| s.present && s.state != "done")
    }

    /// "Tax return delivery: 3 of 5 done", for the client panel.
    pub fn line(&self) -> String {
        let (done, all) = self.progress();
        match self.status.as_str() {
            "complete" => format!("{} {}: complete", self.tax_year, self.return_type),
            "cancelled" => format!("{} {}: cancelled", self.tax_year, self.return_type),
            _ => format!(
                "{} {}: {done} of {all} done{}",
                self.tax_year,
                self.return_type,
                self.waiting()
                    .map(|s| format!(" · {} {}", s.name, state_words(&s.state)))
                    .unwrap_or_default()
            ),
        }
    }
}

/// A step's state in words.
pub fn state_words(state: &str) -> &'static str {
    match state {
        "coming" => "not added yet",
        "locked" => "locked",
        "open" => "with the client",
        "waiting" => "waiting for you to confirm",
        "done" => "done",
        _ => "",
    }
}

/// What Send to client makes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewDelivery {
    pub client_id: String,
    pub tax_year: i32,
    pub return_type: String,
    pub title: String,
    pub steps: StepsIn,
    pub locks: Locks,
    pub assigned_staff_id: Option<String>,
    pub notify: bool,
    pub remind_days: u32,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StepsIn {
    pub letters: bool,
    #[serde(rename = "return")]
    pub return_: bool,
    pub package: bool,
    pub fee: bool,
    pub esign8879: bool,
}

/// What Save changes sends (only what is set).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<StepsIn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locks: Option<Locks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_staff_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind_days: Option<u32>,
}

impl Status {
    /// The portal has tax return delivery, turned on (Addendum D1).
    pub fn has_deliveries(&self) -> bool {
        self.features.iter().any(|f| f == "deliveries")
    }
}

#[derive(Deserialize)]
struct One {
    delivery: Delivery,
}

/// Largest report read.
const REPORT_LIMIT: u64 = 64 * 1024 * 1024;

impl Portal {
    /// Deliveries, newest first (summaries without events); one client's when given.
    pub fn deliveries(&self, client_id: Option<&str>) -> Result<Vec<Delivery>> {
        #[derive(Deserialize)]
        struct R {
            deliveries: Vec<Delivery>,
        }
        let q: Vec<(&str, &str)> = client_id.map(|c| vec![("clientId", c)]).unwrap_or_default();
        self.get::<R>("/deliveries", &q).map(|r| r.deliveries)
    }

    pub fn delivery(&self, id: &str) -> Result<Delivery> {
        self.get::<One>(&format!("/deliveries/{id}"), &[])
            .map(|r| r.delivery)
    }

    pub fn create_delivery(&self, d: &NewDelivery) -> Result<Delivery> {
        self.send_json::<One>(
            "POST",
            "/deliveries",
            &serde_json::to_value(d).expect("serializes"),
        )
        .map(|r| r.delivery)
    }

    pub fn change_delivery(&self, id: &str, c: &DeliveryChange) -> Result<Delivery> {
        self.send_json::<One>(
            "PATCH",
            &format!("/deliveries/{id}"),
            &serde_json::to_value(c).expect("serializes"),
        )
        .map(|r| r.delivery)
    }

    /// Step 2: the return, a file already on the portal for this client.
    pub fn set_delivery_return(&self, id: &str, file_id: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/return"),
            &serde_json::json!({ "fileId": file_id }),
        )
        .map(|r| r.delivery)
    }

    /// Step 5: the client's UltraTax eSignature link (https only).
    pub fn set_esign_link(&self, id: &str, url: &str) -> Result<Delivery> {
        self.send_json::<One>(
            "PUT",
            &format!("/deliveries/{id}/esign-link"),
            &serde_json::json!({ "url": url }),
        )
        .map(|r| r.delivery)
    }

    pub fn confirm_8879(&self, id: &str) -> Result<Delivery> {
        self.post_empty::<One>(&format!("/deliveries/{id}/confirm-8879"))
            .map(|r| r.delivery)
    }

    pub fn cancel_delivery(&self, id: &str) -> Result<Delivery> {
        self.post_empty::<One>(&format!("/deliveries/{id}/cancel"))
            .map(|r| r.delivery)
    }

    /// Tells the client again; how many messages went.
    pub fn remind_delivery(&self, id: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct R {
            told: u32,
        }
        self.post_empty::<R>(&format!("/deliveries/{id}/remind"))
            .map(|r| r.told)
    }

    /// The whole delivery's report PDF.
    pub fn delivery_report(&self, id: &str) -> Result<Vec<u8>> {
        use std::io::Read;
        let mut res = self
            .raw_get(&format!("/deliveries/{id}/report.pdf"))
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

pub const COPY_DIR: &str = "delivery";

/// `<cabinet>/<client folder>/_Vibe/delivery/<id>.jsonl`.
pub fn copy_path(root: &Path, folder_rel: &str, id: &str) -> PathBuf {
    root.join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_DIR)
        .join(format!("{}.jsonl", crate::todo::safe_id(id)))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyLine {
    pub v: u32,
    pub read_at: String,
    pub read_by: String,
    pub delivery: Delivery,
}

/// The newest line of a copy (`None` when there is none).
pub fn newest_copy(path: &Path) -> Option<CopyLine> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| serde_json::from_str(l).ok())
}

/// The line to append when the delivery changed since the copy's last line (events count).
pub fn copy_line(
    last: Option<&CopyLine>,
    d: &Delivery,
    read_at: &str,
    read_by: &str,
) -> Option<String> {
    if last.is_some_and(|l| l.delivery == *d) {
        return None;
    }
    Some(
        serde_json::to_string(&CopyLine {
            v: 1,
            read_at: read_at.into(),
            read_by: read_by.into(),
            delivery: d.clone(),
        })
        .expect("serializes"),
    )
}

/// The newest copy of each delivery in a client folder, newest first.
pub fn read_copies(root: &Path, folder_rel: &str) -> Vec<CopyLine> {
    let dir = root
        .join(folder_rel)
        .join(vf_fs::cabinet::VIBE_DIR)
        .join(COPY_DIR);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<CopyLine> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| newest_copy(&e.path()))
        .collect();
    out.sort_by(|a, b| b.delivery.created_at.cmp(&a.delivery.created_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A delivery as the portal's tests answer it (Vibe-Portal deliveries.test.ts).
    pub(crate) const DELIVERY_JSON: &str = r#"{
      "id":"d1111111-1111-4111-8111-111111111111","clientId":"11111111-1111-4111-8111-111111111111",
      "taxYear":2025,"returnType":"1040","title":"Your 2025 tax return","status":"open",
      "locks":{"returnAfterLetters":true,"esignAfterLetters":true,"esignAfterFee":true},
      "assignedStaffId":null,"notify":true,"remindDays":0,"remindedAt":null,
      "returnFile":{"id":"f1","name":"2025 Form 1040 - Smith.pdf","sha256":"ab"},
      "esignLink":"https://esign.example-tax.test/s/abc","createdBy":"PC1\\kurt",
      "createdAt":"2026-11-24T12:00:00.000Z","completedAt":null,"cancelledAt":null,
      "steps":[
        {"kind":"letters","n":1,"name":"Letters","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"return","n":2,"name":"Tax return","present":true,"state":"done","after":[],"saidAt":null,"completedAt":"2026-11-24T13:00:00.000Z","completedBy":"Jordan","rounds":1},
        {"kind":"package","n":3,"name":"Package selection","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"fee","n":4,"name":"Fee","present":false,"state":"absent","after":[],"saidAt":null,"completedAt":null,"completedBy":null,"rounds":0},
        {"kind":"esign_8879","n":5,"name":"Form 8879","present":true,"state":"waiting","after":[],"saidAt":"2026-11-24T14:00:00.000Z","completedAt":null,"completedBy":null,"rounds":0}
      ],
      "events":[{"at":"2026-11-24T12:00:00.000Z","step":"","kind":"created","words":"Sent to the client","by":"PC1\\kurt"}]}"#;

    #[test]
    fn reads_a_delivery_and_says_where_it_is() {
        let d: Delivery = serde_json::from_str(DELIVERY_JSON).unwrap();
        assert_eq!(d.progress(), (1, 2));
        assert_eq!(
            d.line(),
            "2025 1040: 1 of 2 done · Form 8879 waiting for you to confirm"
        );
        assert_eq!(doc_type_of_step(ESIGN), Some("Delivery — 8879 record"));
        let steps = StepsIn {
            return_: true,
            esign8879: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&steps).unwrap(),
            serde_json::json!({ "letters": false, "return": true, "package": false, "fee": false, "esign8879": true })
        );
    }

    #[test]
    fn the_copy_gets_a_line_only_when_the_delivery_changed() {
        let t = tempfile::tempdir().unwrap();
        let d: Delivery = serde_json::from_str(DELIVERY_JSON).unwrap();
        let p = copy_path(t.path(), "Smith, John (1001)", &d.id);
        let first = copy_line(None, &d, "now", "PC1\\kurt").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("{first}\n")).unwrap();
        let last = newest_copy(&p);
        assert_eq!(copy_line(last.as_ref(), &d, "later", "PC2\\ann"), None);
        let mut done = d.clone();
        done.status = "complete".into();
        assert!(copy_line(last.as_ref(), &done, "later", "PC2\\ann").is_some());
        assert_eq!(read_copies(t.path(), "Smith, John (1001)").len(), 1);
    }
}

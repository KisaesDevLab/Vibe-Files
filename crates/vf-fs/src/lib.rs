//! Tree walker, rebuild, LAN lock protocol, trash, secure delete. Every document move calls the
//! vendored `filer-intake::commit` primitives; this crate never reimplements a move.

pub mod aliases;
pub mod cabinet;
pub mod clients;
pub mod content;
pub mod disposition;
pub mod docmeta;
pub mod doctypes;
pub mod dupes;
pub mod fileops;
pub mod hints;
pub mod holds;
pub mod import;
pub mod intake;
pub mod moveto;
pub mod newclient;
pub mod rebuild;
pub mod rename;
pub mod retention;
pub mod safeguards;
pub mod sweep;
pub mod trash;
pub mod versions;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use filer_core::clients::Client;
    use filer_intake::sentinel;
    use uuid::Uuid;

    /// The JSON example in docs/SENTINEL_V1.md.
    fn doc_example() -> serde_json::Value {
        let doc = include_str!("../../../docs/SENTINEL_V1.md");
        // Line-ending agnostic: a Windows checkout (autocrlf) has CRLF.
        let fence = doc.find("```json").expect("json block");
        let start = fence + doc[fence..].find('\n').expect("fence line end") + 1;
        let end = start + doc[start..].find("```").expect("block end");
        serde_json::from_str(&doc[start..end]).expect("example is valid JSON")
    }

    /// P00 done-when: SENTINEL_V1.md matches the vendored Filer `Sentinel`.
    #[test]
    fn sentinel_doc_matches_filer() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Anderson Farms LLC (1042)");
        std::fs::create_dir(&folder).unwrap();
        let client = Client {
            id: "1042".into(),
            uuid: Uuid::new_v4(),
            name: "Anderson Farms LLC".into(),
            extra: BTreeMap::new(),
        };
        sentinel::write_sentinel(tmp.path(), &folder, &client, Uuid::new_v4()).unwrap();
        let bytes = std::fs::read(sentinel::sentinel_path(&folder)).unwrap();
        let written: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let example = doc_example();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            v.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(keys(&example), keys(&written), "same fields");
        // The vendored reader accepts the documented example.
        let parsed: sentinel::Sentinel = serde_json::from_value(example).unwrap();
        assert_eq!(parsed.version, sentinel::SENTINEL_VERSION);
        assert_eq!(parsed.created_by, None);
        // And reads back what it wrote.
        assert!(sentinel::read_sentinel(&folder).unwrap().is_some());
    }
}

//! The update check (PHASES P13 step 6; QUESTIONS Q-30): the latest release of the public
//! KisaesDevLab/Vibe-Files repository on GitHub. Nothing is downloaded or installed: the app says
//! a newer version exists, where, and its published SHA-256 file. One GET, no data sent but the
//! request itself (CLAUDE.md principle 7: B2 and this check are the only network calls).

use object_store::ObjectStore;
use object_store::path::Path as ObjPath;

pub const API: &str = "https://api.github.com";
pub const LATEST: &str = "repos/KisaesDevLab/Vibe-Files/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// `0.2.0` (the tag without `v`).
    pub version: String,
    /// The release page.
    pub url: String,
}

/// `a` > `b` for dotted numeric versions (`0.10.0` > `0.9.3`); anything else compares as text.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Option<Vec<u64>> { s.split('.').map(|p| p.parse().ok()).collect() };
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x > y,
        _ => a > b,
    }
}

/// Reads the release JSON GitHub returns.
pub fn parse(json: &[u8]) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    if v.get("draft")?.as_bool()? || v.get("prerelease")?.as_bool()? {
        return None;
    }
    let tag = v.get("tag_name")?.as_str()?;
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        url: v.get("html_url")?.as_str()?.to_string(),
    })
}

/// The latest release when it is newer than `current`; `Ok(None)` when up to date.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let store = object_store::http::HttpBuilder::new()
        .with_url(API)
        .with_client_options(
            object_store::ClientOptions::new()
                .with_user_agent(
                    format!("vibe-files/{current}")
                        .parse()
                        .map_err(|_| "user agent".to_string())?,
                )
                .with_timeout(std::time::Duration::from_secs(15)),
        )
        .build()
        .map_err(|e| e.to_string())?;
    let body =
        crate::remote::block_on(async { store.get(&ObjPath::from(LATEST)).await?.bytes().await })
            .map_err(|e| e.to_string())?;
    Ok(parse(&body).filter(|r| newer(&r.version, current)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_release_json() {
        assert!(newer("0.10.0", "0.9.3"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        let j = br#"{"tag_name":"v0.2.0","html_url":"https://github.com/KisaesDevLab/Vibe-Files/releases/tag/v0.2.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            parse(j),
            Some(Release {
                version: "0.2.0".into(),
                url: "https://github.com/KisaesDevLab/Vibe-Files/releases/tag/v0.2.0".into()
            })
        );
        let pre = br#"{"tag_name":"v0.3.0","html_url":"x","draft":false,"prerelease":true}"#;
        assert_eq!(parse(pre), None);
    }
}

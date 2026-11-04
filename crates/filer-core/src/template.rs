//! Folder template (the tree created for a new client), token substitution, and the Windows
//! path rules every generated path must pass (CLAUDE.md §7).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Tokens every template can use. CSV extra columns add more, but may not reuse these names
/// (compared case-insensitively).
pub const BUILTIN_TOKENS: [&str; 12] = [
    "Client",
    "ID",
    "Year",
    "DocType",
    "Rest",
    "Ext",
    "Original",
    "EntityType",
    "Document",
    "K1Recipient",
    "YearEnd",
    "DatePrinted",
];
/// Built-in tokens allowed in the folder tree (no per-document tokens).
pub const TREE_TOKENS: [&str; 3] = ["Client", "ID", "Year"];
pub const TEMPLATE_FILE: &str = "template.json";
/// Longest full path the app will produce, in UTF-16 units.
pub const MAX_PATH_LEN: usize = 240;
const DEFAULT_TEMPLATE: &str = include_str!("../assets/default_template.json");
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

// ---------------------------------------------------------------------------
// Tokens and segments

/// Values for `{Token}` placeholders.
pub type Tokens = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Token(&'a str),
}

/// Splits `a{Client}b` into text and token pieces. Errors on unbalanced braces or bad names.
fn parse(template: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        match rest.find(['{', '}']) {
            None => {
                out.push(Piece::Text(rest));
                break;
            }
            Some(i) if rest.as_bytes()[i] == b'}' => {
                return Err(format!(
                    "\"{template}\" has a '}}' without a matching '{{'."
                ));
            }
            Some(i) => {
                if i > 0 {
                    out.push(Piece::Text(&rest[..i]));
                }
                let after = &rest[i + 1..];
                let Some(end) = after.find('}') else {
                    return Err(format!(
                        "\"{template}\" has a '{{' without a matching '}}'."
                    ));
                };
                let name = &after[..end];
                // `Copy.Client`, `Copy.<column>`: a dot joins the parts of a copy token (P07a).
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                {
                    return Err(format!("\"{template}\" has an invalid token {{{name}}}."));
                }
                out.push(Piece::Token(name));
                rest = &after[end + 1..];
            }
        }
    }
    Ok(out)
}

/// Token names used in `template`, in order.
pub fn tokens_in(template: &str) -> Result<Vec<String>, String> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Token(t) => Some(t.to_string()),
            Piece::Text(_) => None,
        })
        .collect())
}

/// Checks one path segment of a template (no `/`): braces, known tokens, and literal text that
/// would always break a Windows name.
pub fn validate_segment(segment: &str, allowed: &[&str]) -> Vec<String> {
    let mut errors = Vec::new();
    if segment.trim().is_empty() {
        errors.push("A folder or file name is empty.".to_string());
        return errors;
    }
    let pieces = match parse(segment) {
        Ok(p) => p,
        Err(e) => return vec![e],
    };
    for p in &pieces {
        match p {
            Piece::Token(t) if !allowed.contains(t) => {
                errors.push(format!("\"{segment}\" uses unknown token {{{t}}}."));
            }
            Piece::Text(text) if text.chars().any(is_illegal_char) => {
                errors.push(format!(
                    "\"{segment}\" contains a character Windows does not allow (< > : \" / \\ | ? *)."
                ));
            }
            _ => {}
        }
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        errors.push(format!("\"{segment}\" ends with a dot or space."));
    }
    if pieces.iter().all(|p| matches!(p, Piece::Text(_))) {
        if segment == "." || segment == ".." {
            errors.push(format!("\"{segment}\" is not a folder name."));
        } else if is_reserved(segment) {
            errors.push(format!("\"{segment}\" is a reserved Windows name."));
        }
    }
    errors
}

fn is_illegal_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

/// CON, PRN, AUX, NUL, COM1–9, LPT1–9, also with an extension (`CON.txt`).
fn is_reserved(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or("").trim_end();
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(base))
}

/// A file name typed by the user: illegal characters become `_`, trailing dots/spaces are
/// trimmed, and empty or reserved names are rejected.
pub fn safe_file_name(name: &str) -> Result<String, RenderError> {
    let out = sanitize_value(name.trim())
        .trim_end_matches(['.', ' '])
        .to_string();
    if out.is_empty() {
        return Err(RenderError::Empty);
    }
    if is_reserved(&out) {
        return Err(RenderError::Reserved(out));
    }
    Ok(out)
}

/// Makes a token value safe inside a file name: illegal characters become `_`.
pub fn sanitize_value(v: &str) -> String {
    v.chars()
        .map(|c| if is_illegal_char(c) { '_' } else { c })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("template error: {0}")]
    Template(String),
    #[error("no value for {{{0}}}")]
    MissingToken(String),
    #[error("a folder or file name came out empty")]
    Empty,
    #[error("\"{0}\" is a reserved Windows name")]
    Reserved(String),
    #[error("the path would be {0} characters long (limit {MAX_PATH_LEN})")]
    TooLong(usize),
}

/// Fills one segment. Values are sanitized; trailing dots and spaces are trimmed (Windows drops
/// them silently, which would make the name differ from what the log records).
pub fn render_segment(segment: &str, tokens: &Tokens) -> Result<String, RenderError> {
    let mut out = String::new();
    for p in parse(segment).map_err(RenderError::Template)? {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Token(t) => {
                let v = tokens
                    .get(t)
                    .ok_or_else(|| RenderError::MissingToken(t.to_string()))?;
                out.push_str(&sanitize_value(v));
            }
        }
    }
    let out = out.trim_end_matches(['.', ' ']).trim_start().to_string();
    if out.is_empty() {
        return Err(RenderError::Empty);
    }
    if is_reserved(&out) {
        return Err(RenderError::Reserved(out));
    }
    Ok(out)
}

/// Fills a `/`-separated template (e.g. `{Client}/Tax Returns/{Year}`) into a relative path.
#[cfg(test)]
pub fn render_relative(template: &str, tokens: &Tokens) -> Result<PathBuf, RenderError> {
    let mut path = PathBuf::new();
    for seg in template.split('/') {
        path.push(render_segment(seg, tokens)?);
    }
    Ok(path)
}

/// `base` joined with `rel`, rejected when longer than [`MAX_PATH_LEN`].
pub fn checked_join(base: &Path, rel: &Path) -> Result<PathBuf, RenderError> {
    let full = base.join(rel);
    let len = full.as_os_str().encode_wide_len();
    if len > MAX_PATH_LEN {
        return Err(RenderError::TooLong(len));
    }
    Ok(full)
}

trait WideLen {
    fn encode_wide_len(&self) -> usize;
}

impl WideLen for std::ffi::OsStr {
    fn encode_wide_len(&self) -> usize {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().count()
    }
}

/// Built-in client tokens: `{Client}` and `{ID}`, plus the client's CSV extras.
pub fn client_tokens(client: &crate::clients::Client) -> Tokens {
    let mut t = client.extra.clone();
    t.insert("Client".into(), client.name.clone());
    t.insert("ID".into(), client.id.clone());
    t
}

// ---------------------------------------------------------------------------
// Folder tree

/// One folder in the template. The root node is the client folder itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TemplateNode {
    pub name: String,
    #[serde(default)]
    pub children: Vec<TemplateNode>,
}

impl TemplateNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
        }
    }

    /// Uses `{Year}`: this node and everything under it are created only when a route needs it.
    pub fn is_year_node(&self) -> bool {
        self.name.contains("{Year}")
    }
}

pub fn default_tree() -> TemplateNode {
    serde_json::from_str(DEFAULT_TEMPLATE).expect("assets/default_template.json is valid")
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(TEMPLATE_FILE)
}

/// Loads `template.json`, writing the default on first run. A corrupt file gives the default and
/// `false` (do not overwrite it), plus the error for Setup to show.
pub fn load_or_seed(data_dir: &Path) -> (TemplateNode, bool, Option<String>) {
    let p = path(data_dir);
    match std::fs::read_to_string(&p) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(tree) => (tree, true, None),
            Err(e) => {
                tracing::error!(error = %e, "template.json is invalid; not saving over it");
                (
                    default_tree(),
                    false,
                    Some(format!(
                        "template.json could not be read ({e}); showing the default."
                    )),
                )
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let tree = default_tree();
            if let Err(e) = save(data_dir, &tree) {
                tracing::error!(error = %e, "seeding template.json failed");
            }
            (tree, true, None)
        }
        Err(e) => {
            tracing::error!(error = %e, "cannot read template.json");
            (
                default_tree(),
                false,
                Some(format!(
                    "template.json could not be read ({e}); showing the default."
                )),
            )
        }
    }
}

pub fn save(data_dir: &Path, tree: &TemplateNode) -> anyhow::Result<()> {
    crate::datafile::save_json(&path(data_dir), tree)
}

/// Problems with the tree. `extra_tokens` are the CSV extra-column tokens.
pub fn validate_tree(root: &TemplateNode, extra_tokens: &[String]) -> Vec<String> {
    let mut allowed: Vec<&str> = TREE_TOKENS.to_vec();
    allowed.extend(extra_tokens.iter().map(String::as_str));
    let mut errors = Vec::new();
    if root.is_year_node() {
        errors.push("The client folder name cannot use {Year}.".to_string());
    }
    fn walk(node: &TemplateNode, allowed: &[&str], errors: &mut Vec<String>) {
        errors.extend(validate_segment(&node.name, allowed));
        let mut seen: Vec<String> = Vec::new();
        for c in &node.children {
            let key = c.name.trim().to_lowercase();
            if seen.contains(&key) {
                errors.push(format!(
                    "\"{}\" has two sub-folders named \"{}\".",
                    node.name, c.name
                ));
            }
            seen.push(key);
            walk(c, allowed, errors);
        }
    }
    walk(root, &allowed, &mut errors);
    errors
}

/// Name of a client's folder (the rendered root node).
pub fn client_folder_name(
    root: &TemplateNode,
    client: &crate::clients::Client,
) -> Result<String, RenderError> {
    render_segment(&root.name, &client_tokens(client))
}

/// Folders to create for a new client, relative to the client root, parents first. Year nodes
/// (and everything under them) are left out; they are created when a document needs them.
pub fn initial_dirs(
    root: &TemplateNode,
    client: &crate::clients::Client,
) -> Result<Vec<PathBuf>, RenderError> {
    let tokens = client_tokens(client);
    let mut out = Vec::new();
    fn walk(
        node: &TemplateNode,
        parent: &Path,
        tokens: &Tokens,
        out: &mut Vec<PathBuf>,
    ) -> Result<(), RenderError> {
        if node.is_year_node() {
            return Ok(());
        }
        let here = parent.join(render_segment(&node.name, tokens)?);
        out.push(here.clone());
        for c in &node.children {
            walk(c, &here, tokens, out)?;
        }
        Ok(())
    }
    walk(root, Path::new(""), &tokens, &mut out)?;
    Ok(out)
}

/// Every folder of the template below the client folder as a destination template part
/// (`Income Tax/{Year}`), parents first (the rule builder's folder list, V10).
pub fn folder_templates(root: &TemplateNode) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(node: &TemplateNode, parent: &str, out: &mut Vec<String>) {
        let here = if parent.is_empty() {
            node.name.clone()
        } else {
            format!("{parent}/{}", node.name)
        };
        out.push(here.clone());
        for c in &node.children {
            walk(c, &here, out);
        }
    }
    for c in &root.children {
        walk(c, "", &mut out);
    }
    out
}

/// Every folder of the template below the client folder, relative to it, parents first (the
/// "Change folder…" list, V6). Year folders are rendered with `year`, or left out without one.
pub fn folder_list(
    root: &TemplateNode,
    client: &crate::clients::Client,
    year: Option<u16>,
) -> Vec<PathBuf> {
    let mut tokens = client_tokens(client);
    if let Some(y) = year {
        tokens.insert("Year".into(), y.to_string());
    }
    let mut out = Vec::new();
    fn walk(node: &TemplateNode, parent: &Path, tokens: &Tokens, out: &mut Vec<PathBuf>) {
        let Ok(name) = render_segment(&node.name, tokens) else {
            return;
        };
        let here = parent.join(name);
        out.push(here.clone());
        for c in &node.children {
            walk(c, &here, tokens, out);
        }
    }
    for c in &root.children {
        walk(c, Path::new(""), &tokens, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::Client;

    fn tokens(pairs: &[(&str, &str)]) -> Tokens {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn client(id: &str, name: &str) -> Client {
        Client {
            uuid: uuid::Uuid::new_v4(),
            id: id.into(),
            name: name.into(),
            extra: [("Partner".to_string(), "KW".to_string())].into(),
        }
    }

    #[test]
    fn parse_and_tokens() {
        assert_eq!(tokens_in("{Client} ({ID})").unwrap(), ["Client", "ID"]);
        assert!(tokens_in("plain").unwrap().is_empty());
        assert!(tokens_in("{Client").is_err());
        assert!(tokens_in("Client}").is_err());
        assert!(tokens_in("{}").is_err());
        assert!(tokens_in("{Cli ent}").is_err());
    }

    #[test]
    fn segment_validation() {
        let ok = |s: &str| validate_segment(s, &TREE_TOKENS).is_empty();
        assert!(ok("{Client} ({ID})"));
        assert!(ok("Tax Returns"));
        assert!(ok("{Year}"));
        assert!(!ok("{DocType}"), "not a tree token");
        assert!(!ok("Returns."));
        assert!(!ok("Returns "));
        assert!(!ok(".."));
        assert!(!ok("CON"));
        assert!(!ok("lpt1.txt"));
        assert!(!ok("a:b"));
        assert!(!ok("  "));
        assert!(ok("Console"), "only exact reserved names");
    }

    #[test]
    fn render_sanitizes_values() {
        let t = tokens(&[("Client", "Smith / Jones: LLC?"), ("ID", "1234")]);
        assert_eq!(
            render_segment("{Client} ({ID})", &t).unwrap(),
            "Smith _ Jones_ LLC_ (1234)"
        );
        // A value ending in a dot does not leave a trailing dot.
        let t = tokens(&[("Client", "Acme Inc.")]);
        assert_eq!(render_segment("{Client}", &t).unwrap(), "Acme Inc");
        // Values cannot smuggle in path parts.
        let t = tokens(&[("Client", "..")]);
        assert_eq!(render_segment("{Client}", &t), Err(RenderError::Empty));
        let t = tokens(&[("Client", "..\\..\\Windows")]);
        assert_eq!(render_segment("{Client}", &t).unwrap(), ".._.._Windows");
        let t = tokens(&[("Client", "nul")]);
        assert!(matches!(
            render_segment("{Client}", &t),
            Err(RenderError::Reserved(_))
        ));
        assert_eq!(
            render_segment("{Year}", &tokens(&[])),
            Err(RenderError::MissingToken("Year".into()))
        );
    }

    #[test]
    fn render_relative_and_length_limit() {
        let t = tokens(&[("Client", "Smith, John"), ("Year", "2025")]);
        assert_eq!(
            render_relative("{Client}/Tax Returns/{Year}", &t).unwrap(),
            PathBuf::from("Smith, John")
                .join("Tax Returns")
                .join("2025")
        );
        let base = Path::new(r"\\server\clients");
        assert!(checked_join(base, Path::new("short")).is_ok());
        let long = "x".repeat(MAX_PATH_LEN);
        assert!(matches!(
            checked_join(base, Path::new(&long)),
            Err(RenderError::TooLong(_))
        ));
    }

    #[test]
    fn default_tree_and_initial_dirs() {
        let tree = default_tree();
        assert_eq!(tree.name, "{Client} ({ID})");
        assert!(validate_tree(&tree, &[]).is_empty());
        let c = client("1234", "Smith, John");
        assert_eq!(client_folder_name(&tree, &c).unwrap(), "Smith, John (1234)");
        let dirs = initial_dirs(&tree, &c).unwrap();
        let top = PathBuf::from("Smith, John (1234)");
        assert_eq!(dirs[0], top);
        assert!(dirs.contains(&top.join("Permanent")));
        assert!(dirs.contains(&top.join("Income Tax")));
        assert!(dirs.contains(&top.join("Permanent").join("Stock Basis")));
        assert!(dirs.contains(&top.join("Other").join("Practice")));
        assert!(
            !dirs.iter().any(|d| d.to_string_lossy().contains("{Year}")),
            "year folders are lazy"
        );
        assert_eq!(dirs.len(), 14);
    }

    #[test]
    fn tree_validation() {
        let mut tree = TemplateNode::new("{Client} ({ID})");
        tree.children.push(TemplateNode::new("Docs"));
        tree.children.push(TemplateNode::new("docs"));
        tree.children.push(TemplateNode::new("By {Partner}"));
        let errs = validate_tree(&tree, &[]);
        assert!(errs.iter().any(|e| e.contains("two sub-folders")));
        assert!(errs.iter().any(|e| e.contains("unknown token {Partner}")));
        assert_eq!(validate_tree(&tree, &["Partner".into()]).len(), 1);
        assert!(!validate_tree(&TemplateNode::new("{Client} {Year}"), &[]).is_empty());
    }

    #[test]
    fn seed_and_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let (tree, ok, err) = load_or_seed(dir.path());
        assert!(ok && err.is_none());
        assert_eq!(tree, default_tree());
        assert!(path(dir.path()).exists());

        std::fs::write(path(dir.path()), "{ nope").unwrap();
        let (tree, ok, err) = load_or_seed(dir.path());
        assert!(!ok && err.is_some());
        assert_eq!(tree, default_tree());
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "{ nope");
    }
}

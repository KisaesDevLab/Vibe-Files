//! The list of documents in B2, by client, sent to the portal for its staff page (PA-80,
//! Vibe-Portal P90). The portal cannot read the firm's bucket (no server holds the sync key), so
//! Vibe Files sends what it knows to be there: each document's place in the client's folder, its
//! size and tax year. Names only, never content. Off until the firm ticks it in Admin › Portal.
//!
//! Each run asks the portal what it holds and sends only the clients whose list differs. Every
//! PC may run it from its own catalog: a list goes only when this PC's state of the client's
//! segment is at least the one the portal's list was made from, so a PC that has not pulled
//! does not put an older list back. File names go to no log.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::{Portal, PortalError, Result};

/// Said (once, by the caller) when the portal is older than Vibe-Portal P90.
pub const NEEDS_P90: &str =
    "The portal cannot show the documents in B2 yet: update it (sudo vibe upgrade portal).";

/// A document in B2, as the portal lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFile {
    pub doc_id: String,
    /// Below the client's folder, `/` between folders, with the file name.
    pub path: String,
    pub size: u64,
    pub tax_year: Option<i32>,
}

/// One client's documents in B2, as this PC's catalog has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientList {
    pub id: String,
    pub code: String,
    pub name: String,
    /// The client's manifest segment as last exchanged here.
    pub rev: u64,
    pub files: Vec<ListFile>,
}

impl ClientList {
    /// A mark of everything the portal shows of this list: the same mark, nothing to send. The
    /// files are taken in the order given (the caller's is by path).
    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        let mut part = |s: &str| {
            h.update((s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        part(&self.code);
        part(&self.name);
        for f in &self.files {
            part(&f.doc_id);
            part(&f.path);
            part(&f.size.to_string());
            part(&f.tax_year.map(|y| y.to_string()).unwrap_or_default());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// What this PC knows: the clients with a segment here, every client of the cabinet, and how
/// much of the bucket it has seen.
#[derive(Debug, Clone, Default)]
pub struct Local {
    pub head_rev: u64,
    pub clients: Vec<ClientList>,
    pub live: BTreeSet<String>,
    /// How the portal names this PC and Windows user ([`Portal::actor_name`]): the sender it
    /// keeps with each list.
    pub me: String,
}

/// What the portal holds for one client.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub id: String,
    #[serde(default)]
    pub rev: Option<u64>,
    #[serde(default)]
    pub head_rev: Option<u64>,
    #[serde(default)]
    pub hash: Option<String>,
    /// The PC and user whose list is kept.
    #[serde(default)]
    pub by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// The most documents the portal takes in one client's list.
    pub max: usize,
    /// The longest place of a document it takes (characters, as the portal counts them).
    #[serde(default)]
    pub max_path: Option<usize>,
    pub clients: Vec<Held>,
}

/// What a run has to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Send this client's list (its place in `Local::clients`).
    Send(usize),
    /// The client is gone from the cabinet: its list goes.
    Clear(String),
    /// More documents in B2 than the portal lists for one client: not sent (the client's name).
    TooMany(String),
    /// A document's place is longer than the portal lists: the client's list is not sent (the
    /// client's name).
    TooLong(String),
}

/// The steps that make the portal's lists the ones this PC has, leaving alone what a PC that
/// has seen more of the bucket sent.
pub fn plan(local: &Local, state: &State) -> Vec<Step> {
    let held: BTreeMap<&str, &Held> = state.clients.iter().map(|h| (h.id.as_str(), h)).collect();
    let fits = |c: &ClientList, i: usize| -> Step {
        if c.files.len() > state.max {
            Step::TooMany(c.name.clone())
        } else if state.max_path.is_some_and(|m| {
            // As the portal counts: UTF-16 units.
            c.files.iter().any(|f| f.path.encode_utf16().count() > m)
        }) {
            Step::TooLong(c.name.clone())
        } else {
            Step::Send(i)
        }
    };
    let mut steps = vec![];
    for (i, c) in local.clients.iter().enumerate() {
        match held.get(c.id.as_str()) {
            // Not on the portal's lists: only a client with something in B2 is put there.
            None if c.files.is_empty() => {}
            None => steps.push(fits(c, i)),
            Some(h) => {
                // Made from a newer segment than this PC has: not this PC's to change.
                if h.rev.is_some_and(|r| r > c.rev) {
                    continue;
                }
                // Cleared (the client left the cabinet) by a PC that has seen more of the
                // bucket than this one, which may only not have heard yet.
                if h.hash.is_none() && h.head_rev.is_some_and(|r| r > local.head_rev) {
                    continue;
                }
                // Cleared, and nothing of it in B2 here either: nothing to say.
                if h.hash.is_none() && c.files.is_empty() {
                    continue;
                }
                let mine = c.hash();
                if h.hash.as_deref() == Some(mine.as_str()) {
                    continue;
                }
                // The same segment, another PC's list kept, and this one differs (a detail one
                // of the two changed and has not pushed): the portal keeps the list whose mark
                // sorts last, so the two do not replace each other's at every send. This PC's
                // sorts first: not sent.
                if h.rev == Some(c.rev)
                    && h.by.as_deref() != Some(local.me.as_str())
                    && h.hash
                        .as_deref()
                        .is_some_and(|theirs| mine.as_str() < theirs)
                {
                    continue;
                }
                steps.push(fits(c, i));
            }
        }
    }
    let here: BTreeSet<&str> = local.clients.iter().map(|c| c.id.as_str()).collect();
    for h in &state.clients {
        // A client the cabinet no longer has here, as far as a PC that has seen at least as
        // much of the bucket as the list's sender can tell. One the cabinet has, with no
        // segment here: this PC has not pulled it yet.
        if h.hash.is_some()
            && !here.contains(h.id.as_str())
            && !local.live.contains(&h.id)
            && h.head_rev.unwrap_or(0) <= local.head_rev
        {
            steps.push(Step::Clear(h.id.clone()));
        }
    }
    steps
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListReport {
    /// Clients whose list the portal took.
    pub sent: usize,
    /// Clients gone from the cabinet whose list was cleared.
    pub cleared: usize,
    /// Lists the portal answered it keeps another PC's for (nothing changed there).
    pub kept: usize,
    /// Clients with more documents in B2 than the portal lists (their names).
    pub too_many: Vec<String>,
    /// Clients with a document whose place is longer than the portal lists (their names).
    pub too_long: Vec<String>,
    /// Clients whose list the portal refused (their names); the others still went.
    pub refused: Vec<String>,
    /// The firm unticked the setting, or stopped using the portal, while this ran: what this
    /// run had sent was taken off the portal again.
    pub stopped: bool,
    /// Why the run stopped early, in words for the screen.
    pub problem: Option<String>,
    /// The portal is older than Vibe-Portal P90: it cannot take the lists ([`NEEDS_P90`]).
    pub old_portal: bool,
    /// The lists this run took off the portal again (the setting was unticked while it ran)
    /// could not be removed: why.
    pub unremoved: Option<String>,
    /// What the portal takes for one client: documents, and the length of a document's place.
    pub max_docs: usize,
    pub max_path: Option<usize>,
}

impl Portal {
    /// What the portal holds of each client's list. A portal older than P90: [`NEEDS_P90`].
    pub fn cabinet_state(&self) -> Result<State> {
        self.get::<State>("/cabinet", &[]).map_err(|e| {
            if e.status() == Some(404) {
                PortalError::Local(NEEDS_P90.into())
            } else {
                e
            }
        })
    }

    /// Replaces one client's list; the portal makes the client when it has none. `false` when
    /// the portal kept the list it had (another PC's, made from as new a state of the bucket).
    pub fn put_cabinet_list(&self, c: &ClientList, head_rev: u64) -> Result<bool> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            kept: bool,
        }
        let body = serde_json::json!({
            "clientCode": c.code,
            "displayName": c.name,
            "rev": c.rev,
            "headRev": head_rev,
            "hash": c.hash(),
            "files": c.files,
        });
        self.put_json_large::<R>(&format!("/cabinet/{}", c.id), &body)
            .map(|r| !r.kept)
    }

    /// A client gone from the cabinet: its list leaves the portal (the client stays there).
    pub fn clear_cabinet_list(&self, client_id: &str, head_rev: u64) -> Result<()> {
        self.delete::<serde_json::Value>(&format!("/cabinet/{client_id}?headRev={head_rev}"))
            .map(|_| ())
    }

    /// Every list leaves the portal (the firm unticked the setting). Returns how many clients
    /// had one.
    pub fn clear_cabinet_lists(&self) -> Result<u64> {
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            clients: u64,
        }
        self.delete::<R>("/cabinet").map(|r| r.clients)
    }
}

/// The portal said no to this one client's list (what is in it, too large, not valid): the
/// other clients' lists can still go. Not the key, a portal asking to wait, or one that does
/// not know the route: those end the run.
fn only_this_client(e: &PortalError) -> bool {
    matches!(e.status(), Some(400 | 413 | 422))
}

/// Sends the portal the lists that differ from what it holds.
pub fn send(p: &Portal, local: &Local) -> ListReport {
    send_while(p, local, &mut || true)
}

/// [`send`], asking `wanted` before each call, and once more at the end, whether the lists are
/// still to be sent to this portal (the firm may untick the setting, change the portal or stop
/// using it, on any PC, while a long first send runs). When not: the run ends and takes every
/// list off the portal again, so none it sent after the firm's own removal stays there. A
/// client whose list the portal refuses is passed over and named; a portal that cannot be
/// reached, or does not take the key, ends the run (the next one goes on from there).
pub fn send_while(p: &Portal, local: &Local, wanted: &mut dyn FnMut() -> bool) -> ListReport {
    let mut rep = ListReport::default();
    let state = match p.cabinet_state() {
        Ok(s) => s,
        Err(e) => {
            rep.old_portal = e.message() == NEEDS_P90;
            rep.problem = Some(e.message());
            return rep;
        }
    };
    rep.max_docs = state.max;
    rep.max_path = state.max_path;
    let mut called = false;
    for step in plan(local, &state) {
        let name = match &step {
            Step::Send(i) => local.clients[*i].name.clone(),
            Step::Clear(_) => String::new(),
            Step::TooMany(name) => {
                rep.too_many.push(name.clone());
                continue;
            }
            Step::TooLong(name) => {
                rep.too_long.push(name.clone());
                continue;
            }
        };
        if !wanted() {
            rep.stopped = true;
            break;
        }
        called = true;
        match &step {
            Step::Send(i) => match p.put_cabinet_list(&local.clients[*i], local.head_rev) {
                Ok(true) => rep.sent += 1,
                Ok(false) => rep.kept += 1,
                Err(e) if only_this_client(&e) => rep.refused.push(name),
                Err(e) => {
                    rep.problem = Some(e.message());
                    break;
                }
            },
            Step::Clear(id) => match p.clear_cabinet_list(id, local.head_rev) {
                Ok(()) => rep.cleared += 1,
                // A list the portal does not have, or will not clear: nothing to do about it.
                Err(e) if only_this_client(&e) || e.status() == Some(404) => {}
                Err(e) => {
                    rep.problem = Some(e.message());
                    break;
                }
            },
            Step::TooMany(_) | Step::TooLong(_) => {}
        }
    }
    // Unticked (or the portal changed) while this ran: the firm's own removal may have come
    // before this run's last list.
    if rep.stopped || (called && !wanted()) {
        rep.stopped = true;
        if let Err(e) = p.clear_cabinet_lists()
            && e.status() != Some(404)
        {
            rep.unremoved = Some(e.message());
        }
    }
    tracing::info!(
        sent = rep.sent,
        cleared = rep.cleared,
        kept = rep.kept,
        too_many = rep.too_many.len(),
        too_long = rep.too_long.len(),
        refused = rep.refused.len(),
        stopped = rep.stopped,
        failed = rep.problem.is_some() || rep.unremoved.is_some(),
        "portal: lists of documents in B2"
    );
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, path: &str) -> ListFile {
        ListFile {
            doc_id: id.into(),
            path: path.into(),
            size: 10,
            tax_year: Some(2025),
        }
    }

    fn client(id: &str, rev: u64, files: Vec<ListFile>) -> ClientList {
        ClientList {
            id: id.into(),
            code: "1001".into(),
            name: format!("Client {id}"),
            rev,
            files,
        }
    }

    fn held(c: &ClientList, rev: u64, head: u64) -> Held {
        Held {
            id: c.id.clone(),
            rev: Some(rev),
            head_rev: Some(head),
            hash: Some(c.hash()),
            by: Some("PC1\\kurt".into()),
        }
    }

    fn local(head: u64, clients: Vec<ClientList>) -> Local {
        Local {
            head_rev: head,
            live: clients.iter().map(|c| c.id.clone()).collect(),
            clients,
            me: "PC1\\kurt".into(),
        }
    }

    #[test]
    fn the_mark_changes_with_anything_the_portal_shows() {
        let a = client("a", 1, vec![file("d1", "Tax/1040.pdf")]);
        let same = client("a", 9, vec![file("d1", "Tax/1040.pdf")]);
        // The segment's number is not part of it: the same list from a later segment is the same.
        assert_eq!(a.hash(), same.hash());
        let mut moved = a.clone();
        moved.files[0].path = "Tax/2025/1040.pdf".into();
        let mut renamed = a.clone();
        renamed.name = "Other".into();
        let mut year = a.clone();
        year.files[0].tax_year = None;
        let mut size = a.clone();
        size.files[0].size = 11;
        for other in [&moved, &renamed, &year, &size] {
            assert_ne!(a.hash(), other.hash());
        }
        // Where one text ends and the next begins is part of it.
        let ab = client("a", 1, vec![file("d1", "ab"), file("d2", "c")]);
        let mut split = ab.clone();
        split.files[0].path = "a".into();
        split.files[1].doc_id = "bd2".into();
        assert_ne!(ab.hash(), split.hash());
    }

    #[test]
    fn only_what_differs_is_sent() {
        let a = client("a", 3, vec![file("d1", "x.pdf")]);
        let b = client("b", 3, vec![file("d2", "y.pdf")]);
        let empty = client("c", 1, vec![]);
        let l = local(5, vec![a.clone(), b.clone(), empty]);
        // The portal has nothing: the clients with something in B2 go, the empty one does not.
        let none = State {
            max: 100,
            max_path: None,
            clients: vec![],
        };
        assert_eq!(plan(&l, &none), vec![Step::Send(0), Step::Send(1)]);
        // It has both as they are: nothing.
        let both = State {
            max: 100,
            max_path: None,
            clients: vec![held(&a, 3, 5), held(&b, 3, 5)],
        };
        assert!(plan(&l, &both).is_empty());
        // One changed here.
        let mut b2 = b.clone();
        b2.files.push(file("d3", "z.pdf"));
        b2.rev = 4;
        let l2 = local(6, vec![a.clone(), b2]);
        assert_eq!(plan(&l2, &both), vec![Step::Send(1)]);
        // Everything of a listed client left B2: its empty list goes, so the portal shows none.
        let l3 = local(6, vec![a.clone(), client("b", 4, vec![])]);
        assert_eq!(plan(&l3, &both), vec![Step::Send(1)]);
    }

    #[test]
    fn a_pc_that_has_seen_less_leaves_the_portals_list_alone() {
        let old = client("a", 3, vec![file("d1", "old.pdf")]);
        let newer = client("a", 4, vec![file("d1", "new.pdf")]);
        // The portal's list was made from segment 4 by a PC that had seen head 9.
        let state = State {
            max: 100,
            max_path: None,
            clients: vec![held(&newer, 4, 9)],
        };
        assert!(plan(&local(3, vec![old]), &state).is_empty());
        // The same segment with other details (a document's year changed here), and the list
        // kept is this PC's own: it goes.
        let mut same_rev = newer.clone();
        same_rev.files[0].tax_year = None;
        assert_eq!(
            plan(&local(9, vec![same_rev.clone()]), &state),
            vec![Step::Send(0)]
        );
        // The list kept is another PC's: only the list whose mark sorts last goes, so one of
        // the two PCs sends and the other does not, whichever looks.
        let other = |c: &ClientList| State {
            max: 100,
            max_path: None,
            clients: vec![Held {
                by: Some("PC2\\ann".into()),
                ..held(c, 4, 9)
            }],
        };
        let a_goes = !plan(&local(9, vec![same_rev.clone()]), &other(&newer)).is_empty();
        let b_goes = !plan(&local(9, vec![newer.clone()]), &other(&same_rev)).is_empty();
        assert!(a_goes != b_goes, "exactly one of the two sends");
        assert_eq!(a_goes, same_rev.hash() > newer.hash());
    }

    #[test]
    fn a_place_longer_than_the_portal_lists_holds_the_clients_list_back() {
        let long = client("a", 1, vec![file("d1", &"é".repeat(11))]);
        let state = |max_path| State {
            max: 100,
            max_path,
            clients: vec![],
        };
        assert_eq!(
            plan(&local(1, vec![long.clone()]), &state(Some(10))),
            vec![Step::TooLong("Client a".into())]
        );
        assert_eq!(
            plan(&local(1, vec![long.clone()]), &state(Some(11))),
            vec![Step::Send(0)]
        );
        // A portal that does not say: sent, and the portal decides.
        assert_eq!(
            plan(&local(1, vec![long]), &state(None)),
            vec![Step::Send(0)]
        );
    }

    #[test]
    fn a_client_gone_from_the_cabinet_loses_its_list() {
        let a = client("a", 3, vec![file("d1", "x.pdf")]);
        let gone = client("gone", 2, vec![file("d9", "g.pdf")]);
        let state = State {
            max: 100,
            max_path: None,
            clients: vec![held(&a, 3, 5), held(&gone, 2, 5)],
        };
        assert_eq!(
            plan(&local(5, vec![a.clone()]), &state),
            vec![Step::Clear("gone".into())]
        );
        // A PC that has seen less than the list's sender may only not know the client yet.
        assert!(plan(&local(4, vec![a.clone()]), &state).is_empty());
        // A client the cabinet has here with nothing exchanged yet is not gone.
        let mut not_pulled = local(5, vec![a]);
        not_pulled.live.insert("gone".into());
        assert!(plan(&not_pulled, &state).is_empty());
    }

    /// A list cleared because its client left the cabinet is kept by the portal with how
    /// much its clearer had seen: a PC that has seen less does not send it again, and nobody
    /// clears it twice.
    #[test]
    fn a_cleared_list_is_not_sent_back_by_a_pc_that_has_seen_less() {
        let a = client("a", 3, vec![file("d1", "x.pdf")]);
        let state = State {
            max: 100,
            max_path: None,
            clients: vec![Held {
                id: "a".into(),
                rev: None,
                head_rev: Some(9),
                hash: None,
                by: None,
            }],
        };
        assert!(plan(&local(8, vec![a.clone()]), &state).is_empty());
        // A PC that has seen as much has the client again (made anew, or restored): it goes.
        assert_eq!(plan(&local(9, vec![a]), &state), vec![Step::Send(0)]);
        // Gone here too: already cleared, nothing more to do.
        assert!(plan(&local(9, vec![]), &state).is_empty());
        // Here with nothing in B2: nothing to send either.
        assert!(plan(&local(9, vec![client("a", 4, vec![])]), &state).is_empty());
    }

    #[test]
    fn only_a_refusal_of_one_clients_list_lets_the_others_go() {
        let http = |status| PortalError::Http {
            status,
            code: "x".into(),
        };
        assert!(only_this_client(&http(400)));
        assert!(only_this_client(&http(413)));
        // The key, a caller not allowed, a route not there, the portal asking to wait, a
        // failing portal, no line: the run ends.
        assert!(!only_this_client(&http(401)));
        assert!(!only_this_client(&http(403)));
        assert!(!only_this_client(&http(404)));
        assert!(!only_this_client(&http(429)));
        assert!(!only_this_client(&http(500)));
        assert!(!only_this_client(&PortalError::Network("down".into())));
    }

    #[test]
    fn a_list_longer_than_the_portal_takes_is_not_sent() {
        let big = client(
            "a",
            1,
            vec![file("d1", "1"), file("d2", "2"), file("d3", "3")],
        );
        let state = State {
            max: 2,
            max_path: None,
            clients: vec![],
        };
        assert_eq!(
            plan(&local(1, vec![big]), &state),
            vec![Step::TooMany("Client a".into())]
        );
    }
}

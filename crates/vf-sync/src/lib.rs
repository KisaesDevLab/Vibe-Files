//! B2 two-way sync on the Vibe Files bucket layout (PHASES P11; docs/BUCKET_LAYOUT.md).
//!
//! - [`setup`]: the firm sync key (made once, wrapped by the sync passphrase, kept in the bucket's
//!   `cabinet.json` and in `_Vibe/keys/sync.json`, never in the clear) and [`Sync::unlock`].
//! - [`push`]: what changed since the last exchange (any change bumps `document.rev`, whatever
//!   wrote it) goes up: objects first, then segments, then `manifest/HEAD`.
//! - [`pull`]: segments that changed come down; files are written with Filer's move primitive
//!   through Transactions; unlocked collisions keep both (`(conflict from <device>)`).
//! - [`restore`]: a bare PC rebuilds the cabinet from the bucket and the passphrase.
//!
//! Every call blocks (the async runtime stays inside [`remote`]); the app runs them on a thread.
//! Logs carry counts and error kinds only: never paths, names or keys.

pub mod catalog;
pub mod crypto;
pub mod layout;
pub mod pull;
pub mod push;
pub mod remote;
pub mod restore;
pub mod setup;
pub mod update;

use std::path::Path;

pub use crypto::Keys;
pub use remote::Remote;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Remote(#[from] remote::RemoteError),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error("the sync passphrase is wrong")]
    WrongPassphrase,
    #[error("sync is not set up for this cabinet")]
    NotSetUp,
    #[error("the bucket holds a manifest this version cannot read: {0}")]
    Format(String),
    #[error("another workstation pushed at the same moment; try again")]
    Raced,
    #[error("{0}")]
    Local(String),
}

impl From<vf_core::Error> for SyncError {
    fn from(e: vf_core::Error) -> Self {
        SyncError::Local(e.to_string())
    }
}

impl From<vf_core::db::SqlError> for SyncError {
    fn from(e: vf_core::db::SqlError) -> Self {
        SyncError::Local(e.to_string())
    }
}

impl From<vf_security::dpapi::DpapiError> for SyncError {
    fn from(e: vf_security::dpapi::DpapiError) -> Self {
        SyncError::Local(e.to_string())
    }
}

impl From<String> for SyncError {
    fn from(e: String) -> Self {
        SyncError::Local(e)
    }
}

pub type Result<T> = std::result::Result<T, SyncError>;

/// An unlocked sync session for one cabinet on one workstation.
#[derive(Clone, Debug)]
pub struct Sync {
    pub remote: Remote,
    pub keys: Keys,
    pub cabinet_id: String,
    pub device_id: String,
    /// This computer's name, for conflict copies and the manifest.
    pub device_name: String,
}

/// This computer's name (`COMPUTERNAME`), for conflict copies and the manifest.
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this PC".into())
}

impl Sync {
    /// The session from the firm keys and this PC's bucket settings. The application key is
    /// sealed with the keys, so this needs no network.
    pub fn connect(
        keys: Keys,
        s3: &remote::S3Config,
        app_key_sealed: &str,
        cabinet_id: &str,
        device_id: &str,
    ) -> Result<Sync> {
        let app_key = setup::open_secret(&keys, app_key_sealed)?;
        let store = Remote::s3(s3, &app_key)?;
        Ok(Sync {
            remote: Remote::new(store, cabinet_id),
            keys,
            cabinet_id: cabinet_id.into(),
            device_id: device_id.into(),
            device_name: computer_name(),
        })
    }

    /// Opens the session with the sync passphrase (the wrapped key from `_Vibe/keys/sync.json`,
    /// else from the bucket).
    pub fn unlock(
        root: &Path,
        store: std::sync::Arc<dyn object_store::ObjectStore>,
        passphrase: &str,
        device_id: &str,
        device_name: &str,
    ) -> Result<Sync> {
        let cab = vf_fs::cabinet::read(root)?;
        let remote = Remote::new(store, &cab.cabinet_id);
        let keys = setup::unlock(root, &remote, &cab.cabinet_id, passphrase)?;
        Ok(Sync {
            remote,
            keys,
            cabinet_id: cab.cabinet_id,
            device_id: device_id.into(),
            device_name: device_name.into(),
        })
    }

    pub(crate) fn get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(body) = self.remote.get(key)? else {
            return Ok(None);
        };
        let plain = self.keys.open(&body, key)?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|e| SyncError::Format(format!("{key}: {e}")))
    }

    pub(crate) fn put_json<T: serde::Serialize>(&self, key: &str, v: &T) -> Result<()> {
        let plain = serde_json::to_vec(v).expect("serializes");
        self.remote.put(key, self.keys.seal(&plain, key))?;
        Ok(())
    }

    /// The bucket's HEAD (`None`: nothing pushed yet).
    pub fn head(&self) -> Result<Option<layout::Head>> {
        self.get_json(layout::HEAD)
    }
}

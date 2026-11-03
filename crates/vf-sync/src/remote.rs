//! The bucket, as blocking calls. `object_store` is async; its runtime lives here (one per
//! process) so nothing outside vf-sync sees tokio. B2 is reached through its S3-compatible API;
//! tests use `object_store`'s in-memory store with the same calls.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use futures::TryStreamExt;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, PutPayload, WriteMultipart};

/// Uploads at or above this go in parts (BUCKET_LAYOUT: multipart over 100 MB).
pub const MULTIPART_AT: usize = 100 * 1024 * 1024;
const PART: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("bucket: {0}")]
    Store(String),
}

impl From<object_store::Error> for RemoteError {
    fn from(e: object_store::Error) -> Self {
        RemoteError::Store(e.to_string())
    }
}

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("vf-sync-io")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs a future on the sync runtime (for the update check).
pub(crate) fn block_on<F: std::future::Future>(f: F) -> F::Output {
    rt().block_on(f)
}

/// Where a cabinet's bucket is (from `config.toml [sync]`; the application key is sealed).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct S3Config {
    /// `https://s3.us-west-004.backblazeb2.com`
    pub endpoint: String,
    /// `us-west-004`
    pub region: String,
    pub bucket: String,
    pub key_id: String,
}

/// One cabinet's objects: everything under `vf/<cabinet_id>/`.
#[derive(Clone)]
pub struct Remote {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    multipart_at: usize,
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Remote({})", self.prefix)
    }
}

impl Remote {
    pub fn new(store: Arc<dyn ObjectStore>, cabinet_id: &str) -> Remote {
        Remote {
            store,
            prefix: format!("vf/{cabinet_id}/"),
            multipart_at: MULTIPART_AT,
        }
    }

    /// An in-memory bucket (tests; shared between "PCs" by cloning the `Arc`).
    pub fn memory() -> Arc<dyn ObjectStore> {
        Arc::new(object_store::memory::InMemory::new())
    }

    /// B2 (or any S3 endpoint).
    pub fn s3(c: &S3Config, app_key: &str) -> Result<Arc<dyn ObjectStore>, RemoteError> {
        let s = object_store::aws::AmazonS3Builder::new()
            .with_endpoint(&c.endpoint)
            .with_region(&c.region)
            .with_bucket_name(&c.bucket)
            .with_access_key_id(&c.key_id)
            .with_secret_access_key(app_key)
            .with_virtual_hosted_style_request(false)
            .with_retry(object_store::RetryConfig {
                backoff: object_store::BackoffConfig::default(),
                max_retries: 5,
                retry_timeout: Duration::from_secs(120),
            })
            .build()?;
        Ok(Arc::new(s))
    }

    /// For tests: send parts from a lower size.
    pub fn with_multipart_at(mut self, n: usize) -> Remote {
        self.multipart_at = n;
        self
    }

    fn path(&self, key: &str) -> ObjPath {
        ObjPath::from(format!("{}{key}", self.prefix))
    }

    pub fn get(&self, key: &str) -> Result<Option<Bytes>, RemoteError> {
        let p = self.path(key);
        rt().block_on(async {
            match self.store.get(&p).await {
                Ok(r) => Ok(Some(r.bytes().await?)),
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    pub fn exists(&self, key: &str) -> Result<bool, RemoteError> {
        let p = self.path(key);
        rt().block_on(async {
            match self.store.head(&p).await {
                Ok(_) => Ok(true),
                Err(object_store::Error::NotFound { .. }) => Ok(false),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Writes `body` under `key` (in parts when large).
    pub fn put(&self, key: &str, body: Vec<u8>) -> Result<(), RemoteError> {
        let p = self.path(key);
        let large = body.len() >= self.multipart_at;
        rt().block_on(async {
            if large {
                let up = self.store.put_multipart(&p).await?;
                let mut w = WriteMultipart::new_with_chunk_size(up, PART.min(self.multipart_at));
                w.write(&body);
                w.finish().await?;
            } else {
                self.store.put(&p, PutPayload::from(body)).await?;
            }
            Ok(())
        })
    }

    pub fn delete(&self, key: &str) -> Result<(), RemoteError> {
        let p = self.path(key);
        rt().block_on(async {
            match self.store.delete(&p).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Keys under `under` from the bucket root (not the cabinet prefix).
    pub fn list_root(&self, under: &str) -> Result<Vec<String>, RemoteError> {
        let p = ObjPath::from(under);
        rt().block_on(async {
            let items: Vec<_> = self.store.list(Some(&p)).try_collect().await?;
            Ok(items.into_iter().map(|m| m.location.to_string()).collect())
        })
    }

    /// Keys (relative to the cabinet prefix) under `under`.
    pub fn list(&self, under: &str) -> Result<Vec<String>, RemoteError> {
        let p = self.path(under);
        rt().block_on(async {
            let items: Vec<_> = self.store.list(Some(&p)).try_collect().await?;
            Ok(items
                .into_iter()
                .map(|m| {
                    m.location
                        .as_ref()
                        .strip_prefix(&self.prefix)
                        .unwrap_or(m.location.as_ref())
                        .to_string()
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_list_delete_and_parts() {
        let r = Remote::new(Remote::memory(), "cab").with_multipart_at(1024);
        assert_eq!(r.get("a").unwrap(), None);
        r.put("objects/ab/x", b"small".to_vec()).unwrap();
        let big: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        r.put("objects/cd/y", big.clone()).unwrap();
        assert_eq!(r.get("objects/cd/y").unwrap().unwrap().as_ref(), &big[..]);
        assert!(r.exists("objects/ab/x").unwrap());
        let mut l = r.list("objects").unwrap();
        l.sort();
        assert_eq!(l, ["objects/ab/x", "objects/cd/y"]);
        r.delete("objects/ab/x").unwrap();
        r.delete("objects/ab/x").unwrap();
        assert!(!r.exists("objects/ab/x").unwrap());
    }
}

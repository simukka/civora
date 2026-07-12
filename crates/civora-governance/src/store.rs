//! Content-addressed blob store: the local data layer for patch-pack artifacts.
//!
//! A [`BlobStore`] is a git-style directory of raw blobs, each named by the
//! SHA-256 digest ([`Cid`]) of its own content: `root/<hex[0..2]>/<hex64>`. The
//! sharded first byte keeps any one directory small. Blobs are the artifacts a
//! proposal manifest references (source bundles, wasm, assets, ...); they are
//! fetched here, hash-verified, after a proposal is accepted, and nothing loads
//! or executes them — that is a later milestone.
//!
//! **Blob files carry no magic prefix**, a deliberate deviation from the
//! persisted-record house rule (ledger, keyfile). The filename *is*
//! `sha256(content)` and [`BlobStore::get`] re-hashes on read, which subsumes
//! everything a version byte would buy; a prefix would break the property that
//! `sha256sum <file>` equals the filename, the whole integrity story. Blobs are
//! public data, so files use default permissions (no `0o600`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cid::Cid;

/// Hard cap on a single blob, enforced on [`BlobStore::put`], on
/// [`BlobStore::get`], and by the fetch codec. Per-blob only: there is no
/// total-pack cap in v1 (voters see manifest cid counts before approving).
pub const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

/// A local content-addressed blob store rooted at a directory.
pub struct BlobStore {
    root: PathBuf,
}

/// Why a [`BlobStore`] operation failed.
#[derive(Debug)]
pub enum BlobStoreError {
    Io(io::Error),
    /// A blob exceeded [`MAX_BLOB_BYTES`] on put or on read.
    TooLarge {
        len: usize,
    },
    /// A stored blob did not hash to the cid it was requested under (a flipped
    /// byte on disk, or an over-cap file). Healing is manual deletion in v1.
    Corrupt {
        expected: Cid,
        actual: Cid,
    },
}

impl BlobStore {
    /// Open (creating it and its parents if absent) a store rooted at `root`.
    pub fn open(root: PathBuf) -> Result<BlobStore, BlobStoreError> {
        fs::create_dir_all(&root).map_err(BlobStoreError::Io)?;
        Ok(BlobStore { root })
    }

    /// Store `bytes`, returning their [`Cid`]. Idempotent: an identical blob
    /// already present is left untouched. Rejects blobs over [`MAX_BLOB_BYTES`].
    ///
    /// Writes to a pid-suffixed temp file in the shard directory, then renames
    /// (same-directory atomic). The pid suffix keeps concurrent writers to a
    /// shared store from colliding on the temp name, and because identical
    /// content yields identical bytes, a lost race is harmless.
    pub fn put(&self, bytes: &[u8]) -> Result<Cid, BlobStoreError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(BlobStoreError::TooLarge { len: bytes.len() });
        }
        let cid = Cid::of(bytes);
        let hex = cid.to_string();
        let shard = self.root.join(&hex[..2]);
        let path = shard.join(&hex);
        if path.exists() {
            return Ok(cid);
        }
        fs::create_dir_all(&shard).map_err(BlobStoreError::Io)?;
        let tmp = shard.join(format!("{hex}.tmp.{}", std::process::id()));
        fs::write(&tmp, bytes).map_err(BlobStoreError::Io)?;
        fs::rename(&tmp, &path).map_err(BlobStoreError::Io)?;
        Ok(cid)
    }

    /// Read the blob for `cid`. `Ok(None)` if absent; `Err(Corrupt)` if the
    /// stored bytes do not hash back to `cid` (or exceed the cap).
    pub fn get(&self, cid: &Cid) -> Result<Option<Vec<u8>>, BlobStoreError> {
        let bytes = match fs::read(self.path_of(cid)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(BlobStoreError::Io(err)),
        };
        let actual = Cid::of(&bytes);
        if bytes.len() > MAX_BLOB_BYTES || actual != *cid {
            return Err(BlobStoreError::Corrupt {
                expected: *cid,
                actual,
            });
        }
        Ok(Some(bytes))
    }

    /// Whether a blob for `cid` exists, without reading or verifying it.
    pub fn has(&self, cid: &Cid) -> bool {
        self.path_of(cid).exists()
    }

    /// `root/<hex[0..2]>/<hex64>` for `cid`.
    fn path_of(&self, cid: &Cid) -> PathBuf {
        let hex = cid.to_string();
        self.root.join(&hex[..2]).join(&hex)
    }
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobStoreError::Io(err) => write!(f, "blob store io error: {err}"),
            BlobStoreError::TooLarge { len } => {
                write!(
                    f,
                    "blob of {len} bytes exceeds the {MAX_BLOB_BYTES}-byte cap"
                )
            }
            BlobStoreError::Corrupt { expected, actual } => write!(
                f,
                "corrupt blob: content hashes to {actual} but was stored as {expected}"
            ),
        }
    }
}

impl std::error::Error for BlobStoreError {}

/// Convenience for callers that only need to know a path is inside the store.
impl BlobStore {
    /// The store's root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

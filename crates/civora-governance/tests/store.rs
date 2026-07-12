//! Content-addressed blob store round-trips and integrity checks.

use std::fs;

use civora_governance::{BlobStore, BlobStoreError, Cid, MAX_BLOB_BYTES};

fn store() -> (BlobStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::open(dir.path().join("blobs")).unwrap();
    (store, dir)
}

#[test]
fn put_get_round_trips_including_empty() {
    let (store, _dir) = store();
    for content in [&b""[..], b"civora", &vec![0xABu8; 100_000]] {
        let cid = store.put(content).unwrap();
        assert_eq!(cid, Cid::of(content));
        assert_eq!(store.get(&cid).unwrap().as_deref(), Some(content));
        assert!(store.has(&cid));
    }
}

#[test]
fn put_is_idempotent() {
    let (store, _dir) = store();
    let a = store.put(b"same bytes").unwrap();
    let b = store.put(b"same bytes").unwrap();
    assert_eq!(a, b);
    assert_eq!(store.get(&a).unwrap().as_deref(), Some(&b"same bytes"[..]));
}

#[test]
fn missing_blob_reads_as_none() {
    let (store, _dir) = store();
    let cid = Cid::of(b"never stored");
    assert_eq!(store.get(&cid).unwrap(), None);
    assert!(!store.has(&cid));
}

#[test]
fn over_cap_put_is_rejected() {
    let (store, _dir) = store();
    let oversized = vec![0u8; MAX_BLOB_BYTES + 1];
    match store.put(&oversized) {
        Err(BlobStoreError::TooLarge { len }) => assert_eq!(len, MAX_BLOB_BYTES + 1),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn flipped_byte_on_disk_is_corrupt() {
    let (store, dir) = store();
    let cid = store.put(b"integrity matters").unwrap();

    // Corrupt the stored file in place: name = sha256(content), so a flipped
    // byte no longer hashes back to the requested cid.
    let hex = cid.to_string();
    let path = dir.path().join("blobs").join(&hex[..2]).join(&hex);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    match store.get(&cid) {
        Err(BlobStoreError::Corrupt { expected, actual }) => {
            assert_eq!(expected, cid);
            assert_eq!(actual, Cid::of(&bytes));
            assert_ne!(actual, expected);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn layout_is_sharded_and_leaves_no_temp_litter() {
    let (store, dir) = store();
    let cid = store.put(b"shard me").unwrap();
    let hex = cid.to_string();
    let root = dir.path().join("blobs");

    // The blob lives at root/<hex[0..2]>/<hex64>.
    let shard = root.join(&hex[..2]);
    assert!(shard.join(&hex).is_file());

    // No `.tmp.` files survive a successful put.
    let leftovers: Vec<_> = fs::read_dir(&shard)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
}

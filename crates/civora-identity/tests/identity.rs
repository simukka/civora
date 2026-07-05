use civora_identity::{
    ActionLog, Identity, KeyfileError, VerifyError, load_encrypted, save_encrypted,
};
use civora_sim::{Action, BlockId, VoxelWorld, tick};

/// Deterministic test identity (not a secret).
fn identity() -> Identity {
    Identity::from_seed([7; 32])
}

fn place(pos: [i32; 3], block: BlockId) -> Action {
    Action::PlaceBlock { pos, block }
}

#[test]
fn sign_verify_round_trip() {
    let signed = identity().sign(place([1, 4, 1], BlockId::PLANK), 0);
    assert_eq!(signed.author, identity().player_id());
    assert_eq!(signed.seq, 0);
    signed.verify().expect("freshly signed action verifies");
}

#[test]
fn tampered_action_fails_verification() {
    let mut signed = identity().sign(place([1, 4, 1], BlockId::PLANK), 0);
    signed.action = place([2, 4, 1], BlockId::PLANK);
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));

    let mut signed = identity().sign(place([1, 4, 1], BlockId::PLANK), 0);
    signed.seq = 1;
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));
}

#[test]
fn tampered_signature_fails_verification() {
    let mut signed = identity().sign(Action::BreakBlock { pos: [0, 3, 0] }, 0);
    signed.signature[10] ^= 0x01;
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));
}

#[test]
fn reassigned_author_fails_verification() {
    // A valid key that didn't sign the payload must not verify.
    let mut signed = identity().sign(Action::BreakBlock { pos: [0, 3, 0] }, 0);
    signed.author = Identity::from_seed([9; 32]).player_id();
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));
}

#[test]
fn log_rejects_replayed_and_stale_seq() {
    let id = identity();
    let mut log = ActionLog::new();

    log.append(id.sign(place([1, 4, 1], BlockId::PLANK), 0))
        .expect("seq 0 accepted");
    log.append(id.sign(place([1, 5, 1], BlockId::PLANK), 1))
        .expect("seq 1 accepted");

    // Same seq again (even for a different action) is a replay.
    let replay = id.sign(place([9, 4, 9], BlockId::GLASS), 1);
    assert!(matches!(
        log.append(replay),
        Err(VerifyError::SeqReplay { seq: 1, .. })
    ));
    // Going backwards is too.
    let stale = id.sign(place([9, 5, 9], BlockId::GLASS), 0);
    assert!(matches!(
        log.append(stale),
        Err(VerifyError::SeqReplay { seq: 0, .. })
    ));
    assert_eq!(log.len(), 2);
}

#[test]
fn log_allows_seq_gaps_and_multiple_authors() {
    let a = identity();
    let b = Identity::from_seed([9; 32]);
    let mut log = ActionLog::new();

    log.append(a.sign(place([1, 4, 1], BlockId::PLANK), 0))
        .unwrap();
    log.append(a.sign(place([1, 5, 1], BlockId::PLANK), 7))
        .unwrap();
    // Independent sequence space per author.
    log.append(b.sign(place([2, 4, 2], BlockId::STONE), 0))
        .unwrap();
    assert_eq!(log.len(), 3);
}

#[test]
fn verified_replay_reproduces_content_hash() {
    let script = [
        place([1, 4, 1], BlockId::PLANK),
        Action::BreakBlock { pos: [1, 3, 1] },
        place([1, 3, 1], BlockId::GLASS),
        place([1, 4, 1], BlockId::STONE), // rejected: occupied
        Action::BreakBlock { pos: [-5, 3, 7] },
        place([-5, 3, 7], BlockId::DIRT),
    ];

    // Reference: apply the script directly to the sim.
    let mut direct = VoxelWorld::flat(1);
    tick::step(&mut direct, &script);

    // Signed path: log every action, then verify + replay onto a fresh world.
    let id = identity();
    let mut log = ActionLog::new();
    for (seq, action) in script.into_iter().enumerate() {
        log.append(id.sign(action, seq as u64)).unwrap();
    }
    let mut replayed = VoxelWorld::flat(1);
    let dirty = log.verify_and_replay(&mut replayed).expect("log verifies");

    assert_eq!(direct.content_hash(), replayed.content_hash());
    assert!(!dirty.is_empty());
}

#[test]
fn keyfile_round_trip_and_failure_modes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("identity.key");
    let id = identity();

    save_encrypted(&path, &id, "correct horse").unwrap();
    let loaded = load_encrypted(&path, "correct horse").unwrap();
    assert_eq!(loaded.player_id(), id.player_id());

    // Wrong passphrase fails AEAD authentication, never panics.
    assert!(matches!(
        load_encrypted(&path, "wrong horse"),
        Err(KeyfileError::WrongPassphrase)
    ));

    // A flipped ciphertext byte is indistinguishable from a wrong passphrase.
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        load_encrypted(&path, "correct horse"),
        Err(KeyfileError::WrongPassphrase)
    ));

    // Truncation / wrong magic are malformed, missing file is Io.
    std::fs::write(&path, &bytes[..10]).unwrap();
    assert!(matches!(
        load_encrypted(&path, "correct horse"),
        Err(KeyfileError::Malformed)
    ));
    assert!(matches!(
        load_encrypted(&dir.path().join("nope.key"), "x"),
        Err(KeyfileError::Io(_))
    ));
}

#[cfg(unix)]
#[test]
fn keyfile_is_owner_only_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.key");
    save_encrypted(&path, &identity(), "pass").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

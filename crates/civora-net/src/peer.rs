//! One key, two names: the player's Ed25519 identity key is also the libp2p
//! transport key, so a connection's authenticated [`PeerId`] *is* the
//! [`PlayerId`] that signs actions. No separate binding step is needed.

use civora_identity::PlayerId;
use libp2p::PeerId;
use libp2p::identity::Keypair;

/// Derive the libp2p transport keypair from the player identity seed.
///
/// Secret material in, secret material out; the seed copy is zeroized by
/// libp2p after the keypair is built.
pub fn keypair_from_seed(seed: [u8; 32]) -> Keypair {
    Keypair::ed25519_from_bytes(seed).expect("a 32-byte Ed25519 seed is always valid")
}

/// Recover the [`PlayerId`] embedded in a peer's id.
///
/// Ed25519 public keys are small enough that libp2p inlines them in the
/// PeerId multihash (identity code 0x00), so this needs no extra protocol.
/// Returns `None` for peers whose id is not an inlined Ed25519 key — those
/// are not Civora clients and should be disconnected.
pub fn player_id_of(peer: &PeerId) -> Option<PlayerId> {
    let multihash: &libp2p::multihash::Multihash<64> = peer.as_ref();
    if multihash.code() != 0x00 {
        return None;
    }
    let public = libp2p::identity::PublicKey::try_decode_protobuf(multihash.digest()).ok()?;
    Some(PlayerId(public.try_into_ed25519().ok()?.to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use civora_identity::Identity;

    #[test]
    fn peer_id_round_trips_to_player_id() {
        let identity = Identity::from_seed([7; 32]);
        let keypair = keypair_from_seed(identity.seed_bytes());
        let peer_id = keypair.public().to_peer_id();
        assert_eq!(player_id_of(&peer_id), Some(identity.player_id()));
    }
}

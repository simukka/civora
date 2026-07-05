use std::fmt;

use civora_sim::Action;
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;

use crate::signed::{SignedAction, signing_payload};

/// A player's public identity: the raw bytes of their Ed25519 verifying key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PlayerId(pub [u8; 32]);

impl PlayerId {
    /// Short display form (first 8 hex chars) for the HUD and logs.
    pub fn short(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// The local player's Ed25519 keypair.
///
/// The secret key never leaves this type except as the encrypted key file
/// (see [`crate::save_encrypted`]).
pub struct Identity {
    signing: SigningKey,
}

impl Identity {
    /// Generate a fresh keypair from the OS random number generator.
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    /// Rebuild the keypair from its 32-byte secret seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    /// The 32-byte secret seed of this keypair.
    ///
    /// Secret material: exists so the client can derive its libp2p transport
    /// keypair from the same key (PeerId == PlayerId). Never log, display,
    /// or transmit these bytes.
    pub fn seed_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId(self.signing.verifying_key().to_bytes())
    }

    /// Sign `action` as this player's `seq`-th action.
    ///
    /// `seq` must increase strictly per author; [`crate::ActionLog`] enforces
    /// this so a captured action cannot be replayed.
    pub fn sign(&self, action: Action, seq: u64) -> SignedAction {
        let author = self.player_id();
        let payload = signing_payload(&author, seq, &action);
        SignedAction {
            author,
            seq,
            action,
            signature: self.signing.sign(&payload).to_bytes(),
        }
    }
}

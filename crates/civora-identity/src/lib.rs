//! Player identity and signed actions for Civora.
//!
//! The seed of the Reality Kernel's signature verification and player-key
//! protection: an Ed25519 player identity, signed wrappers around sim
//! [`civora_sim::Action`]s, an append-only verified action log, and
//! passphrase-encrypted key storage. No interactive I/O lives here — the
//! client supplies passphrases as arguments.

mod identity;
mod keyfile;
mod log;
mod signed;

pub use identity::{Identity, PlayerId};
pub use keyfile::{KeyfileError, load_encrypted, save_encrypted};
pub use log::ActionLog;
pub use signed::{ACTION_SIGN_DOMAIN, SignedAction, VerifyError};

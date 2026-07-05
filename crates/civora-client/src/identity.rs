//! Local player identity: load or create the passphrase-encrypted key file.
//!
//! Runs in `main` before the Bevy app starts, so the passphrase prompt
//! happens on the terminal before the window opens. Set `CIVORA_PASSPHRASE`
//! to skip the prompt (scripted runs, or launches without a terminal).

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use civora_identity::{ActionLog, Identity, KeyfileError, load_encrypted, save_encrypted};

const PASSPHRASE_ENV: &str = "CIVORA_PASSPHRASE";
const MAX_ATTEMPTS: u32 = 3;

/// The local player's keypair and the next action sequence number.
#[derive(Resource)]
pub struct LocalIdentity {
    pub identity: Identity,
    pub next_seq: u64,
}

/// This session's append-only log of verified signed actions. In the P2P
/// milestone this is what gets gossiped to the cell committee.
#[derive(Resource, Default)]
pub struct SessionLog(pub ActionLog);

/// Load the identity key file, or create it on first run.
pub fn load_or_create() -> Result<Identity, String> {
    let path = key_path()?;
    let env_pass = std::env::var(PASSPHRASE_ENV).ok();
    if path.exists() {
        unlock(&path, env_pass)
    } else {
        create(&path, env_pass)
    }
}

fn key_path() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join("civora").join("identity.key"))
        .ok_or_else(|| "no OS config directory found for the identity key".into())
}

fn unlock(path: &Path, env_pass: Option<String>) -> Result<Identity, String> {
    if let Some(pass) = env_pass {
        return load_encrypted(path, &pass)
            .map_err(|err| format!("cannot unlock {}: {err}", path.display()));
    }
    println!("Civora identity: {}", path.display());
    for attempt in 1..=MAX_ATTEMPTS {
        let pass = prompt("Passphrase: ")?;
        match load_encrypted(path, &pass) {
            Ok(identity) => return Ok(identity),
            Err(KeyfileError::WrongPassphrase) if attempt < MAX_ATTEMPTS => {
                eprintln!("wrong passphrase, try again ({attempt}/{MAX_ATTEMPTS})");
            }
            Err(err) => return Err(format!("cannot unlock {}: {err}", path.display())),
        }
    }
    Err("giving up".into())
}

fn create(path: &Path, env_pass: Option<String>) -> Result<Identity, String> {
    let pass = match env_pass {
        Some(pass) => pass,
        None => {
            println!("No identity key yet; creating {}", path.display());
            let pass = prompt("New passphrase: ")?;
            if pass != prompt("Confirm passphrase: ")? {
                return Err("passphrases do not match".into());
            }
            pass
        }
    };
    if pass.is_empty() {
        return Err("passphrase must not be empty".into());
    }
    let identity = Identity::generate();
    save_encrypted(path, &identity, &pass)
        .map_err(|err| format!("cannot save {}: {err}", path.display()))?;
    Ok(identity)
}

fn prompt(label: &str) -> Result<String, String> {
    rpassword::prompt_password(label).map_err(|err| {
        format!(
            "cannot read passphrase: {err} (set {PASSPHRASE_ENV} when no terminal is available)"
        )
    })
}

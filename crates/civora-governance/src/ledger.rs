//! The accepted-proposal ledger: an append-only, disk-persisted record of every
//! proposal that reached finality.
//!
//! Entries are `(SignedProposal, SignedCertificate)` pairs. [`Ledger::append`]
//! is the sole gate — it re-verifies the proposal's signature and validity and
//! the certificate's internal consistency against the ledger's current rule
//! version (the [`civora_identity::ActionLog`] verify-on-append template), and
//! deduplicates by [`ProposalId`] so the **first valid certificate per proposal
//! wins**. Certificates for one proposal may differ byte-wise across peers
//! (different claimed rosters); the accepted *set* converges even though the
//! bytes need not.
//!
//! Persistence is a whole-file rewrite via a temp file and an atomic rename on
//! every append. Alpha ledgers are tiny, so this costs little and eliminates
//! the truncated-tail case entirely: [`Ledger::load`] is strictly rejecting,
//! rebuilding through [`Ledger::append`] so every signature is re-checked.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use civora_identity::VerifyError;

use crate::certificate::{CertificateError, GENESIS_RULE_VERSION, SignedCertificate};
use crate::proposal::{ProposalId, ProposalKind, ValidationError};
use crate::signed::SignedProposal;

/// Magic prefix identifying (and versioning) a Civora ledger file.
pub const LEDGER_MAGIC: &[u8; 8] = b"CIVLGR1\n";

/// One accepted proposal: the signed manifest and the certificate that carried
/// it to finality.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LedgerEntry {
    pub proposal: SignedProposal,
    pub certificate: SignedCertificate,
}

/// An append-only set of accepted proposals, keyed by [`ProposalId`].
#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct Ledger {
    entries: Vec<LedgerEntry>,
    ids: BTreeSet<ProposalId>,
}

impl Ledger {
    /// Verify `entry` and append it, returning `Ok(true)` if it was added or
    /// `Ok(false)` if its proposal is already in the ledger (a duplicate —
    /// first valid certificate wins).
    ///
    /// Verification re-runs the full governance gate: the proposal's signature
    /// and [`crate::Proposal::validate`], then the certificate's
    /// [`SignedCertificate::verify`] against this proposal and the ledger's
    /// current [`Ledger::rule_version`]. A certificate whose rule version does
    /// not match the ledger's is rejected here.
    pub fn append(&mut self, entry: LedgerEntry) -> Result<bool, LedgerError> {
        let id = entry.proposal.proposal_id();
        if self.ids.contains(&id) {
            return Ok(false);
        }
        entry.proposal.verify().map_err(LedgerError::BadProposal)?;
        entry
            .proposal
            .proposal
            .validate()
            .map_err(LedgerError::InvalidProposal)?;
        entry
            .certificate
            .verify(&entry.proposal.proposal, self.rule_version())
            .map_err(LedgerError::BadCertificate)?;
        self.ids.insert(id);
        self.entries.push(entry);
        Ok(true)
    }

    /// Whether a proposal with this id is already accepted.
    pub fn contains(&self, id: &ProposalId) -> bool {
        self.ids.contains(id)
    }

    /// The entry for `id`, if accepted.
    pub fn get(&self, id: &ProposalId) -> Option<&LedgerEntry> {
        self.entries
            .iter()
            .find(|e| e.proposal.proposal_id() == *id)
    }

    /// All accepted entries, in acceptance order.
    pub fn entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The governance rule version currently in effect: [`GENESIS_RULE_VERSION`]
    /// plus one for each accepted Governance-kind entry. This is the numbering
    /// [`crate::GovernanceChange`] promises the ledger assigns.
    pub fn rule_version(&self) -> u32 {
        GENESIS_RULE_VERSION
            + self
                .entries
                .iter()
                .filter(|e| e.proposal.proposal.kind == ProposalKind::Governance)
                .count() as u32
    }

    /// Load the ledger at `path`, rebuilding it through [`Ledger::append`] so
    /// every entry is fully re-verified.
    ///
    /// A missing file is an empty ledger. Any other failure — bad magic,
    /// truncation, trailing bytes, or an entry that fails verification — is an
    /// error: an alpha ledger is either wholly valid or rejected.
    pub fn load(path: &Path) -> Result<Ledger, LedgerFileError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Ledger::default()),
            Err(err) => return Err(LedgerFileError::Io(err)),
        };
        if bytes.len() < LEDGER_MAGIC.len() || &bytes[..LEDGER_MAGIC.len()] != LEDGER_MAGIC {
            return Err(LedgerFileError::BadMagic);
        }
        let mut rest = &bytes[LEDGER_MAGIC.len()..];
        let mut ledger = Ledger::default();
        while !rest.is_empty() {
            let (proposal, tail) =
                SignedProposal::decode(rest).ok_or(LedgerFileError::Malformed)?;
            let (certificate, tail) =
                SignedCertificate::decode(tail).ok_or(LedgerFileError::Malformed)?;
            rest = tail;
            ledger
                .append(LedgerEntry {
                    proposal,
                    certificate,
                })
                .map_err(LedgerFileError::Entry)?;
        }
        Ok(ledger)
    }

    /// Write the ledger to `path` via a temp file and an atomic rename, so a
    /// reader never sees a half-written ledger. Creates parent directories.
    pub fn save(&self, path: &Path) -> Result<(), LedgerFileError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(LEDGER_MAGIC);
        for entry in &self.entries {
            entry.proposal.encode(&mut bytes);
            entry.certificate.encode(&mut bytes);
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        // The temp file must share a directory with the target so the rename
        // stays within one filesystem (and is therefore atomic).
        let tmp = tmp_path(path);
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Why [`Ledger::append`] rejected an entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LedgerError {
    /// The proposal's author signature is invalid.
    BadProposal(VerifyError),
    /// The proposal is internally inconsistent.
    InvalidProposal(ValidationError),
    /// The certificate failed verification against the proposal and rule
    /// version.
    BadCertificate(CertificateError),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::BadProposal(err) => write!(f, "invalid proposal signature: {err}"),
            LedgerError::InvalidProposal(err) => write!(f, "invalid proposal: {err}"),
            LedgerError::BadCertificate(err) => write!(f, "invalid certificate: {err}"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Why loading or saving the ledger file failed.
#[derive(Debug)]
pub enum LedgerFileError {
    Io(io::Error),
    /// Wrong magic: not a Civora ledger file.
    BadMagic,
    /// An entry did not decode, was truncated, or left trailing bytes.
    Malformed,
    /// An entry decoded but failed verification during rebuild.
    Entry(LedgerError),
}

impl fmt::Display for LedgerFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerFileError::Io(err) => write!(f, "{err}"),
            LedgerFileError::BadMagic => write!(f, "not a Civora ledger file"),
            LedgerFileError::Malformed => write!(f, "malformed ledger file"),
            LedgerFileError::Entry(err) => write!(f, "ledger entry failed verification: {err}"),
        }
    }
}

impl std::error::Error for LedgerFileError {}

impl From<io::Error> for LedgerFileError {
    fn from(err: io::Error) -> Self {
        LedgerFileError::Io(err)
    }
}

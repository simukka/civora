//! Proposal manifests for Civora governance.
//!
//! The seed of "git commits become proposals; proposals become reality only
//! after signed player approval": the canonical [`Proposal`] manifest, its
//! content-derived [`ProposalId`], the [`SignedProposal`] wrapper under the
//! `civora.proposal.v1` signing domain, and the [`SignedVote`] ballot under
//! `civora.vote.v1`. It also defines wall-clock voting [`epoch_at`] windows.
//! Quorum evaluation, finality certificates, the accepted-proposal ledger,
//! and content-addressed patch packs are later milestones.

mod certificate;
mod cid;
mod epoch;
mod ledger;
mod proposal;
mod signed;
mod store;
mod vote;

pub use certificate::{
    CERT_FORMAT_VERSION, CERT_SIGN_DOMAIN, CertificateError, FinalityCertificate,
    GENESIS_RULE_VERSION, MAX_CERTIFICATE_BYTES, MAX_ROSTER_PLAYERS, MIN_QUORUM_BALLOTS,
    QuorumResult, ROSTER_ROOT_DOMAIN, SignedCertificate, VotePair, quorum_passes,
};
pub use cid::{CID_STRING_LEN, CIDV1_RAW_SHA256_PREFIX, Cid};
pub use epoch::{EPOCH_SECS, epoch_at};
pub use ledger::{LEDGER_MAGIC, Ledger, LedgerEntry, LedgerError, LedgerFileError};
pub use proposal::{
    GovernanceChange, MAX_CIDS_PER_LIST, MAX_PROPOSAL_BYTES, PROPOSAL_FORMAT_VERSION,
    PROPOSAL_ID_DOMAIN, Proposal, ProposalId, ProposalKind, RollbackPlan, ValidationError,
};
pub use signed::{PROPOSAL_SIGN_DOMAIN, SignedProposal};
pub use store::{BlobStore, BlobStoreError, MAX_BLOB_BYTES};
pub use vote::{
    MAX_SIGNED_VOTE_BYTES, SignedVote, VOTE_BYTES, VOTE_FORMAT_VERSION, VOTE_SIGN_DOMAIN, Vote,
    VoteChoice,
};

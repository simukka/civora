//! Finality certificates: the record that a proposal was accepted.
//!
//! At a voting window's close, any peer whose roster-filtered tally passes
//! [`quorum_passes`] assembles a **self-contained** [`FinalityCertificate`] —
//! it embeds the certifier's claimed eligible roster and the `(voter,
//! signature)` pairs behind the yes/no counts — wraps it in a certifier-signed
//! [`SignedCertificate`], and gossips it. Verification is *internal
//! consistency only*: every embedded vote must verify against a vote
//! reconstructed from the certificate itself, the certifier must sit in the
//! roster it claims, and the tally must clear quorum. A malicious certifier
//! claiming a tiny roster is a documented alpha limit — real eligibility and
//! anti-Sybil are later milestones.
//!
//! The certificate stores no id or root it could disagree with:
//! [`FinalityCertificate::eligible_roster_root`] is derived on demand, mirroring
//! the never-stored-always-derived [`ProposalId`] precedent.

use std::collections::{BTreeMap, BTreeSet};

use civora_identity::{Identity, PlayerId, verify_payload};
use sha2::{Digest, Sha256};

use crate::proposal::{Proposal, ProposalId, ProposalKind};
use crate::vote::{SignedVote, Vote, VoteChoice, signing_payload};

/// Domain-separation prefix for certificate signatures, so a certifier's
/// signature over a certificate can never be confused with one over an action,
/// a proposal, or a vote.
pub const CERT_SIGN_DOMAIN: &[u8] = b"civora.certificate.v1";

/// Domain prefix hashed in front of the roster ids to derive the roster root,
/// so the root can never collide with a hash of the same bytes in another role.
pub const ROSTER_ROOT_DOMAIN: &[u8] = b"civora.roster-root.v1";

/// Leading byte of every encoded certificate. Certificates are persisted
/// ledger records, so the version travels in the bytes themselves.
pub const CERT_FORMAT_VERSION: u8 = 1;

/// Cap on the eligible roster (and, since yes/no voters are a disjoint subset
/// of it, on each vote list). Guards decode against allocation bombs.
pub const MAX_ROSTER_PLAYERS: usize = 1024;

/// Documented upper bound on an encoded [`SignedCertificate`]. Worst case is a
/// full 1024-player roster (32 KiB) plus 1024 disjoint yes/no vote pairs at 96
/// bytes each (~98 KiB) plus headers — about 131 KiB, with headroom.
pub const MAX_CERTIFICATE_BYTES: usize = 144 * 1024;

/// The governance rule version in effect at genesis. The accepted-proposal
/// ledger increments from here as Governance-kind entries land.
pub const GENESIS_RULE_VERSION: u32 = 1;

/// Minimum ballots cast (yes + no) for any proposal to pass. One means an
/// offline solo player — whose roster is just themselves — self-accepts,
/// which is deliberate and good for demos.
pub const MIN_QUORUM_BALLOTS: usize = 1;

/// One embedded ballot in a certificate: the voter and their 64-byte signature.
/// The proposal id and choice are reconstructed at verify time, so they are not
/// stored here.
pub type VotePair = (PlayerId, [u8; 64]);

/// Outcome recorded by a certificate. Only acceptance is ever certified — the
/// ledger is accepted-only, so rejection certificates do not exist and
/// decoders reject any other byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuorumResult {
    Accepted,
}

impl QuorumResult {
    fn as_byte(self) -> u8 {
        match self {
            QuorumResult::Accepted => 1,
        }
    }

    fn from_byte(byte: u8) -> Option<QuorumResult> {
        match byte {
            1 => Some(QuorumResult::Accepted),
            _ => None,
        }
    }
}

/// Whether a tally clears the quorum for its proposal kind.
///
/// At least [`MIN_QUORUM_BALLOTS`] ballots must be cast. [`ProposalKind::Kernel`]
/// never passes (kernel changes are not hot-patchable in v1). Economy and
/// Governance require a two-thirds supermajority (`yes * 3 > roster * 2`); all
/// other kinds require a simple majority (`yes * 2 > roster`). Both bounds are
/// strict, so an exact tie or an exact two-thirds does not pass.
pub fn quorum_passes(kind: ProposalKind, roster: usize, yes: usize, no: usize) -> bool {
    if matches!(kind, ProposalKind::Kernel) {
        return false;
    }
    if yes + no < MIN_QUORUM_BALLOTS {
        return false;
    }
    match kind {
        ProposalKind::Economy | ProposalKind::Governance => yes * 3 > roster * 2,
        _ => yes * 2 > roster,
    }
}

/// A self-contained record that one proposal was accepted.
///
/// The `(voter, signature)` pairs carry no proposal id or choice: those are
/// reconstructed at verify time from `proposal_id` and the list a pair sits in,
/// so an embedded vote structurally cannot reference the wrong proposal or land
/// in the wrong list. Each pair is 96 bytes rather than a full 130-byte
/// [`SignedVote`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FinalityCertificate {
    pub proposal_id: ProposalId,
    pub governance_rule_version: u32,
    pub accepted_epoch: u64,
    pub quorum_result: QuorumResult,
    /// Strictly ascending, non-empty, at most [`MAX_ROSTER_PLAYERS`].
    pub eligible_roster: Vec<PlayerId>,
    /// `(voter, signature)` pairs, strictly ascending by voter.
    pub yes_votes: Vec<VotePair>,
    /// `(voter, signature)` pairs, strictly ascending by voter.
    pub no_votes: Vec<VotePair>,
}

fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    (bytes.len() >= n).then(|| bytes.split_at(n))
}

fn players_ascending(players: &[PlayerId]) -> bool {
    players.windows(2).all(|w| w[0] < w[1])
}

fn votes_ascending(votes: &[VotePair]) -> bool {
    votes.windows(2).all(|w| w[0].0 < w[1].0)
}

/// Append `count (u16 LE) || count x 32-byte ids`, asserting the list is
/// canonical (encoders must not produce bytes decoders would reject).
fn encode_players(players: &[PlayerId], out: &mut Vec<u8>) {
    assert!(players.len() <= MAX_ROSTER_PLAYERS, "player list over cap");
    assert!(
        players_ascending(players),
        "player list not strictly ascending"
    );
    out.extend_from_slice(&(players.len() as u16).to_le_bytes());
    for id in players {
        out.extend_from_slice(&id.0);
    }
}

/// Decode one player list, rejecting counts over the cap and non-ascending
/// contents.
fn decode_players(bytes: &[u8]) -> Option<(Vec<PlayerId>, &[u8])> {
    let (count, rest) = take(bytes, 2)?;
    let count = u16::from_le_bytes(count.try_into().unwrap()) as usize;
    if count > MAX_ROSTER_PLAYERS {
        return None;
    }
    let (raw, rest) = take(rest, count * 32)?;
    let players: Vec<PlayerId> = raw
        .chunks_exact(32)
        .map(|c| PlayerId(c.try_into().unwrap()))
        .collect();
    players_ascending(&players).then_some((players, rest))
}

/// Append `count (u16 LE) || count x (voter (32) || signature (64))`, asserting
/// the list is canonical.
fn encode_votes(votes: &[VotePair], out: &mut Vec<u8>) {
    assert!(votes.len() <= MAX_ROSTER_PLAYERS, "vote list over cap");
    assert!(
        votes_ascending(votes),
        "vote list not strictly ascending by voter"
    );
    out.extend_from_slice(&(votes.len() as u16).to_le_bytes());
    for (voter, sig) in votes {
        out.extend_from_slice(&voter.0);
        out.extend_from_slice(sig);
    }
}

/// Decode one vote-pair list, rejecting counts over the cap and non-ascending
/// contents.
fn decode_votes(bytes: &[u8]) -> Option<(Vec<VotePair>, &[u8])> {
    let (count, rest) = take(bytes, 2)?;
    let count = u16::from_le_bytes(count.try_into().unwrap()) as usize;
    if count > MAX_ROSTER_PLAYERS {
        return None;
    }
    let (raw, rest) = take(rest, count * 96)?;
    let votes: Vec<VotePair> = raw
        .chunks_exact(96)
        .map(|c| {
            (
                PlayerId(c[..32].try_into().unwrap()),
                c[32..].try_into().unwrap(),
            )
        })
        .collect();
    votes_ascending(&votes).then_some((votes, rest))
}

impl FinalityCertificate {
    /// Append the canonical encoding of this certificate to `out`:
    ///
    /// `version (u8 = 1) || proposal_id (32) || rule_version (u32 LE) ||
    /// accepted_epoch (u64 LE) || quorum byte (u8) ||
    /// n_roster (u16 LE) + ids (n x 32, strictly ascending) ||
    /// n_yes (u16 LE) + (voter (32) || sig (64))* (strictly ascending by voter) ||
    /// n_no (u16 LE) + likewise`
    ///
    /// Exactly one encoding exists per certificate: the roster and both vote
    /// lists are asserted canonical here and rejected non-canonical by
    /// [`FinalityCertificate::decode`].
    pub fn encode(&self, out: &mut Vec<u8>) {
        assert!(!self.eligible_roster.is_empty(), "roster must be non-empty");
        out.push(CERT_FORMAT_VERSION);
        out.extend_from_slice(&self.proposal_id.0);
        out.extend_from_slice(&self.governance_rule_version.to_le_bytes());
        out.extend_from_slice(&self.accepted_epoch.to_le_bytes());
        out.push(self.quorum_result.as_byte());
        encode_players(&self.eligible_roster, out);
        encode_votes(&self.yes_votes, out);
        encode_votes(&self.no_votes, out);
    }

    /// Decode one certificate from the front of `bytes`, returning it and the
    /// remaining bytes.
    ///
    /// Returns `None` for an unknown version or quorum byte, truncation, an
    /// empty roster, any list over [`MAX_ROSTER_PLAYERS`], or a non-ascending
    /// roster or vote list. Decoding checks structure only — call
    /// [`SignedCertificate::verify`] before trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(FinalityCertificate, &[u8])> {
        let (version, rest) = take(bytes, 1)?;
        if version[0] != CERT_FORMAT_VERSION {
            return None;
        }
        let (proposal_id, rest) = take(rest, 32)?;
        let (rule_version, rest) = take(rest, 4)?;
        let (accepted_epoch, rest) = take(rest, 8)?;
        let (quorum, rest) = take(rest, 1)?;
        let quorum_result = QuorumResult::from_byte(quorum[0])?;
        let (eligible_roster, rest) = decode_players(rest)?;
        if eligible_roster.is_empty() {
            return None;
        }
        let (yes_votes, rest) = decode_votes(rest)?;
        let (no_votes, rest) = decode_votes(rest)?;
        Some((
            FinalityCertificate {
                proposal_id: ProposalId(proposal_id.try_into().unwrap()),
                governance_rule_version: u32::from_le_bytes(rule_version.try_into().unwrap()),
                accepted_epoch: u64::from_le_bytes(accepted_epoch.try_into().unwrap()),
                quorum_result,
                eligible_roster,
                yes_votes,
                no_votes,
            },
            rest,
        ))
    }

    /// Decode exactly one certificate, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<FinalityCertificate> {
        match Self::decode(bytes)? {
            (certificate, []) => Some(certificate),
            _ => None,
        }
    }

    /// The roster root PLAN.md's certificate shape names: SHA-256 over
    /// `ROSTER_ROOT_DOMAIN || ids`. Derived on demand so it can never disagree
    /// with the stored roster.
    pub fn eligible_roster_root(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(ROSTER_ROOT_DOMAIN);
        for id in &self.eligible_roster {
            hasher.update(id.0);
        }
        hasher.finalize().into()
    }
}

/// A [`FinalityCertificate`] bound to its certifier by an Ed25519 signature.
///
/// The roster claim is the trust-sensitive part of the certificate, so it must
/// be attributable: the certifier signs, and [`SignedCertificate::verify`]
/// requires the certifier to sit in the roster it claims. The certifier travels
/// alongside the encoding rather than inside it (like
/// [`civora_identity::SignedAction`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SignedCertificate {
    pub certifier: PlayerId,
    pub certificate: FinalityCertificate,
    pub signature: [u8; 64],
}

fn signing_payload_for(certifier: &PlayerId, certificate: &FinalityCertificate) -> Vec<u8> {
    let mut payload = Vec::with_capacity(CERT_SIGN_DOMAIN.len() + 32 + 256);
    payload.extend_from_slice(CERT_SIGN_DOMAIN);
    payload.extend_from_slice(&certifier.0);
    certificate.encode(&mut payload);
    payload
}

impl SignedCertificate {
    /// Sign `certificate` as `identity`.
    ///
    /// Panics if the signer is not in `certificate.eligible_roster`: a
    /// certifier vouching for a roster that excludes themselves is always a
    /// caller bug (and [`SignedCertificate::verify`] would reject it).
    pub fn sign(identity: &Identity, certificate: FinalityCertificate) -> SignedCertificate {
        let certifier = identity.player_id();
        assert!(
            certificate.eligible_roster.contains(&certifier),
            "certifier is not in the certificate's eligible roster"
        );
        let signature = identity.sign_payload(&signing_payload_for(&certifier, &certificate));
        SignedCertificate {
            certifier,
            certificate,
            signature,
        }
    }

    /// Assemble and sign a certificate for `proposal` if the roster-filtered
    /// tally passes quorum, else `None`.
    ///
    /// `ballots` is filtered to `roster` (ballots from outside the claimed
    /// roster do not count); the resulting yes/no counts run through
    /// [`quorum_passes`]. The roster is canonicalized (sorted, deduplicated)
    /// before it is embedded, so callers may pass it in any order. Ballot
    /// signatures are trusted here — they were verified at the store gate and
    /// are re-verified by any receiver through [`SignedCertificate::verify`].
    pub fn certify(
        identity: &Identity,
        proposal: &Proposal,
        roster: &[PlayerId],
        ballots: &BTreeMap<PlayerId, SignedVote>,
        rule_version: u32,
        accepted_epoch: u64,
    ) -> Option<SignedCertificate> {
        let roster_set: BTreeSet<PlayerId> = roster.iter().copied().collect();
        if roster_set.is_empty() || roster_set.len() > MAX_ROSTER_PLAYERS {
            return None;
        }
        let mut yes_votes = Vec::new();
        let mut no_votes = Vec::new();
        // BTreeMap iteration is ascending by voter, so both lists come out
        // strictly ascending without an explicit sort.
        for (voter, signed) in ballots {
            if !roster_set.contains(voter) {
                continue;
            }
            match signed.vote.choice {
                VoteChoice::Yes => yes_votes.push((*voter, signed.signature)),
                VoteChoice::No => no_votes.push((*voter, signed.signature)),
            }
        }
        let eligible_roster: Vec<PlayerId> = roster_set.into_iter().collect();
        if !quorum_passes(
            proposal.kind,
            eligible_roster.len(),
            yes_votes.len(),
            no_votes.len(),
        ) {
            return None;
        }
        let certificate = FinalityCertificate {
            proposal_id: proposal.id(),
            governance_rule_version: rule_version,
            accepted_epoch,
            quorum_result: QuorumResult::Accepted,
            eligible_roster,
            yes_votes,
            no_votes,
        };
        Some(SignedCertificate::sign(identity, certificate))
    }

    /// Append the canonical wire encoding of this signed certificate to `out`:
    /// `cert_len (u32 LE) || cert bytes || certifier (32) || signature (64)`.
    ///
    /// The length prefix makes the encoding self-delimiting so lists of signed
    /// certificates decode by iteration.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut cert = Vec::with_capacity(256);
        self.certificate.encode(&mut cert);
        out.extend_from_slice(&(cert.len() as u32).to_le_bytes());
        out.extend_from_slice(&cert);
        out.extend_from_slice(&self.certifier.0);
        out.extend_from_slice(&self.signature);
    }

    /// Decode one signed certificate from the front of `bytes`, returning it
    /// and the remaining bytes.
    ///
    /// Returns `None` for truncation, a length prefix over
    /// [`MAX_CERTIFICATE_BYTES`], an inner certificate that
    /// [`FinalityCertificate::decode`] rejects, or inner bytes the certificate
    /// does not fill exactly. Decoding checks structure only — call
    /// [`SignedCertificate::verify`] before trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(SignedCertificate, &[u8])> {
        let (len, rest) = take(bytes, 4)?;
        let cert_len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        if cert_len > MAX_CERTIFICATE_BYTES {
            return None;
        }
        let (cert, rest) = take(rest, cert_len)?;
        let (certifier, rest) = take(rest, 32)?;
        let (signature, rest) = take(rest, 64)?;
        Some((
            SignedCertificate {
                certifier: PlayerId(certifier.try_into().unwrap()),
                certificate: FinalityCertificate::decode_exact(cert)?,
                signature: signature.try_into().unwrap(),
            },
            rest,
        ))
    }

    /// Decode exactly one signed certificate, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<SignedCertificate> {
        match Self::decode(bytes)? {
            (signed, []) => Some(signed),
            _ => None,
        }
    }

    /// Verify the certificate is internally consistent against `proposal` and
    /// the ledger's current `rule_version`.
    ///
    /// Checks, in order: the certificate names this proposal; the certifier's
    /// signature is valid; the certifier sits in the claimed roster; every
    /// yes/no voter sits in the roster; the yes and no sets are disjoint; each
    /// embedded pair verifies as a [`SignedVote`] over the reconstructed
    /// [`Vote`]; the tally clears quorum; the accepted epoch is at least the
    /// proposal's activation epoch; and the rule version matches. Any failure
    /// returns a distinct [`CertificateError`].
    pub fn verify(&self, proposal: &Proposal, rule_version: u32) -> Result<(), CertificateError> {
        let cert = &self.certificate;
        if proposal.id() != cert.proposal_id {
            return Err(CertificateError::ProposalMismatch);
        }
        verify_payload(
            &self.certifier,
            &signing_payload_for(&self.certifier, cert),
            &self.signature,
        )
        .map_err(|_| CertificateError::BadCertifierSignature)?;

        let roster: BTreeSet<PlayerId> = cert.eligible_roster.iter().copied().collect();
        if !roster.contains(&self.certifier) {
            return Err(CertificateError::CertifierNotInRoster);
        }
        let yes: BTreeSet<PlayerId> = cert.yes_votes.iter().map(|(v, _)| *v).collect();
        let no: BTreeSet<PlayerId> = cert.no_votes.iter().map(|(v, _)| *v).collect();
        if yes.iter().chain(no.iter()).any(|v| !roster.contains(v)) {
            return Err(CertificateError::VoterNotInRoster);
        }
        if yes.intersection(&no).next().is_some() {
            return Err(CertificateError::VoterInBothLists);
        }
        for (voter, sig, choice) in cert
            .yes_votes
            .iter()
            .map(|(v, s)| (v, s, VoteChoice::Yes))
            .chain(cert.no_votes.iter().map(|(v, s)| (v, s, VoteChoice::No)))
        {
            let vote = Vote {
                proposal_id: cert.proposal_id,
                voter: *voter,
                choice,
            };
            verify_payload(voter, &signing_payload(&vote), sig)
                .map_err(|_| CertificateError::BadVoteSignature)?;
        }
        if !quorum_passes(
            proposal.kind,
            cert.eligible_roster.len(),
            cert.yes_votes.len(),
            cert.no_votes.len(),
        ) {
            return Err(CertificateError::QuorumNotMet);
        }
        if cert.accepted_epoch < proposal.activation_epoch {
            return Err(CertificateError::EpochTooEarly);
        }
        if cert.governance_rule_version != rule_version {
            return Err(CertificateError::RuleVersionMismatch);
        }
        Ok(())
    }
}

/// Why a certificate failed [`SignedCertificate::verify`]. One variant per
/// rejection so callers (and tests) can pin exactly what went wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CertificateError {
    /// The certificate's `proposal_id` does not match the given proposal.
    ProposalMismatch,
    /// The certifier's signature does not match the certificate.
    BadCertifierSignature,
    /// The certifier is not a member of the roster it claims.
    CertifierNotInRoster,
    /// A yes or no voter is not a member of the claimed roster.
    VoterNotInRoster,
    /// A voter appears in both the yes and no lists.
    VoterInBothLists,
    /// An embedded `(voter, signature)` pair does not verify as that voter's
    /// ballot over the reconstructed vote.
    BadVoteSignature,
    /// The tally does not clear quorum for the proposal's kind.
    QuorumNotMet,
    /// The accepted epoch precedes the proposal's activation epoch (a
    /// certificate assembled before the voting window closed).
    EpochTooEarly,
    /// The certificate's rule version does not match the ledger's.
    RuleVersionMismatch,
}

impl std::fmt::Display for CertificateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertificateError::ProposalMismatch => {
                write!(f, "certificate names a different proposal")
            }
            CertificateError::BadCertifierSignature => {
                write!(f, "certifier signature does not match certificate")
            }
            CertificateError::CertifierNotInRoster => {
                write!(f, "certifier is not in the claimed roster")
            }
            CertificateError::VoterNotInRoster => write!(f, "a voter is not in the claimed roster"),
            CertificateError::VoterInBothLists => write!(f, "a voter appears in both vote lists"),
            CertificateError::BadVoteSignature => {
                write!(f, "an embedded vote signature is invalid")
            }
            CertificateError::QuorumNotMet => write!(f, "tally does not clear quorum"),
            CertificateError::EpochTooEarly => {
                write!(f, "accepted epoch precedes the proposal's activation epoch")
            }
            CertificateError::RuleVersionMismatch => {
                write!(f, "certificate rule version does not match the ledger")
            }
        }
    }
}

impl std::error::Error for CertificateError {}

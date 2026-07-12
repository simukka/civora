//! Composite libp2p behaviour: gossipsub (live actions + state beacons),
//! mDNS (LAN lobby discovery), and request-response (snapshot sync).

use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{gossipsub, mdns, request_response};

use crate::codec::{FETCH_PROTOCOL, FetchCodec, SYNC_PROTOCOL, SyncCodec};

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub gossipsub: gossipsub::Behaviour,
    /// Disabled for direct-dial sessions and tests (CI runners often filter
    /// multicast).
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub sync: request_response::Behaviour<SyncCodec>,
    /// Content-addressed blob fetch (`/civora/fetch/1`); a separate behaviour
    /// so its request ids never collide with sync's.
    pub fetch: request_response::Behaviour<FetchCodec>,
}

/// Gossipsub message size cap. The default (64 KiB) is plenty for actions
/// and beacons but below the worst-case encoded [`civora_governance::MAX_PROPOSAL_BYTES`]
/// proposal (192 KiB) and worst-case [`civora_governance::MAX_CERTIFICATE_BYTES`]
/// certificate (~132 KiB), so both full manifests and full-roster certificates
/// fit with framing to spare. Manifests keep gossiping whole; the artifacts they
/// reference travel separately over `/civora/fetch/1` after acceptance.
/// Announce-then-fetch of the manifests themselves is deferred (not delivered).
const MAX_GOSSIP_BYTES: usize = 256 * 1024;

impl Behaviour {
    pub fn new(key: &Keypair, enable_mdns: bool) -> Result<Self, String> {
        // Defaults (plus the size cap) give us signed messages with strict
        // validation; every gossiped payload is additionally a signed
        // message verified by its own gate, so transport-level signing is
        // belt and braces.
        let config = gossipsub::ConfigBuilder::default()
            .max_transmit_size(MAX_GOSSIP_BYTES)
            .build()
            .map_err(|err| err.to_string())?;
        let gossipsub =
            gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Signed(key.clone()), config)
                .map_err(str::to_owned)?;

        let mdns = enable_mdns
            .then(|| {
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())
                    .map_err(|err| err.to_string())
            })
            .transpose()?
            .into();

        let sync = request_response::Behaviour::with_codec(
            SyncCodec,
            [(SYNC_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default(),
        );

        let fetch = request_response::Behaviour::with_codec(
            FetchCodec,
            [(FETCH_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default(),
        );

        Ok(Self {
            gossipsub,
            mdns,
            sync,
            fetch,
        })
    }
}

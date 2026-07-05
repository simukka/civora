//! Composite libp2p behaviour: gossipsub (live actions + state beacons),
//! mDNS (LAN lobby discovery), and request-response (snapshot sync).

use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{gossipsub, mdns, request_response};

use crate::codec::{SYNC_PROTOCOL, SyncCodec};

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub gossipsub: gossipsub::Behaviour,
    /// Disabled for direct-dial sessions and tests (CI runners often filter
    /// multicast).
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub sync: request_response::Behaviour<SyncCodec>,
}

impl Behaviour {
    pub fn new(key: &Keypair, enable_mdns: bool) -> Result<Self, String> {
        // Defaults give us signed messages with strict validation; every
        // gossiped payload is additionally a SignedAction verified by the
        // kernel gate, so transport-level signing is belt and braces.
        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(key.clone()),
            gossipsub::Config::default(),
        )
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

        Ok(Self {
            gossipsub,
            mdns,
            sync,
        })
    }
}

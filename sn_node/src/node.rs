// Copyright 2023 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use super::{error::Result, event::NodeEventsChannel, Marker, NodeEvent};
#[cfg(feature = "open-metrics")]
use crate::metrics::NodeMetrics;
use crate::RunningNode;
use bls::{PublicKey, PK_SIZE};
use bytes::Bytes;
use libp2p::{autonat::NatStatus, identity::Keypair, Multiaddr, PeerId};
#[cfg(feature = "open-metrics")]
use prometheus_client::registry::Registry;
use rand::{rngs::StdRng, Rng, SeedableRng};
use sn_networking::{
    MsgResponder, Network, NetworkBuilder, NetworkEvent, SwarmDriver, CLOSE_GROUP_SIZE,
};
use sn_protocol::{
    error::Error as ProtocolError,
    messages::{Cmd, CmdResponse, Query, QueryResponse, Request, Response},
    NetworkAddress, PrettyPrintRecordKey,
};
use sn_transfers::{LocalWallet, MainPubkey, MainSecretKey, Transfer};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::mpsc::Receiver, task::spawn};
use xor_name::{XorName, XOR_NAME_LEN};

/// Interval to trigger replication on a random close_group peer
const PERIODIC_REPLICATION_INTERVAL: Duration = Duration::from_secs(10);

/// Expected topic name where notifications of royalty payment transfers are sent on.
/// The notification msg is expected to contain the serialised public key, followed by the
/// serialised transfer info encrypted against the referenced public key.
const ROYALTY_TRANSFER_NOTIF_TOPIC: &str = "ROYALTY_TRANSFER";

pub(crate) fn royalty_topic_group(peer_id: &PeerId) -> String {
    let group_index = common_prefix(
        &XorName::from_content(ROYALTY_TRANSFER_NOTIF_TOPIC.as_bytes()),
        &XorName::from_content(&peer_id.to_bytes()),
    );
    format!("{ROYALTY_TRANSFER_NOTIF_TOPIC}_GROUP_{group_index}")
}

/// Returns the length of the common prefix with the `other` name; e. g.
/// the when `other = 11110000` and `self = 11111111` this is 4.
fn common_prefix(xor_name_1: &XorName, xor_name_2: &XorName) -> usize {
    for byte_index in 0..XOR_NAME_LEN {
        if xor_name_1.0[byte_index] != xor_name_2.0[byte_index] {
            return (byte_index * 8)
                + (xor_name_1.0[byte_index] ^ xor_name_2.0[byte_index]).leading_zeros() as usize;
        }
    }
    8 * XOR_NAME_LEN
}

/// Helper to build and run a Node
pub struct NodeBuilder {
    keypair: Keypair,
    addr: SocketAddr,
    initial_peers: Vec<Multiaddr>,
    local: bool,
    root_dir: PathBuf,
    #[cfg(feature = "open-metrics")]
    metrics_server_port: u16,
}

impl NodeBuilder {
    /// Instantiate the builder
    pub fn new(
        keypair: Keypair,
        addr: SocketAddr,
        initial_peers: Vec<Multiaddr>,
        local: bool,
        root_dir: PathBuf,
    ) -> Self {
        Self {
            keypair,
            addr,
            initial_peers,
            local,
            root_dir,
            #[cfg(feature = "open-metrics")]
            metrics_server_port: 0,
        }
    }

    #[cfg(feature = "open-metrics")]
    /// Set the port for the OpenMetrics server. Defaults to a random port if not set
    pub fn metrics_server_port(&mut self, port: u16) {
        self.metrics_server_port = port;
    }

    /// Asynchronously runs a new node instance, setting up the swarm driver,
    /// creating a data storage, and handling network events. Returns the
    /// created `RunningNode` which contains a `NodeEventsChannel` for listening
    /// to node-related events.
    ///
    /// # Returns
    ///
    /// A `RunningNode` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if there is a problem initializing the `SwarmDriver`.
    pub fn build_and_run(self) -> Result<RunningNode> {
        // TODO: Make this key settable, and accessible via API
        let reward_key = MainSecretKey::random();
        let reward_address = reward_key.main_pubkey();

        let mut wallet = LocalWallet::load_from_main_key(&self.root_dir, reward_key)?;
        // store in case it's a fresh wallet created if none was found
        wallet.deposit_and_store_to_disk(&vec![])?;

        #[cfg(feature = "open-metrics")]
        let (metrics_registry, node_metrics) = {
            let mut metrics_registry = Registry::default();
            let node_metrics = NodeMetrics::new(&mut metrics_registry);
            (metrics_registry, node_metrics)
        };

        let mut network_builder = NetworkBuilder::new(self.keypair, self.local, self.root_dir);
        network_builder.listen_addr(self.addr);
        #[cfg(feature = "open-metrics")]
        network_builder.metrics_registry(metrics_registry);
        #[cfg(feature = "open-metrics")]
        network_builder.metrics_server_port(self.metrics_server_port);

        let (network, network_event_receiver, swarm_driver) = network_builder.build_node()?;
        let node_events_channel = NodeEventsChannel::default();

        let node = Node {
            network: network.clone(),
            events_channel: node_events_channel.clone(),
            initial_peers: self.initial_peers,
            reward_address,
            #[cfg(feature = "open-metrics")]
            node_metrics,
        };

        let running_node = RunningNode {
            network,
            node_events_channel,
        };

        // Run the node
        node.run(swarm_driver, network_event_receiver);

        // The subscription shall be undertaken by the nodes that really interested in
        // royalty payment transfer related cash_notes.
        for group_index in 0..256 {
            let topic = format!("{ROYALTY_TRANSFER_NOTIF_TOPIC}_GROUP_{group_index}");
            running_node
                .subscribe_to_topic(topic.clone())
                .map(|()| info!("Node has been subscribed to gossipsub topic '{topic}' to receive network royalties payments notifications."))?;
        }

        Ok(running_node)
    }
}

/// `Node` represents a single node in the distributed network. It handles
/// network events, processes incoming requests, interacts with the data
/// storage, and broadcasts node-related events.
#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) network: Network,
    pub(crate) events_channel: NodeEventsChannel,
    /// Peers that are dialed at startup of node.
    initial_peers: Vec<Multiaddr>,
    reward_address: MainPubkey,
    #[cfg(feature = "open-metrics")]
    pub(crate) node_metrics: NodeMetrics,
}

impl Node {
    /// Runs the provided `SwarmDriver` and spawns a task to process for `NetworkEvents`
    fn run(self, swarm_driver: SwarmDriver, mut network_event_receiver: Receiver<NetworkEvent>) {
        let mut rng = StdRng::from_entropy();

        let peers_connected = Arc::new(AtomicUsize::new(0));

        let _handle = spawn(swarm_driver.run());
        let _handle = spawn(async move {
            // use a random inactivity timeout to ensure that the nodes do not sync when messages
            // are being transmitted.
            let inactivity_timeout: i32 = rng.gen_range(20..40);
            let inactivity_timeout = Duration::from_secs(inactivity_timeout as u64);

            let mut replication_interval = tokio::time::interval(PERIODIC_REPLICATION_INTERVAL);
            let _ = replication_interval.tick().await; // first tick completes immediately

            loop {
                let peers_connected = peers_connected.clone();

                tokio::select! {
                    net_event = network_event_receiver.recv() => {
                        match net_event {
                            Some(event) => {
                                let stateless_node_copy = self.clone();
                                let _handle =
                                    spawn(async move {
                                        let start = std::time::Instant::now();
                                        let event_string = format!("{:?}", event);

                                        stateless_node_copy.handle_network_event(event, peers_connected).await ;
                                        info!("Handled network event in {:?}: {:?}", start.elapsed(), event_string);

                                    });
                            }
                            None => {
                                error!("The `NetworkEvent` channel is closed");
                                self.events_channel.broadcast(NodeEvent::ChannelClosed);
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(inactivity_timeout) => {
                        trace!("NetworkEvent inactivity timeout hit");
                        Marker::NoNetworkActivity( inactivity_timeout ).log();
                    }
                    // runs every replication_interval time
                    _ = replication_interval.tick() => {
                        let start = std::time::Instant::now();
                        info!("Periodic replication triggered");
                        let stateless_node_copy = self.clone();
                        let _handle = spawn(async move {
                            let closest_peers = match stateless_node_copy
                                .network
                                .get_closest_local_peers(&NetworkAddress::from_peer(
                                    stateless_node_copy.network.peer_id,
                                ))
                                .await
                            {
                                Ok(closest_peers) => closest_peers,
                                Err(err) => {
                                    error!("During forced replication cann't fetch local closest_peers {err:?}");
                                    return;
                                }
                            };

                            // early return to avoid panic on gen_range
                            if closest_peers.is_empty() {
                                info!("No peers to replicate to");
                                return;
                            }

                            let peer_id =
                                closest_peers[rand::thread_rng().gen_range(0..closest_peers.len())];
                            Marker::ForcedReplication(peer_id).log();

                            if let Err(err) = stateless_node_copy
                                .try_trigger_targetted_replication(peer_id, true)
                                .await
                            {
                                error!("During forced replication simulating lost of {peer_id:?}, error while triggering replication {err:?}");
                            }

                            info!("Periodic replication took {:?}", start.elapsed());
                        });
                    }
                }
            }
        });
    }

    /// Calls Marker::log() to insert the marker into the log files.
    /// Also calls NodeMetrics::record() to record the metric if the `open-metrics` feature flag is enabled.
    pub(crate) fn record_metrics(&self, marker: Marker) {
        marker.log();
        #[cfg(feature = "open-metrics")]
        self.node_metrics.record(marker);
    }

    // **** Private helpers *****

    async fn handle_network_event(&self, event: NetworkEvent, peers_connected: Arc<AtomicUsize>) {
        // when the node has not been connected to enough peers, it should not perform activities
        // that might require peers in the RT to succeed.
        let mut log_when_not_enough_peers = true;
        let start = std::time::Instant::now();
        loop {
            if peers_connected.load(Ordering::Relaxed) >= CLOSE_GROUP_SIZE {
                break;
            }
            match &event {
                // these activities requires the node to be connected to some peer to be able to carry
                // out get kad.get_record etc. This happens during replication/PUT. So we should wait
                // until we have enough nodes, else these might fail.
                NetworkEvent::RequestReceived { .. }
                | NetworkEvent::UnverifiedRecord(_)
                | NetworkEvent::FailedToWrite(_)
                | NetworkEvent::ResponseReceived { .. }
                | NetworkEvent::KeysForReplication(_) => {
                    if log_when_not_enough_peers {
                        debug!("Waiting before processing certain NetworkEvent before reaching {CLOSE_GROUP_SIZE} peers");
                    }
                    log_when_not_enough_peers = false;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                // These events do not need to wait until there are enough peers
                NetworkEvent::PeerAdded(_)
                | NetworkEvent::PeerRemoved(_)
                | NetworkEvent::NewListenAddr(_)
                | NetworkEvent::NatStatusChanged(_)
                | NetworkEvent::GossipsubMsgReceived { .. }
                | NetworkEvent::GossipsubMsgPublished { .. } => break,
            }
        }
        let event_string = format!("{:?}", event);
        trace!("Handling NetworkEvent {event_string:?}");

        match event {
            NetworkEvent::RequestReceived { req, channel } => {
                self.handle_request(req, channel).await;
            }
            NetworkEvent::ResponseReceived { res } => {
                trace!("NetworkEvent::ResponseReceived {res:?}");
                if let Err(err) = self.handle_response(res) {
                    error!("Error while handling NetworkEvent::ResponseReceived {err:?}");
                }
            }
            NetworkEvent::PeerAdded(peer_id) => {
                // increment peers_connected and send ConnectedToNetwork event if have connected to K_VALUE peers
                let _ = peers_connected.fetch_add(1, Ordering::SeqCst);
                if peers_connected.load(Ordering::SeqCst) == CLOSE_GROUP_SIZE {
                    self.events_channel.broadcast(NodeEvent::ConnectedToNetwork);
                }

                self.record_metrics(Marker::PeerAddedToRoutingTable(peer_id));

                if let Err(err) = self.try_trigger_targetted_replication(peer_id, false).await {
                    error!("During CloseGroupUpdate, error while triggering replication {err:?}");
                }
            }
            NetworkEvent::PeerRemoved(peer_id) => {
                self.record_metrics(Marker::PeerRemovedFromRoutingTable(peer_id));
                // During a node restart, the new node got added before the old one got removed.
                // If the old one is `pushed out of close_group by the new one`, then the records
                // that being close to the old one won't got replicated during the CloseGroupUpdate
                // of the new one, as the old one still sits in the local kBuckets.
                // Hence, the replication attempts shall also be undertaken when PeerRemoved.
                if let Err(err) = self.try_trigger_targetted_replication(peer_id, true).await {
                    error!("During PeerRemoved, error while triggering replication {err:?}");
                }
            }
            NetworkEvent::KeysForReplication(keys) => {
                self.record_metrics(Marker::fetching_keys_for_replication(&keys));

                if let Err(err) = self.fetch_replication_keys_without_wait(keys) {
                    error!("Error while trying to fetch replicated data {err:?}");
                }
            }
            NetworkEvent::NewListenAddr(_) => {
                if !cfg!(feature = "local-discovery") {
                    let network = self.network.clone();
                    let peers = self.initial_peers.clone();
                    let _handle = spawn(async move {
                        for addr in &peers {
                            if let Err(err) = network.dial(addr.clone()).await {
                                tracing::error!("Failed to dial {addr}: {err:?}");
                            };
                        }
                    });
                }
            }
            NetworkEvent::NatStatusChanged(status) => {
                if matches!(status, NatStatus::Private) {
                    tracing::warn!("NAT status is determined to be private!");
                    self.events_channel.broadcast(NodeEvent::BehindNat);
                }
            }
            NetworkEvent::UnverifiedRecord(record) => {
                let key = PrettyPrintRecordKey::from(&record.key).into_owned();
                match self.validate_and_store_record(record).await {
                    Ok(cmdok) => trace!("UnverifiedRecord {key} stored with {cmdok:?}."),
                    Err(err) => {
                        self.record_metrics(Marker::RecordRejected(&key));
                        trace!("UnverifiedRecord {key} failed to be stored with error {err:?}.")
                    }
                }
            }
            NetworkEvent::FailedToWrite(key) => {
                if let Err(e) = self.network.remove_failed_local_record(key) {
                    error!("Failed to remove local record: {e:?}");
                }
            }
            NetworkEvent::GossipsubMsgReceived { topic, msg }
            | NetworkEvent::GossipsubMsgPublished { topic, msg } => {
                trace!("Received a gossip msg for the topic of {topic}");

                if self.events_channel.receiver_count() > 0 {
                    // TODO: shall record which topics this network instance subscribed.
                    //       currently just using a wild match
                    if topic.contains(ROYALTY_TRANSFER_NOTIF_TOPIC) {
                        // this is expected to be a notification of a transfer which we treat specially
                        match try_decode_transfer_notif(&msg) {
                            Ok(notif_event) => return self.events_channel.broadcast(notif_event),
                            Err(err) => warn!("GossipsubMsg matching the transfer notif. topic name, couldn't be decoded as such: {:?}", err),
                        }
                    }

                    self.events_channel
                        .broadcast(NodeEvent::GossipsubMsg { topic, msg });
                }
            }
        }

        trace!(
            "NetworkEvent handled in {:?} : {event_string:?}",
            start.elapsed()
        );
    }

    // Handle the response that was not awaited at the call site
    fn handle_response(&self, response: Response) -> Result<()> {
        match response {
            Response::Cmd(CmdResponse::Replicate(Ok(()))) => {
                // Nothing to do, response was fine
                // This only exists to ensure we dont drop the handle and
                // exit early, potentially logging false connection woes
            }
            Response::Query(QueryResponse::GetReplicatedRecord(resp)) => {
                error!("Response to replication shall be handled by called not by common handler, {resp:?}");
            }
            other => {
                warn!("handle_response not implemented for {other:?}");
            }
        };

        Ok(())
    }

    async fn handle_request(&self, request: Request, response_channel: MsgResponder) {
        trace!("Handling request: {request:?}");
        let response = match request {
            Request::Cmd(cmd) => self.handle_node_cmd(cmd),
            Request::Query(query) => self.handle_query(query).await,
        };
        self.send_response(response, response_channel);
    }

    async fn handle_query(&self, query: Query) -> Response {
        let resp: QueryResponse = match query {
            Query::GetStoreCost(address) => {
                trace!("Got GetStoreCost request for {address:?}");
                let payment_address = self.reward_address;

                let record_exists = {
                    if let Some(key) = address.as_record_key() {
                        match self.network.is_record_key_present_locally(&key).await {
                            Ok(res) => res,
                            Err(error) => {
                                error!("Problem getting record key's existence: {error:?}");
                                false
                            }
                        }
                    } else {
                        false
                    }
                };

                if record_exists {
                    QueryResponse::GetStoreCost {
                        store_cost: Err(ProtocolError::RecordExists(
                            PrettyPrintRecordKey::from(&address.to_record_key()).into_owned(),
                        )),
                        payment_address,
                    }
                } else {
                    let store_cost = self
                        .network
                        .get_local_storecost()
                        .await
                        .map_err(|_| ProtocolError::GetStoreCostFailed);

                    QueryResponse::GetStoreCost {
                        store_cost,
                        payment_address,
                    }
                }
            }
            Query::GetReplicatedRecord { requester, key } => {
                trace!("Got GetReplicatedRecord from {requester:?} regarding {key:?}");

                let our_address = NetworkAddress::from_peer(self.network.peer_id);
                let mut result = Err(ProtocolError::ReplicatedRecordNotFound {
                    holder: Box::new(our_address.clone()),
                    key: Box::new(key.clone()),
                });
                let record_key = key.as_record_key();

                if let Some(record_key) = record_key {
                    if let Ok(Some(record)) = self.network.get_local_record(&record_key).await {
                        result = Ok((our_address, Bytes::from(record.value)));
                    }
                }

                QueryResponse::GetReplicatedRecord(result)
            }
        };
        Response::Query(resp)
    }

    fn handle_node_cmd(&self, cmd: Cmd) -> Response {
        Marker::NodeCmdReceived(&cmd).log();
        let resp = match cmd {
            Cmd::Replicate { holder, keys } => {
                trace!(
                    "Received replication list from {holder:?} of {} keys {keys:?}",
                    keys.len()
                );

                if let Some(peer_id) = holder.as_peer_id() {
                    // todo: error is not propagated to the caller here
                    let _ = self.add_keys_to_replication_fetcher(peer_id, keys);
                } else {
                    error!("Within the replication list, Can not parse peer_id from {holder:?}");
                }

                // if we do not send a response, we can cause connection failures.
                CmdResponse::Replicate(Ok(()))
            }
        };

        Marker::NodeCmdResponded(&resp).log();

        Response::Cmd(resp)
    }

    fn send_response(&self, resp: Response, response_channel: MsgResponder) {
        if let Err(err) = self.network.send_response(resp, response_channel) {
            warn!("Error while sending response: {err:?}");
        }
    }
}

fn try_decode_transfer_notif(msg: &[u8]) -> eyre::Result<NodeEvent> {
    let mut key_bytes = [0u8; PK_SIZE];
    key_bytes.copy_from_slice(
        msg.get(0..PK_SIZE)
            .ok_or_else(|| eyre::eyre!("msg doesn't have enough bytes"))?,
    );
    let key = PublicKey::from_bytes(key_bytes)?;

    let transfers: Vec<Transfer> = bincode::deserialize(&msg[PK_SIZE..])?;

    Ok(NodeEvent::TransferNotif { key, transfers })
}

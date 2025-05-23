// This file is part of Substrate.
//
// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.
//
// If you read this, you are very thorough, congratulations.

//! Traits defined by `sc-network`.

use crate::{
	config::{IncomingRequest, MultiaddrWithPeerId, NotificationHandshake, Params, SetConfig},
	error::{self, Error},
	event::Event,
	network_state::NetworkState,
	request_responses::{IfDisconnected, RequestFailure},
	service::{metrics::NotificationMetrics, signature::Signature, PeerStoreProvider},
	types::ProtocolName,
	ReputationChange,
};

use futures::{channel::oneshot, Stream};
use prometheus_endpoint::Registry;

use sc_client_api::BlockBackend;
use sc_network_common::{role::ObservedRole, ExHashT};
pub use sc_network_types::{
	kad::{Key as KademliaKey, Record},
	multiaddr::Multiaddr,
	PeerId,
};
use sp_runtime::traits::Block as BlockT;

use std::{
	collections::HashSet,
	fmt::Debug,
	future::Future,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};

pub use libp2p::identity::SigningError;

/// Supertrait defining the services provided by [`NetworkBackend`] service handle.
pub trait NetworkService:
	NetworkSigner
	+ NetworkDHTProvider
	+ NetworkStatusProvider
	+ NetworkPeers
	+ NetworkEventStream
	+ NetworkStateInfo
	+ NetworkRequest
	+ Send
	+ Sync
	+ 'static
{
}

impl<T> NetworkService for T where
	T: NetworkSigner
		+ NetworkDHTProvider
		+ NetworkStatusProvider
		+ NetworkPeers
		+ NetworkEventStream
		+ NetworkStateInfo
		+ NetworkRequest
		+ Send
		+ Sync
		+ 'static
{
}

/// Trait defining the required functionality from a notification protocol configuration.
pub trait NotificationConfig: Debug {
	/// Get access to the `SetConfig` of the notification protocol.
	fn set_config(&self) -> &SetConfig;

	/// Get protocol name.
	fn protocol_name(&self) -> &ProtocolName;
}

/// Trait defining the required functionality from a request-response protocol configuration.
pub trait RequestResponseConfig: Debug {
	/// Get protocol name.
	fn protocol_name(&self) -> &ProtocolName;
}

/// Trait defining required functionality from `PeerStore`.
#[async_trait::async_trait]
pub trait PeerStore {
	/// Get handle to `PeerStore`.
	fn handle(&self) -> Arc<dyn PeerStoreProvider>;

	/// Start running `PeerStore` event loop.
	async fn run(self);
}

/// Networking backend.
#[async_trait::async_trait]
pub trait NetworkBackend<B: BlockT + 'static, H: ExHashT>: Send + 'static {
	/// Type representing notification protocol-related configuration.
	type NotificationProtocolConfig: NotificationConfig;

	/// Type representing request-response protocol-related configuration.
	type RequestResponseProtocolConfig: RequestResponseConfig;

	/// Type implementing `NetworkService` for the networking backend.
	///
	/// `NetworkService` allows other subsystems of the blockchain to interact with `sc-network`
	/// using `NetworkService`.
	type NetworkService<Block, Hash>: NetworkService + Clone;

	/// Type implementing [`PeerStore`].
	type PeerStore: PeerStore;

	/// Bitswap config.
	type BitswapConfig;

	/// Create new `NetworkBackend`.
	fn new(params: Params<B, H, Self>) -> Result<Self, Error>
	where
		Self: Sized;

	/// Get handle to `NetworkService` of the `NetworkBackend`.
	fn network_service(&self) -> Arc<dyn NetworkService>;

	/// Create [`PeerStore`].
	fn peer_store(bootnodes: Vec<PeerId>, metrics_registry: Option<Registry>) -> Self::PeerStore;

	/// Register metrics that are used by the notification protocols.
	fn register_notification_metrics(registry: Option<&Registry>) -> NotificationMetrics;

	/// Create Bitswap server.
	fn bitswap_server(
		client: Arc<dyn BlockBackend<B> + Send + Sync>,
	) -> (Pin<Box<dyn Future<Output = ()> + Send>>, Self::BitswapConfig);

	/// Create notification protocol configuration and an associated `NotificationService`
	/// for the protocol.
	fn notification_config(
		protocol_name: ProtocolName,
		fallback_names: Vec<ProtocolName>,
		max_notification_size: u64,
		handshake: Option<NotificationHandshake>,
		set_config: SetConfig,
		metrics: NotificationMetrics,
		peerstore_handle: Arc<dyn PeerStoreProvider>,
	) -> (Self::NotificationProtocolConfig, Box<dyn NotificationService>);

	/// Create request-response protocol configuration.
	fn request_response_config(
		protocol_name: ProtocolName,
		fallback_names: Vec<ProtocolName>,
		max_request_size: u64,
		max_response_size: u64,
		request_timeout: Duration,
		inbound_queue: Option<async_channel::Sender<IncomingRequest>>,
	) -> Self::RequestResponseProtocolConfig;

	/// Start [`NetworkBackend`] event loop.
	async fn run(mut self);
}

/// Signer with network identity
pub trait NetworkSigner {
	/// Signs the message with the `KeyPair` that defines the local [`PeerId`].
	fn sign_with_local_identity(&self, msg: Vec<u8>) -> Result<Signature, SigningError>;

	/// Verify signature using peer's public key.
	///
	/// `public_key` must be Protobuf-encoded ed25519 public key.
	///
	/// Returns `Err(())` if public cannot be parsed into a valid ed25519 public key.
	fn verify(
		&self,
		peer_id: sc_network_types::PeerId,
		public_key: &Vec<u8>,
		signature: &Vec<u8>,
		message: &Vec<u8>,
	) -> Result<bool, String>;
}

impl<T> NetworkSigner for Arc<T>
where
	T: ?Sized,
	T: NetworkSigner,
{
	fn sign_with_local_identity(&self, msg: Vec<u8>) -> Result<Signature, SigningError> {
		T::sign_with_local_identity(self, msg)
	}

	fn verify(
		&self,
		peer_id: sc_network_types::PeerId,
		public_key: &Vec<u8>,
		signature: &Vec<u8>,
		message: &Vec<u8>,
	) -> Result<bool, String> {
		T::verify(self, peer_id, public_key, signature, message)
	}
}

/// Provides access to the networking DHT.
pub trait NetworkDHTProvider {
	/// Start finding closest peers to the target.
	fn find_closest_peers(&self, target: PeerId);

	/// Start getting a value from the DHT.
	fn get_value(&self, key: &KademliaKey);

	/// Start putting a value in the DHT.
	fn put_value(&self, key: KademliaKey, value: Vec<u8>);

	/// Start putting the record to `peers`.
	///
	/// If `update_local_storage` is true the local storage is udpated as well.
	fn put_record_to(&self, record: Record, peers: HashSet<PeerId>, update_local_storage: bool);

	/// Store a record in the DHT memory store.
	fn store_record(
		&self,
		key: KademliaKey,
		value: Vec<u8>,
		publisher: Option<PeerId>,
		expires: Option<Instant>,
	);

	/// Register this node as a provider for `key` on the DHT.
	fn start_providing(&self, key: KademliaKey);

	/// Deregister this node as a provider for `key` on the DHT.
	fn stop_providing(&self, key: KademliaKey);

	/// Start getting the list of providers for `key` on the DHT.
	fn get_providers(&self, key: KademliaKey);
}

impl<T> NetworkDHTProvider for Arc<T>
where
	T: ?Sized,
	T: NetworkDHTProvider,
{
	fn find_closest_peers(&self, target: PeerId) {
		T::find_closest_peers(self, target)
	}

	fn get_value(&self, key: &KademliaKey) {
		T::get_value(self, key)
	}

	fn put_value(&self, key: KademliaKey, value: Vec<u8>) {
		T::put_value(self, key, value)
	}

	fn put_record_to(&self, record: Record, peers: HashSet<PeerId>, update_local_storage: bool) {
		T::put_record_to(self, record, peers, update_local_storage)
	}

	fn store_record(
		&self,
		key: KademliaKey,
		value: Vec<u8>,
		publisher: Option<PeerId>,
		expires: Option<Instant>,
	) {
		T::store_record(self, key, value, publisher, expires)
	}

	fn start_providing(&self, key: KademliaKey) {
		T::start_providing(self, key)
	}

	fn stop_providing(&self, key: KademliaKey) {
		T::stop_providing(self, key)
	}

	fn get_providers(&self, key: KademliaKey) {
		T::get_providers(self, key)
	}
}

/// Provides an ability to set a fork sync request for a particular block.
pub trait NetworkSyncForkRequest<BlockHash, BlockNumber> {
	/// Notifies the sync service to try and sync the given block from the given
	/// peers.
	///
	/// If the given vector of peers is empty then the underlying implementation
	/// should make a best effort to fetch the block from any peers it is
	/// connected to (NOTE: this assumption will change in the future #3629).
	fn set_sync_fork_request(&self, peers: Vec<PeerId>, hash: BlockHash, number: BlockNumber);
}

impl<T, BlockHash, BlockNumber> NetworkSyncForkRequest<BlockHash, BlockNumber> for Arc<T>
where
	T: ?Sized,
	T: NetworkSyncForkRequest<BlockHash, BlockNumber>,
{
	fn set_sync_fork_request(&self, peers: Vec<PeerId>, hash: BlockHash, number: BlockNumber) {
		T::set_sync_fork_request(self, peers, hash, number)
	}
}

/// Overview status of the network.
#[derive(Clone)]
pub struct NetworkStatus {
	/// Total number of connected peers.
	pub num_connected_peers: usize,
	/// The total number of bytes received.
	pub total_bytes_inbound: u64,
	/// The total number of bytes sent.
	pub total_bytes_outbound: u64,
}

/// Provides high-level status information about network.
#[async_trait::async_trait]
pub trait NetworkStatusProvider {
	/// High-level network status information.
	///
	/// Returns an error if the `NetworkWorker` is no longer running.
	async fn status(&self) -> Result<NetworkStatus, ()>;

	/// Get the network state.
	///
	/// Returns an error if the `NetworkWorker` is no longer running.
	async fn network_state(&self) -> Result<NetworkState, ()>;
}

// Manual implementation to avoid extra boxing here
impl<T> NetworkStatusProvider for Arc<T>
where
	T: ?Sized,
	T: NetworkStatusProvider,
{
	fn status<'life0, 'async_trait>(
		&'life0 self,
	) -> Pin<Box<dyn Future<Output = Result<NetworkStatus, ()>> + Send + 'async_trait>>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		T::status(self)
	}

	fn network_state<'life0, 'async_trait>(
		&'life0 self,
	) -> Pin<Box<dyn Future<Output = Result<NetworkState, ()>> + Send + 'async_trait>>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		T::network_state(self)
	}
}

/// Provides low-level API for manipulating network peers.
#[async_trait::async_trait]
pub trait NetworkPeers {
	/// Set authorized peers.
	///
	/// Need a better solution to manage authorized peers, but now just use reserved peers for
	/// prototyping.
	fn set_authorized_peers(&self, peers: HashSet<PeerId>);

	/// Set authorized_only flag.
	///
	/// Need a better solution to decide authorized_only, but now just use reserved_only flag for
	/// prototyping.
	fn set_authorized_only(&self, reserved_only: bool);

	/// Adds an address known to a node.
	fn add_known_address(&self, peer_id: PeerId, addr: Multiaddr);

	/// Report a given peer as either beneficial (+) or costly (-) according to the
	/// given scalar.
	fn report_peer(&self, peer_id: PeerId, cost_benefit: ReputationChange);

	/// Get peer reputation.
	fn peer_reputation(&self, peer_id: &PeerId) -> i32;

	/// Disconnect from a node as soon as possible.
	///
	/// This triggers the same effects as if the connection had closed itself spontaneously.
	fn disconnect_peer(&self, peer_id: PeerId, protocol: ProtocolName);

	/// Connect to unreserved peers and allow unreserved peers to connect for syncing purposes.
	fn accept_unreserved_peers(&self);

	/// Disconnect from unreserved peers and deny new unreserved peers to connect for syncing
	/// purposes.
	fn deny_unreserved_peers(&self);

	/// Adds a `PeerId` and its `Multiaddr` as reserved for a sync protocol (default peer set).
	///
	/// Returns an `Err` if the given string is not a valid multiaddress
	/// or contains an invalid peer ID (which includes the local peer ID).
	fn add_reserved_peer(&self, peer: MultiaddrWithPeerId) -> Result<(), String>;

	/// Removes a `PeerId` from the list of reserved peers for a sync protocol (default peer set).
	fn remove_reserved_peer(&self, peer_id: PeerId);

	/// Sets the reserved set of a protocol to the given set of peers.
	///
	/// Each `Multiaddr` must end with a `/p2p/` component containing the `PeerId`. It can also
	/// consist of only `/p2p/<peerid>`.
	///
	/// The node will start establishing/accepting connections and substreams to/from peers in this
	/// set, if it doesn't have any substream open with them yet.
	///
	/// Note however, if a call to this function results in less peers on the reserved set, they
	/// will not necessarily get disconnected (depending on available free slots in the peer set).
	/// If you want to also disconnect those removed peers, you will have to call
	/// `remove_from_peers_set` on those in addition to updating the reserved set. You can omit
	/// this step if the peer set is in reserved only mode.
	///
	/// Returns an `Err` if one of the given addresses is invalid or contains an
	/// invalid peer ID (which includes the local peer ID), or if `protocol` does not
	/// refer to a known protocol.
	fn set_reserved_peers(
		&self,
		protocol: ProtocolName,
		peers: HashSet<Multiaddr>,
	) -> Result<(), String>;

	/// Add peers to a peer set.
	///
	/// Each `Multiaddr` must end with a `/p2p/` component containing the `PeerId`. It can also
	/// consist of only `/p2p/<peerid>`.
	///
	/// Returns an `Err` if one of the given addresses is invalid or contains an
	/// invalid peer ID (which includes the local peer ID), or if `protocol` does not
	/// refer to a know protocol.
	fn add_peers_to_reserved_set(
		&self,
		protocol: ProtocolName,
		peers: HashSet<Multiaddr>,
	) -> Result<(), String>;

	/// Remove peers from a peer set.
	///
	/// Returns `Err` if `protocol` does not refer to a known protocol.
	fn remove_peers_from_reserved_set(
		&self,
		protocol: ProtocolName,
		peers: Vec<PeerId>,
	) -> Result<(), String>;

	/// Returns the number of peers in the sync peer set we're connected to.
	fn sync_num_connected(&self) -> usize;

	/// Attempt to get peer role.
	///
	/// Right now the peer role is decoded from the received handshake for all protocols
	/// (`/block-announces/1` has other information as well). If the handshake cannot be
	/// decoded into a role, the role queried from `PeerStore` and if the role is not stored
	/// there either, `None` is returned and the peer should be discarded.
	fn peer_role(&self, peer_id: PeerId, handshake: Vec<u8>) -> Option<ObservedRole>;

	/// Get the list of reserved peers.
	///
	/// Returns an error if the `NetworkWorker` is no longer running.
	async fn reserved_peers(&self) -> Result<Vec<PeerId>, ()>;
}

// Manual implementation to avoid extra boxing here
#[async_trait::async_trait]
impl<T> NetworkPeers for Arc<T>
where
	T: ?Sized,
	T: NetworkPeers,
{
	fn set_authorized_peers(&self, peers: HashSet<PeerId>) {
		T::set_authorized_peers(self, peers)
	}

	fn set_authorized_only(&self, reserved_only: bool) {
		T::set_authorized_only(self, reserved_only)
	}

	fn add_known_address(&self, peer_id: PeerId, addr: Multiaddr) {
		T::add_known_address(self, peer_id, addr)
	}

	fn report_peer(&self, peer_id: PeerId, cost_benefit: ReputationChange) {
		T::report_peer(self, peer_id, cost_benefit)
	}

	fn peer_reputation(&self, peer_id: &PeerId) -> i32 {
		T::peer_reputation(self, peer_id)
	}

	fn disconnect_peer(&self, peer_id: PeerId, protocol: ProtocolName) {
		T::disconnect_peer(self, peer_id, protocol)
	}

	fn accept_unreserved_peers(&self) {
		T::accept_unreserved_peers(self)
	}

	fn deny_unreserved_peers(&self) {
		T::deny_unreserved_peers(self)
	}

	fn add_reserved_peer(&self, peer: MultiaddrWithPeerId) -> Result<(), String> {
		T::add_reserved_peer(self, peer)
	}

	fn remove_reserved_peer(&self, peer_id: PeerId) {
		T::remove_reserved_peer(self, peer_id)
	}

	fn set_reserved_peers(
		&self,
		protocol: ProtocolName,
		peers: HashSet<Multiaddr>,
	) -> Result<(), String> {
		T::set_reserved_peers(self, protocol, peers)
	}

	fn add_peers_to_reserved_set(
		&self,
		protocol: ProtocolName,
		peers: HashSet<Multiaddr>,
	) -> Result<(), String> {
		T::add_peers_to_reserved_set(self, protocol, peers)
	}

	fn remove_peers_from_reserved_set(
		&self,
		protocol: ProtocolName,
		peers: Vec<PeerId>,
	) -> Result<(), String> {
		T::remove_peers_from_reserved_set(self, protocol, peers)
	}

	fn sync_num_connected(&self) -> usize {
		T::sync_num_connected(self)
	}

	fn peer_role(&self, peer_id: PeerId, handshake: Vec<u8>) -> Option<ObservedRole> {
		T::peer_role(self, peer_id, handshake)
	}

	fn reserved_peers<'life0, 'async_trait>(
		&'life0 self,
	) -> Pin<Box<dyn Future<Output = Result<Vec<PeerId>, ()>> + Send + 'async_trait>>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		T::reserved_peers(self)
	}
}

/// Provides access to network-level event stream.
pub trait NetworkEventStream {
	/// Returns a stream containing the events that happen on the network.
	///
	/// If this method is called multiple times, the events are duplicated.
	///
	/// The stream never ends (unless the `NetworkWorker` gets shut down).
	///
	/// The name passed is used to identify the channel in the Prometheus metrics. Note that the
	/// parameter is a `&'static str`, and not a `String`, in order to avoid accidentally having
	/// an unbounded set of Prometheus metrics, which would be quite bad in terms of memory
	fn event_stream(&self, name: &'static str) -> Pin<Box<dyn Stream<Item = Event> + Send>>;
}

impl<T> NetworkEventStream for Arc<T>
where
	T: ?Sized,
	T: NetworkEventStream,
{
	fn event_stream(&self, name: &'static str) -> Pin<Box<dyn Stream<Item = Event> + Send>> {
		T::event_stream(self, name)
	}
}

/// Trait for providing information about the local network state
pub trait NetworkStateInfo {
	/// Returns the local external addresses.
	fn external_addresses(&self) -> Vec<Multiaddr>;

	/// Returns the listening addresses (without trailing `/p2p/` with our `PeerId`).
	fn listen_addresses(&self) -> Vec<Multiaddr>;

	/// Returns the local Peer ID.
	fn local_peer_id(&self) -> PeerId;
}

impl<T> NetworkStateInfo for Arc<T>
where
	T: ?Sized,
	T: NetworkStateInfo,
{
	fn external_addresses(&self) -> Vec<Multiaddr> {
		T::external_addresses(self)
	}

	fn listen_addresses(&self) -> Vec<Multiaddr> {
		T::listen_addresses(self)
	}

	fn local_peer_id(&self) -> PeerId {
		T::local_peer_id(self)
	}
}

/// Reserved slot in the notifications buffer, ready to accept data.
pub trait NotificationSenderReady {
	/// Consumes this slots reservation and actually queues the notification.
	///
	/// NOTE: Traits can't consume itself, but calling this method second time will return an error.
	fn send(&mut self, notification: Vec<u8>) -> Result<(), NotificationSenderError>;
}

/// A `NotificationSender` allows for sending notifications to a peer with a chosen protocol.
#[async_trait::async_trait]
pub trait NotificationSender: Send + Sync + 'static {
	/// Returns a future that resolves when the `NotificationSender` is ready to send a
	/// notification.
	async fn ready(&self)
		-> Result<Box<dyn NotificationSenderReady + '_>, NotificationSenderError>;
}

/// Error returned by the notification sink.
#[derive(Debug, thiserror::Error)]
pub enum NotificationSenderError {
	/// The notification receiver has been closed, usually because the underlying connection
	/// closed.
	///
	/// Some of the notifications most recently sent may not have been received. However,
	/// the peer may still be connected and a new notification sink for the same
	/// protocol obtained from [`NotificationService::message_sink()`].
	#[error("The notification receiver has been closed")]
	Closed,
	/// Protocol name hasn't been registered.
	#[error("Protocol name hasn't been registered")]
	BadProtocol,
}

/// Provides ability to send network requests.
#[async_trait::async_trait]
pub trait NetworkRequest {
	/// Sends a single targeted request to a specific peer. On success, returns the response of
	/// the peer.
	///
	/// Request-response protocols are a way to complement notifications protocols, but
	/// notifications should remain the default ways of communicating information. For example, a
	/// peer can announce something through a notification, after which the recipient can obtain
	/// more information by performing a request.
	/// As such, call this function with `IfDisconnected::ImmediateError` for `connect`. This way
	/// you will get an error immediately for disconnected peers, instead of waiting for a
	/// potentially very long connection attempt, which would suggest that something is wrong
	/// anyway, as you are supposed to be connected because of the notification protocol.
	///
	/// No limit or throttling of concurrent outbound requests per peer and protocol are enforced.
	/// Such restrictions, if desired, need to be enforced at the call site(s).
	///
	/// The protocol must have been registered through
	/// `NetworkConfiguration::request_response_protocols`.
	async fn request(
		&self,
		target: PeerId,
		protocol: ProtocolName,
		request: Vec<u8>,
		fallback_request: Option<(Vec<u8>, ProtocolName)>,
		connect: IfDisconnected,
	) -> Result<(Vec<u8>, ProtocolName), RequestFailure>;

	/// Variation of `request` which starts a request whose response is delivered on a provided
	/// channel.
	///
	/// Instead of blocking and waiting for a reply, this function returns immediately, sending
	/// responses via the passed in sender. This alternative API exists to make it easier to
	/// integrate with message passing APIs.
	///
	/// Keep in mind that the connected receiver might receive a `Canceled` event in case of a
	/// closing connection. This is expected behaviour. With `request` you would get a
	/// `RequestFailure::Network(OutboundFailure::ConnectionClosed)` in that case.
	fn start_request(
		&self,
		target: PeerId,
		protocol: ProtocolName,
		request: Vec<u8>,
		fallback_request: Option<(Vec<u8>, ProtocolName)>,
		tx: oneshot::Sender<Result<(Vec<u8>, ProtocolName), RequestFailure>>,
		connect: IfDisconnected,
	);
}

// Manual implementation to avoid extra boxing here
impl<T> NetworkRequest for Arc<T>
where
	T: ?Sized,
	T: NetworkRequest,
{
	fn request<'life0, 'async_trait>(
		&'life0 self,
		target: PeerId,
		protocol: ProtocolName,
		request: Vec<u8>,
		fallback_request: Option<(Vec<u8>, ProtocolName)>,
		connect: IfDisconnected,
	) -> Pin<
		Box<
			dyn Future<Output = Result<(Vec<u8>, ProtocolName), RequestFailure>>
				+ Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		T::request(self, target, protocol, request, fallback_request, connect)
	}

	fn start_request(
		&self,
		target: PeerId,
		protocol: ProtocolName,
		request: Vec<u8>,
		fallback_request: Option<(Vec<u8>, ProtocolName)>,
		tx: oneshot::Sender<Result<(Vec<u8>, ProtocolName), RequestFailure>>,
		connect: IfDisconnected,
	) {
		T::start_request(self, target, protocol, request, fallback_request, tx, connect)
	}
}

/// Provides ability to announce blocks to the network.
pub trait NetworkBlock<BlockHash, BlockNumber> {
	/// Make sure an important block is propagated to peers.
	///
	/// In chain-based consensus, we often need to make sure non-best forks are
	/// at least temporarily synced. This function forces such an announcement.
	fn announce_block(&self, hash: BlockHash, data: Option<Vec<u8>>);

	/// Inform the network service about new best imported block.
	fn new_best_block_imported(&self, hash: BlockHash, number: BlockNumber);
}

impl<T, BlockHash, BlockNumber> NetworkBlock<BlockHash, BlockNumber> for Arc<T>
where
	T: ?Sized,
	T: NetworkBlock<BlockHash, BlockNumber>,
{
	fn announce_block(&self, hash: BlockHash, data: Option<Vec<u8>>) {
		T::announce_block(self, hash, data)
	}

	fn new_best_block_imported(&self, hash: BlockHash, number: BlockNumber) {
		T::new_best_block_imported(self, hash, number)
	}
}

/// Substream acceptance result.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationResult {
	/// Accept inbound substream.
	Accept,

	/// Reject inbound substream.
	Reject,
}

/// Substream direction.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Direction {
	/// Substream opened by the remote node.
	Inbound,

	/// Substream opened by the local node.
	Outbound,
}

impl From<litep2p::protocol::notification::Direction> for Direction {
	fn from(direction: litep2p::protocol::notification::Direction) -> Self {
		match direction {
			litep2p::protocol::notification::Direction::Inbound => Direction::Inbound,
			litep2p::protocol::notification::Direction::Outbound => Direction::Outbound,
		}
	}
}

impl Direction {
	/// Is the direction inbound.
	pub fn is_inbound(&self) -> bool {
		std::matches!(self, Direction::Inbound)
	}
}

/// Events received by the protocol from `Notifications`.
#[derive(Debug)]
pub enum NotificationEvent {
	/// Validate inbound substream.
	ValidateInboundSubstream {
		/// Peer ID.
		peer: PeerId,

		/// Received handshake.
		handshake: Vec<u8>,

		/// `oneshot::Sender` for sending validation result back to `Notifications`
		result_tx: tokio::sync::oneshot::Sender<ValidationResult>,
	},

	/// Remote identified by `PeerId` opened a substream and sent `Handshake`.
	/// Validate `Handshake` and report status (accept/reject) to `Notifications`.
	NotificationStreamOpened {
		/// Peer ID.
		peer: PeerId,

		/// Is the substream inbound or outbound.
		direction: Direction,

		/// Received handshake.
		handshake: Vec<u8>,

		/// Negotiated fallback.
		negotiated_fallback: Option<ProtocolName>,
	},

	/// Substream was closed.
	NotificationStreamClosed {
		/// Peer Id.
		peer: PeerId,
	},

	/// Notification was received from the substream.
	NotificationReceived {
		/// Peer ID.
		peer: PeerId,

		/// Received notification.
		notification: Vec<u8>,
	},
}

/// Notification service
///
/// Defines behaviors that both the protocol implementations and `Notifications` can expect from
/// each other.
///
/// `Notifications` can send two different kinds of information to protocol:
///  * substream-related information
///  * notification-related information
///
/// When an unvalidated, inbound substream is received by `Notifications`, it sends the inbound
/// stream information (peer ID, handshake) to protocol for validation. Protocol must then verify
/// that the handshake is valid (and in the future that it has a slot it can allocate for the peer)
/// and then report back the `ValidationResult` which is either `Accept` or `Reject`.
///
/// After the validation result has been received by `Notifications`, it prepares the
/// substream for communication by initializing the necessary sinks and emits
/// `NotificationStreamOpened` which informs the protocol that the remote peer is ready to receive
/// notifications.
///
/// Two different flavors of sending options are provided:
///  * synchronous sending ([`NotificationService::send_sync_notification()`])
///  * asynchronous sending ([`NotificationService::send_async_notification()`])
///
/// The former is used by the protocols not ready to exercise backpressure and the latter by the
/// protocols that can do it.
///
/// Both local and remote peer can close the substream at any time. Local peer can do so by calling
/// [`NotificationService::close_substream()`] which instructs `Notifications` to close the
/// substream. Remote closing the substream is indicated to the local peer by receiving
/// [`NotificationEvent::NotificationStreamClosed`] event.
///
/// In case the protocol must update its handshake while it's operating (such as updating the best
/// block information), it can do so by calling [`NotificationService::set_handshake()`]
/// which instructs `Notifications` to update the handshake it stored during protocol
/// initialization.
///
/// All peer events are multiplexed on the same incoming event stream from `Notifications` and thus
/// each event carries a `PeerId` so the protocol knows whose information to update when receiving
/// an event.
#[async_trait::async_trait]
pub trait NotificationService: Debug + Send {
	/// Instruct `Notifications` to open a new substream for `peer`.
	///
	/// `dial_if_disconnected` informs `Notifications` whether to dial
	// the peer if there is currently no active connection to it.
	//
	// NOTE: not offered by the current implementation
	async fn open_substream(&mut self, peer: PeerId) -> Result<(), ()>;

	/// Instruct `Notifications` to close substream for `peer`.
	//
	// NOTE: not offered by the current implementation
	async fn close_substream(&mut self, peer: PeerId) -> Result<(), ()>;

	/// Send synchronous `notification` to `peer`.
	fn send_sync_notification(&mut self, peer: &PeerId, notification: Vec<u8>);

	/// Send asynchronous `notification` to `peer`, allowing sender to exercise backpressure.
	///
	/// Returns an error if the peer doesn't exist.
	async fn send_async_notification(
		&mut self,
		peer: &PeerId,
		notification: Vec<u8>,
	) -> Result<(), error::Error>;

	/// Set handshake for the notification protocol replacing the old handshake.
	async fn set_handshake(&mut self, handshake: Vec<u8>) -> Result<(), ()>;

	/// Non-blocking variant of `set_handshake()` that attempts to update the handshake
	/// and returns an error if the channel is blocked.
	///
	/// Technically the function can return an error if the channel to `Notifications` is closed
	/// but that doesn't happen under normal operation.
	fn try_set_handshake(&mut self, handshake: Vec<u8>) -> Result<(), ()>;

	/// Get next event from the `Notifications` event stream.
	async fn next_event(&mut self) -> Option<NotificationEvent>;

	/// Make a copy of the object so it can be shared between protocol components
	/// who wish to have access to the same underlying notification protocol.
	fn clone(&mut self) -> Result<Box<dyn NotificationService>, ()>;

	/// Get protocol name of the `NotificationService`.
	fn protocol(&self) -> &ProtocolName;

	/// Get message sink of the peer.
	fn message_sink(&self, peer: &PeerId) -> Option<Box<dyn MessageSink>>;
}

/// Message sink for peers.
///
/// If protocol cannot use [`NotificationService`] to send notifications to peers and requires,
/// e.g., notifications to be sent in another task, the protocol may acquire a [`MessageSink`]
/// object for each peer by calling [`NotificationService::message_sink()`]. Calling this
/// function returns an object which allows the protocol to send notifications to the remote peer.
///
/// Use of this API is discouraged as it's not as performant as sending notifications through
/// [`NotificationService`] due to synchronization required to keep the underlying notification
/// sink up to date with possible sink replacement events.
#[async_trait::async_trait]
pub trait MessageSink: Send + Sync {
	/// Send synchronous `notification` to the peer associated with this [`MessageSink`].
	fn send_sync_notification(&self, notification: Vec<u8>);

	/// Send an asynchronous `notification` to to the peer associated with this [`MessageSink`],
	/// allowing sender to exercise backpressure.
	///
	/// Returns an error if the peer does not exist.
	async fn send_async_notification(&self, notification: Vec<u8>) -> Result<(), error::Error>;
}

/// Trait defining the behavior of a bandwidth sink.
pub trait BandwidthSink: Send + Sync {
	/// Get the number of bytes received.
	fn total_inbound(&self) -> u64;

	/// Get the number of bytes sent.
	fn total_outbound(&self) -> u64;
}

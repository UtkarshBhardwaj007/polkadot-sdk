// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! `V7` Primitives.
use alloc::{
	vec,
	vec::{IntoIter, Vec},
};
use bitvec::{field::BitField, slice::BitSlice, vec::BitVec};
use codec::{Decode, DecodeWithMemTracking, Encode};
use core::{
	marker::PhantomData,
	slice::{Iter, IterMut},
};
use scale_info::TypeInfo;

use sp_application_crypto::KeyTypeId;
use sp_arithmetic::{
	traits::{BaseArithmetic, Saturating},
	Perbill,
};
use sp_core::RuntimeDebug;
use sp_inherents::InherentIdentifier;
use sp_runtime::traits::{AppVerify, Header as HeaderT};

pub use sp_runtime::traits::{BlakeTwo256, Hash as HashT};

// Export some core primitives.
pub use polkadot_core_primitives::v2::{
	AccountId, AccountIndex, AccountPublic, Balance, Block, BlockId, BlockNumber, CandidateHash,
	ChainId, DownwardMessage, Hash, Header, InboundDownwardMessage, InboundHrmpMessage, Moment,
	Nonce, OutboundHrmpMessage, Remark, Signature, UncheckedExtrinsic,
};

// Export some polkadot-parachain primitives
pub use polkadot_parachain_primitives::primitives::{
	HeadData, HorizontalMessages, HrmpChannelId, Id, UpwardMessage, UpwardMessages, ValidationCode,
	ValidationCodeHash, LOWEST_PUBLIC_ID,
};

use serde::{Deserialize, Serialize};

pub use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
pub use sp_consensus_slots::Slot;
pub use sp_staking::SessionIndex;

/// Signed data.
mod signed;
pub use signed::{EncodeAs, Signed, UncheckedSigned};

pub mod async_backing;
pub mod executor_params;
pub mod slashing;

pub use async_backing::AsyncBackingParams;
pub use executor_params::{
	ExecutorParam, ExecutorParamError, ExecutorParams, ExecutorParamsHash, ExecutorParamsPrepHash,
};

mod metrics;
pub use metrics::{
	metric_definitions, RuntimeMetricLabel, RuntimeMetricLabelValue, RuntimeMetricLabelValues,
	RuntimeMetricLabels, RuntimeMetricOp, RuntimeMetricUpdate,
};

/// The key type ID for a collator key.
pub const COLLATOR_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"coll");
const LOG_TARGET: &str = "runtime::primitives";

mod collator_app {
	use sp_application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::COLLATOR_KEY_TYPE_ID);
}

/// Identity that collators use.
pub type CollatorId = collator_app::Public;

/// A Parachain collator keypair.
#[cfg(feature = "std")]
pub type CollatorPair = collator_app::Pair;

/// Signature on candidate's block data by a collator.
pub type CollatorSignature = collator_app::Signature;

/// The key type ID for a parachain validator key.
pub const PARACHAIN_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"para");

mod validator_app {
	use sp_application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::PARACHAIN_KEY_TYPE_ID);
}

/// Identity that parachain validators use when signing validation messages.
///
/// For now we assert that parachain validator set is exactly equivalent to the authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorId = validator_app::Public;

/// Trait required for type specific indices e.g. `ValidatorIndex` and `GroupIndex`
pub trait TypeIndex {
	/// Returns the index associated to this value.
	fn type_index(&self) -> usize;
}

/// Index of the validator is used as a lightweight replacement of the `ValidatorId` when
/// appropriate.
#[derive(
	Eq,
	Ord,
	PartialEq,
	PartialOrd,
	Copy,
	Clone,
	Encode,
	Decode,
	DecodeWithMemTracking,
	TypeInfo,
	RuntimeDebug,
)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash))]
pub struct ValidatorIndex(pub u32);

/// Index of an availability chunk.
///
/// The underlying type is identical to `ValidatorIndex`, because
/// the number of chunks will always be equal to the number of validators.
/// However, the chunk index held by a validator may not always be equal to its `ValidatorIndex`, so
/// we use a separate type to make code easier to read.
#[derive(Eq, Ord, PartialEq, PartialOrd, Copy, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash))]
pub struct ChunkIndex(pub u32);

impl From<ChunkIndex> for ValidatorIndex {
	fn from(c_index: ChunkIndex) -> Self {
		ValidatorIndex(c_index.0)
	}
}

impl From<ValidatorIndex> for ChunkIndex {
	fn from(v_index: ValidatorIndex) -> Self {
		ChunkIndex(v_index.0)
	}
}

impl From<u32> for ChunkIndex {
	fn from(n: u32) -> Self {
		ChunkIndex(n)
	}
}

// We should really get https://github.com/paritytech/polkadot/issues/2403 going ..
impl From<u32> for ValidatorIndex {
	fn from(n: u32) -> Self {
		ValidatorIndex(n)
	}
}

impl TypeIndex for ValidatorIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

sp_application_crypto::with_pair! {
	/// A Parachain validator keypair.
	pub type ValidatorPair = validator_app::Pair;
}

/// Signature with which parachain validators sign blocks.
///
/// For now we assert that parachain validator set is exactly equivalent to the authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorSignature = validator_app::Signature;

/// A declarations of storage keys where an external observer can find some interesting data.
pub mod well_known_keys {
	use super::{HrmpChannelId, Id, WellKnownKey};
	use alloc::vec::Vec;
	use codec::Encode as _;
	use hex_literal::hex;
	use sp_io::hashing::twox_64;

	// A note on generating these magic values below:
	//
	// The `StorageValue`, such as `ACTIVE_CONFIG` was obtained by calling:
	//
	//     ActiveConfig::<T>::hashed_key()
	//
	// The `StorageMap` values require `prefix`, and for example for `hrmp_egress_channel_index`,
	// it could be obtained like:
	//
	//     HrmpEgressChannelsIndex::<T>::prefix_hash();
	//

	/// The current epoch index.
	///
	/// The storage item should be access as a `u64` encoded value.
	pub const EPOCH_INDEX: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f38316cbf8fa0da822a20ac1c55bf1be3"];

	/// The current relay chain block randomness
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const CURRENT_BLOCK_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087fd077dfdb8adb10f78f10a5df8742c545"];

	/// The randomness for one epoch ago
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const ONE_EPOCH_AGO_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f7ce678799d3eff024253b90e84927cc6"];

	/// The randomness for two epochs ago
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const TWO_EPOCHS_AGO_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f7a414cb008e0e61e46722aa60abdd672"];

	/// The current slot number.
	///
	/// The storage entry should be accessed as a `Slot` encoded value.
	pub const CURRENT_SLOT: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f06155b3cd9a8c9e5e9a23fd5dc13a5ed"];

	/// The currently active host configuration.
	///
	/// The storage entry should be accessed as an `AbridgedHostConfiguration` encoded value.
	pub const ACTIVE_CONFIG: &[u8] =
		&hex!["06de3d8a54d27e44a9d5ce189618f22db4b49d95320d9021994c850f25b8e385"];

	/// The authorities for the current epoch.
	///
	/// The storage entry should be accessed as an `Vec<(AuthorityId, BabeAuthorityWeight)>` encoded
	/// value.
	pub const AUTHORITIES: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f5e0621c4869aa60c02be9adcc98a0d1d"];

	/// The authorities for the next epoch.
	///
	/// The storage entry should be accessed as an `Vec<(AuthorityId, BabeAuthorityWeight)>` encoded
	/// value.
	pub const NEXT_AUTHORITIES: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087faacf00b9b41fda7a9268821c2a2b3e4c"];

	/// Hash of the committed head data for a given registered para.
	///
	/// The storage entry stores wrapped `HeadData(Vec<u8>)`.
	pub fn para_head(para_id: Id) -> Vec<u8> {
		let prefix = hex!["cd710b30bd2eab0352ddcc26417aa1941b3c252fcb29d88eff4f3de5de4476c3"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The upward message dispatch queue for the given para id.
	///
	/// The storage entry stores a tuple of two values:
	///
	/// - `count: u32`, the number of messages currently in the queue for given para,
	/// - `total_size: u32`, the total size of all messages in the queue.
	#[deprecated = "Use `relay_dispatch_queue_remaining_capacity` instead"]
	pub fn relay_dispatch_queue_size(para_id: Id) -> Vec<u8> {
		let prefix = hex!["f5207f03cfdce586301014700e2c2593fad157e461d71fd4c1f936839a5f1f3e"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// Type safe version of `relay_dispatch_queue_size`.
	#[deprecated = "Use `relay_dispatch_queue_remaining_capacity` instead"]
	pub fn relay_dispatch_queue_size_typed(para: Id) -> WellKnownKey<(u32, u32)> {
		#[allow(deprecated)]
		relay_dispatch_queue_size(para).into()
	}

	/// The upward message dispatch queue remaining capacity for the given para id.
	///
	/// The storage entry stores a tuple of two values:
	///
	/// - `count: u32`, the number of additional messages which may be enqueued for the given para,
	/// - `total_size: u32`, the total size of additional messages which may be enqueued for the
	/// given para.
	pub fn relay_dispatch_queue_remaining_capacity(para_id: Id) -> WellKnownKey<(u32, u32)> {
		(b":relay_dispatch_queue_remaining_capacity", para_id).encode().into()
	}

	/// The HRMP channel for the given identifier.
	///
	/// The storage entry should be accessed as an `AbridgedHrmpChannel` encoded value.
	pub fn hrmp_channels(channel: HrmpChannelId) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627cb6604cff828a6e3f579ca6c59ace013d"];

		channel.using_encoded(|channel: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(channel).iter())
				.chain(channel.iter())
				.cloned()
				.collect()
		})
	}

	/// The list of inbound channels for the given para.
	///
	/// The storage entry stores a `Vec<ParaId>`
	pub fn hrmp_ingress_channel_index(para_id: Id) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627c1d3719f5b0b12c7105c073c507445948"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The list of outbound channels for the given para.
	///
	/// The storage entry stores a `Vec<ParaId>`
	pub fn hrmp_egress_channel_index(para_id: Id) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627cf12b746dcf32e843354583c9702cc020"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The MQC head for the downward message queue of the given para. See more in the `Dmp` module.
	///
	/// The storage entry stores a `Hash`. This is polkadot hash which is at the moment
	/// `blake2b-256`.
	pub fn dmq_mqc_head(para_id: Id) -> Vec<u8> {
		let prefix = hex!["63f78c98723ddc9073523ef3beefda0c4d7fefc408aac59dbfe80a72ac8e3ce5"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The signal that indicates whether the parachain should go-ahead with the proposed validation
	/// code upgrade.
	///
	/// The storage entry stores a value of `UpgradeGoAhead` type.
	pub fn upgrade_go_ahead_signal(para_id: Id) -> Vec<u8> {
		let prefix = hex!["cd710b30bd2eab0352ddcc26417aa1949e94c040f5e73d9b7addd6cb603d15d3"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The signal that indicates whether the parachain is disallowed to signal an upgrade at this
	/// relay-parent.
	///
	/// The storage entry stores a value of `UpgradeRestriction` type.
	pub fn upgrade_restriction_signal(para_id: Id) -> Vec<u8> {
		let prefix = hex!["cd710b30bd2eab0352ddcc26417aa194f27bbb460270642b5bcaf032ea04d56a"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}
}

/// Unique identifier for the Parachains Inherent
pub const PARACHAINS_INHERENT_IDENTIFIER: InherentIdentifier = *b"parachn0";

/// The key type ID for parachain assignment key.
pub const ASSIGNMENT_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"asgn");

/// Compressed or not the wasm blob can never be less than 9 bytes.
pub const MIN_CODE_SIZE: u32 = 9;

/// Maximum compressed code size we support right now.
/// At the moment we have runtime upgrade on chain, which restricts scalability severely. If we want
/// to have bigger values, we should fix that first.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
/// * when detecting a code decompression bomb in the client
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_CODE_SIZE: u32 = 3 * 1024 * 1024;

/// Maximum head data size we support right now.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_HEAD_DATA_SIZE: u32 = 1 * 1024 * 1024;

/// Maximum PoV size we support right now.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
/// * when detecting a PoV decompression bomb in the client
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_POV_SIZE: u32 = 10 * 1024 * 1024;

/// Default queue size we use for the on-demand order book.
///
/// Can be adjusted in configuration.
pub const ON_DEMAND_DEFAULT_QUEUE_MAX_SIZE: u32 = 10_000;

/// Maximum for maximum queue size.
///
/// Setting `on_demand_queue_max_size` to a value higher than this is unsound. This is more a
/// theoretical limit, just below enough what the target type supports, so comparisons are possible
/// even with indices that are overflowing the underyling type.
pub const ON_DEMAND_MAX_QUEUE_MAX_SIZE: u32 = 1_000_000_000;

/// Backing votes threshold used from the host prior to runtime API version 6 and from the runtime
/// prior to v9 configuration migration.
pub const LEGACY_MIN_BACKING_VOTES: u32 = 2;

/// Default value for `SchedulerParams.lookahead`
pub const DEFAULT_SCHEDULING_LOOKAHEAD: u32 = 3;

// The public key of a keypair used by a validator for determining assignments
/// to approve included parachain candidates.
mod assignment_app {
	use sp_application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::ASSIGNMENT_KEY_TYPE_ID);
}

/// The public key of a keypair used by a validator for determining assignments
/// to approve included parachain candidates.
pub type AssignmentId = assignment_app::Public;

sp_application_crypto::with_pair! {
	/// The full keypair used by a validator for determining assignments to approve included
	/// parachain candidates.
	pub type AssignmentPair = assignment_app::Pair;
}

/// The index of the candidate in the list of candidates fully included as-of the block.
pub type CandidateIndex = u32;

/// Get a collator signature payload on a relay-parent, block-data combo.
pub fn collator_signature_payload<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
	validation_code_hash: &ValidationCodeHash,
) -> [u8; 132] {
	// 32-byte hash length is protected in a test below.
	let mut payload = [0u8; 132];

	payload[0..32].copy_from_slice(relay_parent.as_ref());
	u32::from(*para_id).using_encoded(|s| payload[32..32 + s.len()].copy_from_slice(s));
	payload[36..68].copy_from_slice(persisted_validation_data_hash.as_ref());
	payload[68..100].copy_from_slice(pov_hash.as_ref());
	payload[100..132].copy_from_slice(validation_code_hash.as_ref());

	payload
}

pub(crate) fn check_collator_signature<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
	validation_code_hash: &ValidationCodeHash,
	collator: &CollatorId,
	signature: &CollatorSignature,
) -> Result<(), ()> {
	let payload = collator_signature_payload(
		relay_parent,
		para_id,
		persisted_validation_data_hash,
		pov_hash,
		validation_code_hash,
	);

	if signature.verify(&payload[..], collator) {
		Ok(())
	} else {
		Err(())
	}
}

/// A unique descriptor of the candidate receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct CandidateDescriptor<H = Hash> {
	/// The ID of the para this is a candidate for.
	pub para_id: Id,
	/// The hash of the relay-chain block this is executed in the context of.
	pub relay_parent: H,
	/// The collator's sr25519 public key.
	pub collator: CollatorId,
	/// The blake2-256 hash of the persisted validation data. This is extra data derived from
	/// relay-chain state which may vary based on bitfields included before the candidate.
	/// Thus it cannot be derived entirely from the relay-parent.
	pub persisted_validation_data_hash: Hash,
	/// The blake2-256 hash of the PoV.
	pub pov_hash: Hash,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the `pov_hash`.
	pub signature: CollatorSignature,
	/// Hash of the para header that is being generated by this candidate.
	pub para_head: Hash,
	/// The blake2-256 hash of the validation code bytes.
	pub validation_code_hash: ValidationCodeHash,
}

impl<H: AsRef<[u8]>> CandidateDescriptor<H> {
	/// Check the signature of the collator within this descriptor.
	pub fn check_collator_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.para_id,
			&self.persisted_validation_data_hash,
			&self.pov_hash,
			&self.validation_code_hash,
			&self.collator,
			&self.signature,
		)
	}
}

/// A candidate-receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub struct CandidateReceipt<H = Hash> {
	/// The descriptor of the candidate.
	pub descriptor: CandidateDescriptor<H>,
	/// The hash of the encoded commitments made as a result of candidate execution.
	pub commitments_hash: Hash,
}

impl<H> CandidateReceipt<H> {
	/// Get a reference to the candidate descriptor.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}

	/// Computes the blake2-256 hash of the receipt.
	pub fn hash(&self) -> CandidateHash
	where
		H: Encode,
	{
		CandidateHash(BlakeTwo256::hash_of(self))
	}
}

/// A candidate-receipt with commitments directly included.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct CommittedCandidateReceipt<H = Hash> {
	/// The descriptor of the candidate.
	pub descriptor: CandidateDescriptor<H>,
	/// The commitments of the candidate receipt.
	pub commitments: CandidateCommitments,
}

impl<H> CommittedCandidateReceipt<H> {
	/// Get a reference to the candidate descriptor.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}
}

impl<H: Clone> CommittedCandidateReceipt<H> {
	/// Transforms this into a plain `CandidateReceipt`.
	pub fn to_plain(&self) -> CandidateReceipt<H> {
		CandidateReceipt {
			descriptor: self.descriptor.clone(),
			commitments_hash: self.commitments.hash(),
		}
	}

	/// Computes the hash of the committed candidate receipt.
	///
	/// This computes the canonical hash, not the hash of the directly encoded data.
	/// Thus this is a shortcut for `candidate.to_plain().hash()`.
	pub fn hash(&self) -> CandidateHash
	where
		H: Encode,
	{
		self.to_plain().hash()
	}

	/// Does this committed candidate receipt corresponds to the given [`CandidateReceipt`]?
	pub fn corresponds_to(&self, receipt: &CandidateReceipt<H>) -> bool
	where
		H: PartialEq,
	{
		receipt.descriptor == self.descriptor && receipt.commitments_hash == self.commitments.hash()
	}
}

impl PartialOrd for CommittedCandidateReceipt {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for CommittedCandidateReceipt {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		// TODO: compare signatures or something more sane
		// https://github.com/paritytech/polkadot/issues/222
		self.descriptor()
			.para_id
			.cmp(&other.descriptor().para_id)
			.then_with(|| self.commitments.head_data.cmp(&other.commitments.head_data))
	}
}

/// The validation data provides information about how to create the inputs for validation of a
/// candidate. This information is derived from the chain state and will vary from para to para,
/// although some fields may be the same for every para.
///
/// Since this data is used to form inputs to the validation function, it needs to be persisted by
/// the availability system to avoid dependence on availability of the relay-chain state.
///
/// Furthermore, the validation data acts as a way to authorize the additional data the collator
/// needs to pass to the validation function. For example, the validation function can check whether
/// the incoming messages (e.g. downward messages) were actually sent by using the data provided in
/// the validation data using so called MQC heads.
///
/// Since the commitments of the validation function are checked by the relay-chain, secondary
/// checkers can rely on the invariant that the relay-chain only includes para-blocks for which
/// these checks have already been done. As such, there is no need for the validation data used to
/// inform validators and collators about the checks the relay-chain will perform to be persisted by
/// the availability system.
///
/// The `PersistedValidationData` should be relatively lightweight primarily because it is
/// constructed during inclusion for each candidate and therefore lies on the critical path of
/// inclusion.
#[derive(PartialEq, Eq, Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Default))]
pub struct PersistedValidationData<H = Hash, N = BlockNumber> {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The relay-chain block number this is in the context of.
	pub relay_parent_number: N,
	/// The relay-chain block storage root this is in the context of.
	pub relay_parent_storage_root: H,
	/// The maximum legal size of a POV block, in bytes.
	pub max_pov_size: u32,
}

impl<H: Encode, N: Encode> PersistedValidationData<H, N> {
	/// Compute the blake2-256 hash of the persisted validation data.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Default, Hash))]
pub struct CandidateCommitments<N = BlockNumber> {
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: UpwardMessages,
	/// Horizontal messages sent by the parachain.
	pub horizontal_messages: HorizontalMessages,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	pub head_data: HeadData,
	/// The number of messages processed from the DMQ.
	pub processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are
	/// processed.
	pub hrmp_watermark: N,
}

impl CandidateCommitments {
	/// Compute the blake2-256 hash of the commitments.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// A bitfield concerning availability of backed candidates.
///
/// Every bit refers to an availability core index.
#[derive(PartialEq, Eq, Clone, Encode, Decode, DecodeWithMemTracking, RuntimeDebug, TypeInfo)]
pub struct AvailabilityBitfield(pub BitVec<u8, bitvec::order::Lsb0>);

impl From<BitVec<u8, bitvec::order::Lsb0>> for AvailabilityBitfield {
	fn from(inner: BitVec<u8, bitvec::order::Lsb0>) -> Self {
		AvailabilityBitfield(inner)
	}
}

/// A signed compact statement, suitable to be sent to the chain.
pub type SignedStatement = Signed<CompactStatement>;
/// A signed compact statement, with signature not yet checked.
pub type UncheckedSignedStatement = UncheckedSigned<CompactStatement>;

/// A bitfield signed by a particular validator about the availability of pending candidates.
pub type SignedAvailabilityBitfield = Signed<AvailabilityBitfield>;
/// A signed bitfield with signature not yet checked.
pub type UncheckedSignedAvailabilityBitfield = UncheckedSigned<AvailabilityBitfield>;

/// A set of signed availability bitfields. Should be sorted by validator index, ascending.
pub type SignedAvailabilityBitfields = Vec<SignedAvailabilityBitfield>;
/// A set of unchecked signed availability bitfields. Should be sorted by validator index,
/// ascending.
pub type UncheckedSignedAvailabilityBitfields = Vec<UncheckedSignedAvailabilityBitfield>;

/// A backed (or backable, depending on context) candidate.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct BackedCandidate<H = Hash> {
	/// The candidate referred to.
	candidate: CommittedCandidateReceipt<H>,
	/// The validity votes themselves, expressed as signatures.
	validity_votes: Vec<ValidityAttestation>,
	/// The indices of the validators within the group, expressed as a bitfield. Is extended
	/// beyond the backing group size to contain the assigned core index.
	validator_indices: BitVec<u8, bitvec::order::Lsb0>,
}

impl<H> BackedCandidate<H> {
	/// Constructor
	pub fn new(
		candidate: CommittedCandidateReceipt<H>,
		validity_votes: Vec<ValidityAttestation>,
		validator_indices: BitVec<u8, bitvec::order::Lsb0>,
		core_index: CoreIndex,
	) -> Self {
		let mut instance = Self { candidate, validity_votes, validator_indices };
		instance.inject_core_index(core_index);
		instance
	}

	/// Get a reference to the descriptor of the candidate.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.candidate.descriptor
	}

	/// Get a reference to the committed candidate receipt of the candidate.
	pub fn candidate(&self) -> &CommittedCandidateReceipt<H> {
		&self.candidate
	}

	/// Get a reference to the validity votes of the candidate.
	pub fn validity_votes(&self) -> &[ValidityAttestation] {
		&self.validity_votes
	}

	/// Get a mutable reference to validity votes of the para.
	pub fn validity_votes_mut(&mut self) -> &mut Vec<ValidityAttestation> {
		&mut self.validity_votes
	}

	/// Compute this candidate's hash.
	pub fn hash(&self) -> CandidateHash
	where
		H: Clone + Encode,
	{
		self.candidate.hash()
	}

	/// Get this candidate's receipt.
	pub fn receipt(&self) -> CandidateReceipt<H>
	where
		H: Clone,
	{
		self.candidate.to_plain()
	}

	/// Get a copy of the validator indices and the assumed core index, if any.
	pub fn validator_indices_and_core_index(
		&self,
	) -> (&BitSlice<u8, bitvec::order::Lsb0>, Option<CoreIndex>) {
		// `BackedCandidate::validity_indices` are extended to store a 8 bit core index.
		let core_idx_offset = self.validator_indices.len().saturating_sub(8);
		if core_idx_offset > 0 {
			let (validator_indices_slice, core_idx_slice) =
				self.validator_indices.split_at(core_idx_offset);
			return (validator_indices_slice, Some(CoreIndex(core_idx_slice.load::<u8>() as u32)));
		}

		(&self.validator_indices, None)
	}

	/// Inject a core index in the validator_indices bitvec.
	fn inject_core_index(&mut self, core_index: CoreIndex) {
		let core_index_to_inject: BitVec<u8, bitvec::order::Lsb0> =
			BitVec::from_vec(vec![core_index.0 as u8]);
		self.validator_indices.extend(core_index_to_inject);
	}

	/// Update the validator indices and core index in the candidate.
	pub fn set_validator_indices_and_core_index(
		&mut self,
		new_indices: BitVec<u8, bitvec::order::Lsb0>,
		maybe_core_index: Option<CoreIndex>,
	) {
		self.validator_indices = new_indices;

		if let Some(core_index) = maybe_core_index {
			self.inject_core_index(core_index);
		}
	}
}

/// Verify the backing of the given candidate.
///
/// Provide a lookup from the index of a validator within the group assigned to this para,
/// as opposed to the index of the validator within the overall validator set, as well as
/// the number of validators in the group.
///
/// Also provide the signing context.
///
/// Returns either an error, indicating that one of the signatures was invalid or that the index
/// was out-of-bounds, or the number of signatures checked.
pub fn check_candidate_backing<H: AsRef<[u8]> + Clone + Encode + core::fmt::Debug>(
	candidate_hash: CandidateHash,
	validity_votes: &[ValidityAttestation],
	validator_indices: &BitSlice<u8, bitvec::order::Lsb0>,
	signing_context: &SigningContext<H>,
	group_len: usize,
	validator_lookup: impl Fn(usize) -> Option<ValidatorId>,
) -> Result<usize, ()> {
	if validator_indices.len() != group_len {
		log::debug!(
			target: LOG_TARGET,
			"Check candidate backing: indices mismatch: group_len = {} , indices_len = {}",
			group_len,
			validator_indices.len(),
		);
		return Err(())
	}

	if validity_votes.len() > group_len {
		log::debug!(
			target: LOG_TARGET,
			"Check candidate backing: Too many votes, expected: {}, found: {}",
			group_len,
			validity_votes.len(),
		);
		return Err(())
	}

	let mut signed = 0;
	for ((val_in_group_idx, _), attestation) in validator_indices
		.iter()
		.enumerate()
		.filter(|(_, signed)| **signed)
		.zip(validity_votes.iter())
	{
		let validator_id = validator_lookup(val_in_group_idx).ok_or(())?;
		let payload = attestation.signed_payload(candidate_hash, signing_context);
		let sig = attestation.signature();

		if sig.verify(&payload[..], &validator_id) {
			signed += 1;
		} else {
			log::debug!(
				target: LOG_TARGET,
				"Check candidate backing: Invalid signature. validator_id = {:?}, validator_index = {} ",
				validator_id,
				val_in_group_idx,
			);
			return Err(())
		}
	}

	if signed != validity_votes.len() {
		log::error!(
			target: LOG_TARGET,
			"Check candidate backing: Too many signatures, expected = {}, found = {}",
			validity_votes.len(),
			signed,
		);
		return Err(())
	}

	Ok(signed)
}

/// The unique (during session) index of a core.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Default,
	PartialOrd,
	Ord,
	Eq,
	PartialEq,
	Clone,
	Copy,
	TypeInfo,
	RuntimeDebug,
)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct CoreIndex(pub u32);

impl From<u32> for CoreIndex {
	fn from(i: u32) -> CoreIndex {
		CoreIndex(i)
	}
}

impl TypeIndex for CoreIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

/// The unique (during session) index of a validator group.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Default,
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	TypeInfo,
	PartialOrd,
	Ord,
)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct GroupIndex(pub u32);

impl From<u32> for GroupIndex {
	fn from(i: u32) -> GroupIndex {
		GroupIndex(i)
	}
}

impl TypeIndex for GroupIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

/// A claim on authoring the next block for a given parathread (on-demand parachain).
#[derive(Clone, Encode, Decode, TypeInfo, PartialEq, RuntimeDebug)]
pub struct ParathreadClaim(pub Id, pub Option<CollatorId>);

/// An entry tracking a claim to ensure it does not pass the maximum number of retries.
#[derive(Clone, Encode, Decode, TypeInfo, PartialEq, RuntimeDebug)]
pub struct ParathreadEntry {
	/// The claim.
	pub claim: ParathreadClaim,
	/// Number of retries
	pub retries: u32,
}

/// A helper data-type for tracking validator-group rotations.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct GroupRotationInfo<N = BlockNumber> {
	/// The block number where the session started.
	pub session_start_block: N,
	/// How often groups rotate. 0 means never.
	pub group_rotation_frequency: N,
	/// The current block number.
	pub now: N,
}

impl GroupRotationInfo {
	/// Returns the index of the group needed to validate the core at the given index, assuming
	/// the given number of cores.
	///
	/// `core_index` should be less than `cores`, which is capped at `u32::max()`.
	pub fn group_for_core(&self, core_index: CoreIndex, cores: usize) -> GroupIndex {
		if self.group_rotation_frequency == 0 {
			return GroupIndex(core_index.0)
		}
		if cores == 0 {
			return GroupIndex(0)
		}

		let cores = core::cmp::min(cores, u32::MAX as usize);
		let blocks_since_start = self.now.saturating_sub(self.session_start_block);
		let rotations = blocks_since_start / self.group_rotation_frequency;

		// g = c + r mod cores

		let idx = (core_index.0 as usize + rotations as usize) % cores;
		GroupIndex(idx as u32)
	}

	/// Returns the index of the group assigned to the given core. This does no checking or
	/// whether the group index is in-bounds.
	///
	/// `core_index` should be less than `cores`, which is capped at `u32::max()`.
	pub fn core_for_group(&self, group_index: GroupIndex, cores: usize) -> CoreIndex {
		if self.group_rotation_frequency == 0 {
			return CoreIndex(group_index.0)
		}
		if cores == 0 {
			return CoreIndex(0)
		}

		let cores = core::cmp::min(cores, u32::MAX as usize);
		let blocks_since_start = self.now.saturating_sub(self.session_start_block);
		let rotations = blocks_since_start / self.group_rotation_frequency;
		let rotations = rotations % cores as u32;

		// g = c + r mod cores
		// c = g - r mod cores
		// x = x + cores mod cores
		// c = (g + cores) - r mod cores

		let idx = (group_index.0 as usize + cores - rotations as usize) % cores;
		CoreIndex(idx as u32)
	}

	/// Create a new `GroupRotationInfo` with one further rotation applied.
	pub fn bump_rotation(&self) -> Self {
		GroupRotationInfo {
			session_start_block: self.session_start_block,
			group_rotation_frequency: self.group_rotation_frequency,
			now: self.next_rotation_at(),
		}
	}
}

impl<N: Saturating + BaseArithmetic + Copy> GroupRotationInfo<N> {
	/// Returns the block number of the next rotation after the current block. If the current block
	/// is 10 and the rotation frequency is 5, this should return 15.
	pub fn next_rotation_at(&self) -> N {
		let cycle_once = self.now + self.group_rotation_frequency;
		cycle_once -
			(cycle_once.saturating_sub(self.session_start_block) % self.group_rotation_frequency)
	}

	/// Returns the block number of the last rotation before or including the current block. If the
	/// current block is 10 and the rotation frequency is 5, this should return 10.
	pub fn last_rotation_at(&self) -> N {
		self.now -
			(self.now.saturating_sub(self.session_start_block) % self.group_rotation_frequency)
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct OccupiedCore<H = Hash, N = BlockNumber> {
	// NOTE: this has no ParaId as it can be deduced from the candidate descriptor.
	/// If this core is freed by availability, this is the assignment that is next up on this
	/// core, if any. None if there is nothing queued for this core.
	pub next_up_on_available: Option<ScheduledCore>,
	/// The relay-chain block number this began occupying the core at.
	pub occupied_since: N,
	/// The relay-chain block this will time-out at, if any.
	pub time_out_at: N,
	/// If this core is freed by being timed-out, this is the assignment that is next up on this
	/// core. None if there is nothing queued for this core or there is no possibility of timing
	/// out.
	pub next_up_on_time_out: Option<ScheduledCore>,
	/// A bitfield with 1 bit for each validator in the set. `1` bits mean that the corresponding
	/// validators has attested to availability on-chain. A 2/3+ majority of `1` bits means that
	/// this will be available.
	pub availability: BitVec<u8, bitvec::order::Lsb0>,
	/// The group assigned to distribute availability pieces of this candidate.
	pub group_responsible: GroupIndex,
	/// The hash of the candidate occupying the core.
	pub candidate_hash: CandidateHash,
	/// The descriptor of the candidate occupying the core.
	pub candidate_descriptor: CandidateDescriptor<H>,
}

impl<H, N> OccupiedCore<H, N> {
	/// Get the Para currently occupying this core.
	pub fn para_id(&self) -> Id {
		self.candidate_descriptor.para_id
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ScheduledCore {
	/// The ID of a para scheduled.
	pub para_id: Id,
	/// DEPRECATED: see: <https://github.com/paritytech/polkadot/issues/7575>
	///
	/// Will be removed in a future version.
	pub collator: Option<CollatorId>,
}

/// The state of a particular availability core.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum CoreState<H = Hash, N = BlockNumber> {
	/// The core is currently occupied.
	#[codec(index = 0)]
	Occupied(OccupiedCore<H, N>),
	/// The core is currently free, with a para scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	#[codec(index = 1)]
	Scheduled(ScheduledCore),
	/// The core is currently free and there is nothing scheduled. This can be the case for
	/// parathread cores when there are no parathread blocks queued. Parachain cores will never be
	/// left idle.
	#[codec(index = 2)]
	Free,
}

impl<N> CoreState<N> {
	/// Returns the scheduled `ParaId` for the core or `None` if nothing is scheduled.
	///
	/// This function is deprecated. `ClaimQueue` should be used to obtain the scheduled `ParaId`s
	/// for each core.
	#[deprecated(
		note = "`para_id` will be removed. Use `ClaimQueue` to query the scheduled `para_id` instead."
	)]
	pub fn para_id(&self) -> Option<Id> {
		match self {
			Self::Occupied(ref core) => core.next_up_on_available.as_ref().map(|n| n.para_id),
			Self::Scheduled(core) => Some(core.para_id),
			Self::Free => None,
		}
	}

	/// Is this core state `Self::Occupied`?
	pub fn is_occupied(&self) -> bool {
		matches!(self, Self::Occupied(_))
	}
}

/// An assumption being made about the state of an occupied core.
#[derive(Clone, Copy, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq, Eq, Hash))]
pub enum OccupiedCoreAssumption {
	/// The candidate occupying the core was made available and included to free the core.
	#[codec(index = 0)]
	Included,
	/// The candidate occupying the core timed out and freed the core without advancing the para.
	#[codec(index = 1)]
	TimedOut,
	/// The core was not occupied to begin with.
	#[codec(index = 2)]
	Free,
}

/// An event concerning a candidate.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum CandidateEvent<H = Hash> {
	/// This candidate receipt was backed in the most recent block.
	/// This includes the core index the candidate is now occupying.
	#[codec(index = 0)]
	CandidateBacked(CandidateReceipt<H>, HeadData, CoreIndex, GroupIndex),
	/// This candidate receipt was included and became a parablock at the most recent block.
	/// This includes the core index the candidate was occupying as well as the group responsible
	/// for backing the candidate.
	#[codec(index = 1)]
	CandidateIncluded(CandidateReceipt<H>, HeadData, CoreIndex, GroupIndex),
	/// This candidate receipt was not made available in time and timed out.
	/// This includes the core index the candidate was occupying.
	#[codec(index = 2)]
	CandidateTimedOut(CandidateReceipt<H>, HeadData, CoreIndex),
}

/// Scraped runtime backing votes and resolved disputes.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ScrapedOnChainVotes<H: Encode + Decode = Hash> {
	/// The session in which the block was included.
	pub session: SessionIndex,
	/// Set of backing validators for each candidate, represented by its candidate
	/// receipt.
	pub backing_validators_per_candidate:
		Vec<(CandidateReceipt<H>, Vec<(ValidatorIndex, ValidityAttestation)>)>,
	/// On-chain-recorded set of disputes.
	/// Note that the above `backing_validators` are
	/// unrelated to the backers of the disputes candidates.
	pub disputes: MultiDisputeStatementSet,
}

/// A vote of approval on a candidate.
#[derive(Clone, RuntimeDebug)]
pub struct ApprovalVote(pub CandidateHash);

impl ApprovalVote {
	/// Yields the signing payload for this approval vote.
	pub fn signing_payload(&self, session_index: SessionIndex) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"APPR";

		(MAGIC, &self.0, session_index).encode()
	}
}

/// A vote of approval for multiple candidates.
#[derive(Clone, RuntimeDebug)]
pub struct ApprovalVoteMultipleCandidates<'a>(pub &'a [CandidateHash]);

impl<'a> ApprovalVoteMultipleCandidates<'a> {
	/// Yields the signing payload for this approval vote.
	pub fn signing_payload(&self, session_index: SessionIndex) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"APPR";
		// Make this backwards compatible with `ApprovalVote` so if we have just on candidate the
		// signature will look the same.
		// This gives us the nice benefit that old nodes can still check signatures when len is 1
		// and the new node can check the signature coming from old nodes.
		if self.0.len() == 1 {
			(MAGIC, self.0.first().expect("QED: we just checked"), session_index).encode()
		} else {
			(MAGIC, &self.0, session_index).encode()
		}
	}
}

/// Approval voting configuration parameters
#[derive(
	RuntimeDebug,
	Copy,
	Clone,
	PartialEq,
	Encode,
	Decode,
	DecodeWithMemTracking,
	TypeInfo,
	serde::Serialize,
	serde::Deserialize,
)]
pub struct ApprovalVotingParams {
	/// The maximum number of candidates `approval-voting` can vote for with
	/// a single signatures.
	///
	/// Setting it to 1, means we send the approval as soon as we have it available.
	pub max_approval_coalesce_count: u32,
}

impl Default for ApprovalVotingParams {
	fn default() -> Self {
		Self { max_approval_coalesce_count: 1 }
	}
}

/// Custom validity errors used in Polkadot while validating transactions.
#[repr(u8)]
pub enum ValidityError {
	/// The Ethereum signature is invalid.
	InvalidEthereumSignature = 0,
	/// The signer has no claim.
	SignerHasNoClaim = 1,
	/// No permission to execute the call.
	NoPermission = 2,
	/// An invalid statement was made for a claim.
	InvalidStatement = 3,
}

impl From<ValidityError> for u8 {
	fn from(err: ValidityError) -> Self {
		err as u8
	}
}

/// Abridged version of `HostConfiguration` (from the `Configuration` parachains host runtime
/// module) meant to be used by a parachain or PDK such as cumulus.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct AbridgedHostConfiguration {
	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub max_upward_queue_size: u32,
	/// The maximum size of an upward message that can be sent by a candidate.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_size: u32,
	/// The maximum number of messages that a candidate can contain.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_num_per_candidate: u32,
	/// The maximum number of outbound HRMP messages can be sent by a candidate.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_max_message_num_per_candidate: u32,
	/// The minimum period, in blocks, between which parachains can update their validation code.
	pub validation_upgrade_cooldown: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
	/// Asynchronous backing parameters.
	pub async_backing_params: AsyncBackingParams,
}

/// Abridged version of `HrmpChannel` (from the `Hrmp` parachains host runtime module) meant to be
/// used by a parachain or PDK such as cumulus.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Default, PartialEq))]
pub struct AbridgedHrmpChannel {
	/// The maximum number of messages that can be pending in the channel at once.
	pub max_capacity: u32,
	/// The maximum total size of the messages that can be pending in the channel at once.
	pub max_total_size: u32,
	/// The maximum message size that could be put into the channel.
	pub max_message_size: u32,
	/// The current number of messages pending in the channel.
	/// Invariant: should be less or equal to `max_capacity`.s`.
	pub msg_count: u32,
	/// The total size in bytes of all message payloads in the channel.
	/// Invariant: should be less or equal to `max_total_size`.
	pub total_size: u32,
	/// A head of the Message Queue Chain for this channel. Each link in this chain has a form:
	/// `(prev_head, B, H(M))`, where
	/// - `prev_head`: is the previous value of `mqc_head` or zero if none.
	/// - `B`: is the [relay-chain] block number in which a message was appended
	/// - `H(M)`: is the hash of the message being appended.
	/// This value is initialized to a special value that consists of all zeroes which indicates
	/// that no messages were previously added.
	pub mqc_head: Option<Hash>,
}

/// A possible upgrade restriction that prevents a parachain from performing an upgrade.
#[derive(Copy, Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
pub enum UpgradeRestriction {
	/// There is an upgrade restriction and there are no details about its specifics nor how long
	/// it could last.
	#[codec(index = 0)]
	Present,
}

/// A struct that the relay-chain communicates to a parachain indicating what course of action the
/// parachain should take in the coordinated parachain validation code upgrade process.
///
/// This data type appears in the last step of the upgrade process. After the parachain observes it
/// and reacts to it the upgrade process concludes.
#[derive(Copy, Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
pub enum UpgradeGoAhead {
	/// Abort the upgrade process. There is something wrong with the validation code previously
	/// submitted by the parachain. This variant can also be used to prevent upgrades by the
	/// governance should an emergency emerge.
	///
	/// The expected reaction on this variant is that the parachain will admit this message and
	/// remove all the data about the pending upgrade. Depending on the nature of the problem (to
	/// be examined offchain for now), it can try to send another validation code or just retry
	/// later.
	#[codec(index = 0)]
	Abort,
	/// Apply the pending code change. The parablock that is built on a relay-parent that is
	/// descendant of the relay-parent where the parachain observed this signal must use the
	/// upgraded validation code.
	#[codec(index = 1)]
	GoAhead,
}

/// Consensus engine id for polkadot v1 consensus engine.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"POL1";

/// A consensus log item for polkadot validation. To be used with [`POLKADOT_ENGINE_ID`].
#[derive(Decode, Encode, Clone, PartialEq, Eq)]
pub enum ConsensusLog {
	/// A parachain upgraded its code.
	#[codec(index = 1)]
	ParaUpgradeCode(Id, ValidationCodeHash),
	/// A parachain scheduled a code upgrade.
	#[codec(index = 2)]
	ParaScheduleUpgradeCode(Id, ValidationCodeHash, BlockNumber),
	/// Governance requests to auto-approve every candidate included up to the given block
	/// number in the current chain, inclusive.
	#[codec(index = 3)]
	ForceApprove(BlockNumber),
	/// A signal to revert the block number in the same chain as the
	/// header this digest is part of and all of its descendants.
	///
	/// It is a no-op for a block to contain a revert digest targeting
	/// its own number or a higher number.
	///
	/// In practice, these are issued when on-chain logic has detected an
	/// invalid parachain block within its own chain, due to a dispute.
	#[codec(index = 4)]
	Revert(BlockNumber),
}

impl ConsensusLog {
	/// Attempt to convert a reference to a generic digest item into a consensus log.
	pub fn from_digest_item(
		digest_item: &sp_runtime::DigestItem,
	) -> Result<Option<Self>, codec::Error> {
		match digest_item {
			sp_runtime::DigestItem::Consensus(id, encoded) if id == &POLKADOT_ENGINE_ID =>
				Ok(Some(Self::decode(&mut &encoded[..])?)),
			_ => Ok(None),
		}
	}
}

impl From<ConsensusLog> for sp_runtime::DigestItem {
	fn from(c: ConsensusLog) -> sp_runtime::DigestItem {
		Self::Consensus(POLKADOT_ENGINE_ID, c.encode())
	}
}

/// A statement about a candidate, to be used within the dispute resolution process.
///
/// Statements are either in favor of the candidate's validity or against it.
#[derive(Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum DisputeStatement {
	/// A valid statement, of the given kind.
	#[codec(index = 0)]
	Valid(ValidDisputeStatementKind),
	/// An invalid statement, of the given kind.
	#[codec(index = 1)]
	Invalid(InvalidDisputeStatementKind),
}

impl DisputeStatement {
	/// Get the payload data for this type of dispute statement.
	///
	/// Returns Error if the candidate_hash is not included in the list of signed
	/// candidate from ApprovalCheckingMultipleCandidate.
	pub fn payload_data(
		&self,
		candidate_hash: CandidateHash,
		session: SessionIndex,
	) -> Result<Vec<u8>, ()> {
		match self {
			DisputeStatement::Valid(ValidDisputeStatementKind::Explicit) =>
				Ok(ExplicitDisputeStatement { valid: true, candidate_hash, session }
					.signing_payload()),
			DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(
				inclusion_parent,
			)) => Ok(CompactStatement::Seconded(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: *inclusion_parent,
			})),
			DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent)) =>
				Ok(CompactStatement::Valid(candidate_hash).signing_payload(&SigningContext {
					session_index: session,
					parent_hash: *inclusion_parent,
				})),
			DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking) =>
				Ok(ApprovalVote(candidate_hash).signing_payload(session)),
			DisputeStatement::Valid(
				ValidDisputeStatementKind::ApprovalCheckingMultipleCandidates(candidate_hashes),
			) =>
				if candidate_hashes.contains(&candidate_hash) {
					Ok(ApprovalVoteMultipleCandidates(candidate_hashes).signing_payload(session))
				} else {
					Err(())
				},
			DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit) =>
				Ok(ExplicitDisputeStatement { valid: false, candidate_hash, session }
					.signing_payload()),
		}
	}

	/// Check the signature on a dispute statement.
	pub fn check_signature(
		&self,
		validator_public: &ValidatorId,
		candidate_hash: CandidateHash,
		session: SessionIndex,
		validator_signature: &ValidatorSignature,
	) -> Result<(), ()> {
		let payload = self.payload_data(candidate_hash, session)?;

		if validator_signature.verify(&payload[..], &validator_public) {
			Ok(())
		} else {
			Err(())
		}
	}

	/// Whether the statement indicates validity.
	pub fn indicates_validity(&self) -> bool {
		match *self {
			DisputeStatement::Valid(_) => true,
			DisputeStatement::Invalid(_) => false,
		}
	}

	/// Whether the statement indicates invalidity.
	pub fn indicates_invalidity(&self) -> bool {
		match *self {
			DisputeStatement::Valid(_) => false,
			DisputeStatement::Invalid(_) => true,
		}
	}

	/// Statement is backing statement.
	pub fn is_backing(&self) -> bool {
		match self {
			Self::Valid(s) => s.is_backing(),
			Self::Invalid(_) => false,
		}
	}
}

/// Different kinds of statements of validity on  a candidate.
#[derive(Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum ValidDisputeStatementKind {
	/// An explicit statement issued as part of a dispute.
	#[codec(index = 0)]
	Explicit,
	/// A seconded statement on a candidate from the backing phase.
	#[codec(index = 1)]
	BackingSeconded(Hash),
	/// A valid statement on a candidate from the backing phase.
	#[codec(index = 2)]
	BackingValid(Hash),
	/// An approval vote from the approval checking phase.
	#[codec(index = 3)]
	ApprovalChecking,
	/// An approval vote from the new version.
	/// We can't create this version until all nodes
	/// have been updated to support it and max_approval_coalesce_count
	/// is set to more than 1.
	#[codec(index = 4)]
	ApprovalCheckingMultipleCandidates(Vec<CandidateHash>),
}

impl ValidDisputeStatementKind {
	/// Whether the statement is from the backing phase.
	pub fn is_backing(&self) -> bool {
		match self {
			ValidDisputeStatementKind::BackingSeconded(_) |
			ValidDisputeStatementKind::BackingValid(_) => true,
			ValidDisputeStatementKind::Explicit |
			ValidDisputeStatementKind::ApprovalChecking |
			ValidDisputeStatementKind::ApprovalCheckingMultipleCandidates(_) => false,
		}
	}
}

/// Different kinds of statements of invalidity on a candidate.
#[derive(Encode, Decode, DecodeWithMemTracking, Copy, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum InvalidDisputeStatementKind {
	/// An explicit statement issued as part of a dispute.
	#[codec(index = 0)]
	Explicit,
}

/// An explicit statement on a candidate issued as part of a dispute.
#[derive(Clone, PartialEq, RuntimeDebug)]
pub struct ExplicitDisputeStatement {
	/// Whether the candidate is valid
	pub valid: bool,
	/// The candidate hash.
	pub candidate_hash: CandidateHash,
	/// The session index of the candidate.
	pub session: SessionIndex,
}

impl ExplicitDisputeStatement {
	/// Produce the payload used for signing this type of statement.
	pub fn signing_payload(&self) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"DISP";

		(MAGIC, self.valid, self.candidate_hash, self.session).encode()
	}
}

/// A set of statements about a specific candidate.
#[derive(Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct DisputeStatementSet {
	/// The candidate referenced by this set.
	pub candidate_hash: CandidateHash,
	/// The session index of the candidate.
	pub session: SessionIndex,
	/// Statements about the candidate.
	pub statements: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
}

impl From<CheckedDisputeStatementSet> for DisputeStatementSet {
	fn from(other: CheckedDisputeStatementSet) -> Self {
		other.0
	}
}

impl AsRef<DisputeStatementSet> for DisputeStatementSet {
	fn as_ref(&self) -> &DisputeStatementSet {
		&self
	}
}

/// A set of dispute statements.
pub type MultiDisputeStatementSet = Vec<DisputeStatementSet>;

/// A _checked_ set of dispute statements.
#[derive(Clone, PartialEq, RuntimeDebug, Encode)]
pub struct CheckedDisputeStatementSet(DisputeStatementSet);

impl AsRef<DisputeStatementSet> for CheckedDisputeStatementSet {
	fn as_ref(&self) -> &DisputeStatementSet {
		&self.0
	}
}

impl core::cmp::PartialEq<DisputeStatementSet> for CheckedDisputeStatementSet {
	fn eq(&self, other: &DisputeStatementSet) -> bool {
		self.0.eq(other)
	}
}

impl CheckedDisputeStatementSet {
	/// Convert from an unchecked, the verification of correctness of the `unchecked` statement set
	/// _must_ be done before calling this function!
	pub fn unchecked_from_unchecked(unchecked: DisputeStatementSet) -> Self {
		Self(unchecked)
	}
}

/// A set of _checked_ dispute statements.
pub type CheckedMultiDisputeStatementSet = Vec<CheckedDisputeStatementSet>;

/// The entire state of a dispute.
#[derive(Encode, Decode, Clone, RuntimeDebug, PartialEq, TypeInfo)]
pub struct DisputeState<N = BlockNumber> {
	/// A bitfield indicating all validators for the candidate.
	pub validators_for: BitVec<u8, bitvec::order::Lsb0>, // one bit per validator.
	/// A bitfield indicating all validators against the candidate.
	pub validators_against: BitVec<u8, bitvec::order::Lsb0>, // one bit per validator.
	/// The block number at which the dispute started on-chain.
	pub start: N,
	/// The block number at which the dispute concluded on-chain.
	pub concluded_at: Option<N>,
}

/// Parachains inherent-data passed into the runtime by a block author
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct InherentData<HDR: HeaderT = Header> {
	/// Signed bitfields by validators about availability.
	pub bitfields: UncheckedSignedAvailabilityBitfields,
	/// Backed candidates for inclusion in the block.
	pub backed_candidates: Vec<BackedCandidate<HDR::Hash>>,
	/// Sets of dispute votes for inclusion,
	pub disputes: MultiDisputeStatementSet,
	/// The parent block header. Used for checking state proofs.
	pub parent_header: HDR,
}

/// An either implicit or explicit attestation to the validity of a parachain
/// candidate.
#[derive(Clone, Eq, PartialEq, Decode, DecodeWithMemTracking, Encode, RuntimeDebug, TypeInfo)]
pub enum ValidityAttestation {
	/// Implicit validity attestation by issuing.
	/// This corresponds to issuance of a `Candidate` statement.
	#[codec(index = 1)]
	Implicit(ValidatorSignature),
	/// An explicit attestation. This corresponds to issuance of a
	/// `Valid` statement.
	#[codec(index = 2)]
	Explicit(ValidatorSignature),
}

impl ValidityAttestation {
	/// Produce the underlying signed payload of the attestation, given the hash of the candidate,
	/// which should be known in context.
	pub fn to_compact_statement(&self, candidate_hash: CandidateHash) -> CompactStatement {
		// Explicit and implicit map directly from
		// `ValidityVote::Valid` and `ValidityVote::Issued`, and hence there is a
		// `1:1` relationship which enables the conversion.
		match *self {
			ValidityAttestation::Implicit(_) => CompactStatement::Seconded(candidate_hash),
			ValidityAttestation::Explicit(_) => CompactStatement::Valid(candidate_hash),
		}
	}

	/// Get a reference to the signature.
	pub fn signature(&self) -> &ValidatorSignature {
		match *self {
			ValidityAttestation::Implicit(ref sig) => sig,
			ValidityAttestation::Explicit(ref sig) => sig,
		}
	}

	/// Produce the underlying signed payload of the attestation, given the hash of the candidate,
	/// which should be known in context.
	pub fn signed_payload<H: Encode>(
		&self,
		candidate_hash: CandidateHash,
		signing_context: &SigningContext<H>,
	) -> Vec<u8> {
		match *self {
			ValidityAttestation::Implicit(_) =>
				(CompactStatement::Seconded(candidate_hash), signing_context).encode(),
			ValidityAttestation::Explicit(_) =>
				(CompactStatement::Valid(candidate_hash), signing_context).encode(),
		}
	}
}

/// A type returned by runtime with current session index and a parent hash.
#[derive(Clone, Eq, PartialEq, Default, Decode, Encode, RuntimeDebug)]
pub struct SigningContext<H = Hash> {
	/// Current session index.
	pub session_index: sp_staking::SessionIndex,
	/// Hash of the parent.
	pub parent_hash: H,
}

const BACKING_STATEMENT_MAGIC: [u8; 4] = *b"BKNG";

/// Statements that can be made about parachain candidates. These are the
/// actual values that are signed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
pub enum CompactStatement {
	/// Proposal of a parachain candidate.
	Seconded(CandidateHash),
	/// State that a parachain candidate is valid.
	Valid(CandidateHash),
}

impl CompactStatement {
	/// Yields the payload used for validator signatures on this kind
	/// of statement.
	pub fn signing_payload(&self, context: &SigningContext) -> Vec<u8> {
		(self, context).encode()
	}

	/// Get the underlying candidate hash this references.
	pub fn candidate_hash(&self) -> &CandidateHash {
		match *self {
			CompactStatement::Seconded(ref h) | CompactStatement::Valid(ref h) => h,
		}
	}
}

// Inner helper for codec on `CompactStatement`.
#[derive(Encode, Decode, TypeInfo)]
enum CompactStatementInner {
	#[codec(index = 1)]
	Seconded(CandidateHash),
	#[codec(index = 2)]
	Valid(CandidateHash),
}

impl From<CompactStatement> for CompactStatementInner {
	fn from(s: CompactStatement) -> Self {
		match s {
			CompactStatement::Seconded(h) => CompactStatementInner::Seconded(h),
			CompactStatement::Valid(h) => CompactStatementInner::Valid(h),
		}
	}
}

impl codec::Encode for CompactStatement {
	fn size_hint(&self) -> usize {
		// magic + discriminant + payload
		4 + 1 + 32
	}

	fn encode_to<T: codec::Output + ?Sized>(&self, dest: &mut T) {
		dest.write(&BACKING_STATEMENT_MAGIC);
		CompactStatementInner::from(self.clone()).encode_to(dest)
	}
}

impl codec::Decode for CompactStatement {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
		let maybe_magic = <[u8; 4]>::decode(input)?;
		if maybe_magic != BACKING_STATEMENT_MAGIC {
			return Err(codec::Error::from("invalid magic string"))
		}

		Ok(match CompactStatementInner::decode(input)? {
			CompactStatementInner::Seconded(h) => CompactStatement::Seconded(h),
			CompactStatementInner::Valid(h) => CompactStatement::Valid(h),
		})
	}
}

/// `IndexedVec` struct indexed by type specific indices.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct IndexedVec<K, V>(Vec<V>, PhantomData<fn(K) -> K>);

impl<K, V> Default for IndexedVec<K, V> {
	fn default() -> Self {
		Self(vec![], PhantomData)
	}
}

impl<K, V> From<Vec<V>> for IndexedVec<K, V> {
	fn from(validators: Vec<V>) -> Self {
		Self(validators, PhantomData)
	}
}

impl<K, V> FromIterator<V> for IndexedVec<K, V> {
	fn from_iter<T: IntoIterator<Item = V>>(iter: T) -> Self {
		Self(Vec::from_iter(iter), PhantomData)
	}
}

impl<K, V> IndexedVec<K, V>
where
	V: Clone,
{
	/// Returns a reference to an element indexed using `K`.
	pub fn get(&self, index: K) -> Option<&V>
	where
		K: TypeIndex,
	{
		self.0.get(index.type_index())
	}

	/// Returns a mutable reference to an element indexed using `K`.
	pub fn get_mut(&mut self, index: K) -> Option<&mut V>
	where
		K: TypeIndex,
	{
		self.0.get_mut(index.type_index())
	}

	/// Returns number of elements in vector.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	/// Returns contained vector.
	pub fn to_vec(&self) -> Vec<V> {
		self.0.clone()
	}

	/// Returns an iterator over the underlying vector.
	pub fn iter(&self) -> Iter<'_, V> {
		self.0.iter()
	}

	/// Returns a mutable iterator over the underlying vector.
	pub fn iter_mut(&mut self) -> IterMut<'_, V> {
		self.0.iter_mut()
	}

	/// Creates a consuming iterator.
	pub fn into_iter(self) -> IntoIter<V> {
		self.0.into_iter()
	}

	/// Returns true if the underlying container is empty.
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}
}

/// The maximum number of validators `f` which may safely be faulty.
///
/// The total number of validators is `n = 3f + e` where `e in { 1, 2, 3 }`.
pub const fn byzantine_threshold(n: usize) -> usize {
	n.saturating_sub(1) / 3
}

/// The supermajority threshold of validators which represents a subset
/// guaranteed to have at least f+1 honest validators.
pub const fn supermajority_threshold(n: usize) -> usize {
	n - byzantine_threshold(n)
}

/// Adjust the configured needed backing votes with the size of the backing group.
pub fn effective_minimum_backing_votes(
	group_len: usize,
	configured_minimum_backing_votes: u32,
) -> usize {
	core::cmp::min(group_len, configured_minimum_backing_votes as usize)
}

/// Information about validator sets of a session.
///
/// NOTE: `SessionInfo` is frozen. Do not include new fields, consider creating a separate runtime
/// API. Reasoning and further outlook [here](https://github.com/paritytech/polkadot/issues/6586).
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct SessionInfo {
	/****** New in v2 ****** */
	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	pub active_validator_indices: Vec<ValidatorIndex>,
	/// A secure random seed for the session, gathered from BABE.
	pub random_seed: [u8; 32],
	/// The amount of sessions to keep for disputes.
	pub dispute_period: SessionIndex,

	/****** Old fields ***** */
	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators`
	/// participating in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to `max_validators` when set.
	pub validators: IndexedVec<ValidatorIndex, ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	///
	/// NOTE: The first `validators.len()` entries will match the corresponding validators in
	/// `validators`, afterwards any remaining authorities can be found. This is any authorities
	/// not participating in parachain consensus - see
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148)
	pub discovery_keys: Vec<AuthorityDiscoveryId>,
	/// The assignment keys for validators.
	///
	/// NOTE: There might be more authorities in the current session, than validators participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// Therefore:
	/// ```ignore
	/// 	assignment_keys.len() == validators.len() && validators.len() <= discovery_keys.len()
	/// ```
	pub assignment_keys: Vec<AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	pub n_cores: u32,
	/// The zeroth delay tranche width.
	pub zeroth_delay_tranche_width: u32,
	/// The number of samples we do of `relay_vrf_modulo`.
	pub relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
	/// no-show.
	pub no_show_slots: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
}

/// A statement from the specified validator whether the given validation code passes PVF
/// pre-checking or not anchored to the given session index.
#[derive(Encode, Decode, DecodeWithMemTracking, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct PvfCheckStatement {
	/// `true` if the subject passed pre-checking and `false` otherwise.
	pub accept: bool,
	/// The validation code hash that was checked.
	pub subject: ValidationCodeHash,
	/// The index of a session during which this statement is considered valid.
	pub session_index: SessionIndex,
	/// The index of the validator from which this statement originates.
	pub validator_index: ValidatorIndex,
}

impl PvfCheckStatement {
	/// Produce the payload used for signing this type of statement.
	///
	/// It is expected that it will be signed by the validator at `validator_index` in the
	/// `session_index`.
	pub fn signing_payload(&self) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"VCPC"; // for "validation code pre-checking"
		(MAGIC, self.accept, self.subject, self.session_index, self.validator_index).encode()
	}
}

/// A well-known and typed storage key.
///
/// Allows for type-safe access to raw well-known storage keys.
pub struct WellKnownKey<T> {
	/// The raw storage key.
	pub key: Vec<u8>,
	_p: core::marker::PhantomData<T>,
}

impl<T> From<Vec<u8>> for WellKnownKey<T> {
	fn from(key: Vec<u8>) -> Self {
		Self { key, _p: Default::default() }
	}
}

impl<T> AsRef<[u8]> for WellKnownKey<T> {
	fn as_ref(&self) -> &[u8] {
		self.key.as_ref()
	}
}

impl<T: Decode> WellKnownKey<T> {
	/// Gets the value or `None` if it does not exist or decoding failed.
	pub fn get(&self) -> Option<T> {
		sp_io::storage::get(&self.key)
			.and_then(|raw| codec::DecodeAll::decode_all(&mut raw.as_ref()).ok())
	}
}

impl<T: Encode> WellKnownKey<T> {
	/// Sets the value.
	pub fn set(&self, value: T) {
		sp_io::storage::set(&self.key, &value.encode());
	}
}

/// Type discriminator for PVF preparation.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	TypeInfo,
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
)]
pub enum PvfPrepKind {
	/// For prechecking requests.
	Precheck,

	/// For execution and heads-up requests.
	Prepare,
}

/// Type discriminator for PVF execution.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	TypeInfo,
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
)]
pub enum PvfExecKind {
	/// For backing requests.
	Backing,
	/// For approval and dispute request.
	Approval,
}

/// Bit indices in the `HostConfiguration.node_features` that correspond to different node features.
pub type NodeFeatures = BitVec<u8, bitvec::order::Lsb0>;

/// Module containing feature-specific bit indices into the `NodeFeatures` bitvec.
pub mod node_features {
	/// A feature index used to identify a bit into the node_features array stored
	/// in the HostConfiguration.
	#[repr(u8)]
	#[derive(Clone, Copy)]
	pub enum FeatureIndex {
		/// Tells if tranch0 assignments could be sent in a single certificate.
		/// Reserved for: `<https://github.com/paritytech/polkadot-sdk/issues/628>`
		EnableAssignmentsV2 = 0,
		/// This feature enables the extension of `BackedCandidate::validator_indices` by 8 bits.
		/// The value stored there represents the assumed core index where the candidates
		/// are backed. This is needed for the elastic scaling MVP.
		ElasticScalingMVP = 1,
		/// Tells if the chunk mapping feature is enabled.
		/// Enables the implementation of
		/// [RFC-47](https://github.com/polkadot-fellows/RFCs/blob/main/text/0047-assignment-of-availability-chunks.md).
		/// Must not be enabled unless all validators and collators have stopped using `req_chunk`
		/// protocol version 1. If it is enabled, validators can start systematic chunk recovery.
		AvailabilityChunkMapping = 2,
		/// Enables node side support of `CoreIndex` committed candidate receipts.
		/// See [RFC-103](https://github.com/polkadot-fellows/RFCs/pull/103) for details.
		/// Only enable if at least 2/3 of nodes support the feature.
		CandidateReceiptV2 = 3,
		/// First unassigned feature bit.
		/// Every time a new feature flag is assigned it should take this value.
		/// and this should be incremented.
		FirstUnassigned = 4,
	}
}

/// Scheduler configuration parameters. All coretime/ondemand parameters are here.
#[derive(
	RuntimeDebug,
	Copy,
	Clone,
	PartialEq,
	Encode,
	Decode,
	DecodeWithMemTracking,
	TypeInfo,
	serde::Serialize,
	serde::Deserialize,
)]
pub struct SchedulerParams<BlockNumber> {
	/// How often parachain groups should be rotated across parachains.
	///
	/// Must be non-zero.
	pub group_rotation_frequency: BlockNumber,
	/// Availability timeout for a block on a core, measured in blocks.
	///
	/// This is the maximum amount of blocks after a core became occupied that validators have time
	/// to make the block available.
	///
	/// This value only has effect on group rotations. If backers backed something at the end of
	/// their rotation, the occupied core affects the backing group that comes afterwards. We limit
	/// the effect one backing group can have on the next to `paras_availability_period` blocks.
	///
	/// Within a group rotation there is no timeout as backers are only affecting themselves.
	///
	/// Must be at least 1. With a value of 1, the previous group will not be able to negatively
	/// affect the following group at the expense of a tight availability timeline at group
	/// rotation boundaries.
	pub paras_availability_period: BlockNumber,
	/// The maximum number of validators to have per core.
	///
	/// `None` means no maximum.
	pub max_validators_per_core: Option<u32>,
	/// The amount of blocks ahead to schedule paras.
	pub lookahead: u32,
	/// How many cores are managed by the coretime chain.
	pub num_cores: u32,
	/// Deprecated and no longer used by the runtime.
	/// Removal is tracked by <https://github.com/paritytech/polkadot-sdk/issues/6067>.
	#[deprecated]
	pub max_availability_timeouts: u32,
	/// The maximum queue size of the pay as you go module.
	pub on_demand_queue_max_size: u32,
	/// The target utilization of the spot price queue in percentages.
	pub on_demand_target_queue_utilization: Perbill,
	/// How quickly the fee rises in reaction to increased utilization.
	/// The lower the number the slower the increase.
	pub on_demand_fee_variability: Perbill,
	/// The minimum amount needed to claim a slot in the spot pricing queue.
	pub on_demand_base_fee: Balance,
	/// Deprecated and no longer used by the runtime.
	/// Removal is tracked by <https://github.com/paritytech/polkadot-sdk/issues/6067>.
	#[deprecated]
	pub ttl: BlockNumber,
}

impl<BlockNumber: Default + From<u32>> Default for SchedulerParams<BlockNumber> {
	#[allow(deprecated)]
	fn default() -> Self {
		Self {
			group_rotation_frequency: 1u32.into(),
			paras_availability_period: 1u32.into(),
			max_validators_per_core: Default::default(),
			lookahead: 1,
			num_cores: Default::default(),
			max_availability_timeouts: Default::default(),
			on_demand_queue_max_size: ON_DEMAND_DEFAULT_QUEUE_MAX_SIZE,
			on_demand_target_queue_utilization: Perbill::from_percent(25),
			on_demand_fee_variability: Perbill::from_percent(3),
			on_demand_base_fee: 10_000_000u128,
			ttl: 5u32.into(),
		}
	}
}

#[cfg(test)]
/// Test helpers
pub mod tests {
	use super::*;
	use bitvec::bitvec;
	use sp_core::sr25519;

	/// Create a dummy committed candidate receipt
	pub fn dummy_committed_candidate_receipt() -> CommittedCandidateReceipt {
		let zeros = Hash::zero();

		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: 0.into(),
				relay_parent: zeros,
				collator: CollatorId::from(sr25519::Public::default()),
				persisted_validation_data_hash: zeros,
				pov_hash: zeros,
				erasure_root: zeros,
				signature: CollatorSignature::from(sr25519::Signature::default()),
				para_head: zeros,
				validation_code_hash: ValidationCode(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]).hash(),
			},
			commitments: CandidateCommitments {
				head_data: HeadData(vec![]),
				upward_messages: vec![].try_into().expect("empty vec fits within bounds"),
				new_validation_code: None,
				horizontal_messages: vec![].try_into().expect("empty vec fits within bounds"),
				processed_downward_messages: 0,
				hrmp_watermark: 0_u32,
			},
		}
	}

	#[test]
	fn group_rotation_info_calculations() {
		let info =
			GroupRotationInfo { session_start_block: 10u32, now: 15, group_rotation_frequency: 5 };

		assert_eq!(info.next_rotation_at(), 20);
		assert_eq!(info.last_rotation_at(), 15);
	}

	#[test]
	fn group_for_core_is_core_for_group() {
		for cores in 1..=256 {
			for rotations in 0..(cores * 2) {
				let info = GroupRotationInfo {
					session_start_block: 0u32,
					now: rotations,
					group_rotation_frequency: 1,
				};

				for core in 0..cores {
					let group = info.group_for_core(CoreIndex(core), cores as usize);
					assert_eq!(info.core_for_group(group, cores as usize).0, core);
				}
			}
		}
	}

	#[test]
	fn collator_signature_payload_is_valid() {
		// if this fails, collator signature verification code has to be updated.
		let h = Hash::default();
		assert_eq!(h.as_ref().len(), 32);

		let _payload = collator_signature_payload(
			&Hash::repeat_byte(1),
			&5u32.into(),
			&Hash::repeat_byte(2),
			&Hash::repeat_byte(3),
			&Hash::repeat_byte(4).into(),
		);
	}

	#[test]
	fn test_byzantine_threshold() {
		assert_eq!(byzantine_threshold(0), 0);
		assert_eq!(byzantine_threshold(1), 0);
		assert_eq!(byzantine_threshold(2), 0);
		assert_eq!(byzantine_threshold(3), 0);
		assert_eq!(byzantine_threshold(4), 1);
		assert_eq!(byzantine_threshold(5), 1);
		assert_eq!(byzantine_threshold(6), 1);
		assert_eq!(byzantine_threshold(7), 2);
	}

	#[test]
	fn test_supermajority_threshold() {
		assert_eq!(supermajority_threshold(0), 0);
		assert_eq!(supermajority_threshold(1), 1);
		assert_eq!(supermajority_threshold(2), 2);
		assert_eq!(supermajority_threshold(3), 3);
		assert_eq!(supermajority_threshold(4), 3);
		assert_eq!(supermajority_threshold(5), 4);
		assert_eq!(supermajority_threshold(6), 5);
		assert_eq!(supermajority_threshold(7), 5);
	}

	#[test]
	fn balance_bigger_than_usize() {
		let zero_b: Balance = 0;
		let zero_u: usize = 0;

		assert!(zero_b.leading_zeros() >= zero_u.leading_zeros());
	}

	#[test]
	fn test_backed_candidate_injected_core_index() {
		let initial_validator_indices = bitvec![u8, bitvec::order::Lsb0; 0, 1, 0, 1];
		let mut candidate = BackedCandidate::new(
			dummy_committed_candidate_receipt(),
			vec![],
			initial_validator_indices.clone(),
			CoreIndex(10),
		);

		// No core index supplied.
		candidate
			.set_validator_indices_and_core_index(initial_validator_indices.clone().into(), None);
		let (validator_indices, core_index) = candidate.validator_indices_and_core_index();
		assert_eq!(validator_indices, initial_validator_indices.as_bitslice());
		assert!(core_index.is_none());

		// No core index supplied. Decoding is corrupted if backing group
		// size larger than 8.
		candidate.set_validator_indices_and_core_index(
			bitvec![u8, bitvec::order::Lsb0; 0, 1, 0, 1, 0, 1, 0, 1, 0].into(),
			None,
		);

		let (validator_indices, core_index) = candidate.validator_indices_and_core_index();
		assert_eq!(validator_indices, bitvec![u8, bitvec::order::Lsb0; 0].as_bitslice());
		assert!(core_index.is_some());

		// Core index supplied.
		let mut candidate = BackedCandidate::new(
			dummy_committed_candidate_receipt(),
			vec![],
			bitvec![u8, bitvec::order::Lsb0; 0, 1, 0, 1],
			CoreIndex(10),
		);
		let (validator_indices, core_index) = candidate.validator_indices_and_core_index();
		assert_eq!(validator_indices, bitvec![u8, bitvec::order::Lsb0; 0, 1, 0, 1]);
		assert_eq!(core_index, Some(CoreIndex(10)));

		let encoded_validator_indices = candidate.validator_indices.clone();
		candidate.set_validator_indices_and_core_index(validator_indices.into(), core_index);
		assert_eq!(candidate.validator_indices, encoded_validator_indices);
	}
}

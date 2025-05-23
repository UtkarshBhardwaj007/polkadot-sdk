// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! GRANDPA Consensus module for runtime.
//!
//! This manages the GRANDPA authority set ready for the native code.
//! These authorities are only for GRANDPA finality, not for consensus overall.
//!
//! In the future, it will also handle misbehavior reports, and on-chain
//! finality notifications.
//!
//! For full integration with GRANDPA, the `GrandpaApi` should be implemented.
//! The necessary items are re-exported via the `fg_primitives` crate.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Re-export since this is necessary for `impl_apis` in runtime.
pub use sp_consensus_grandpa::{
	self as fg_primitives, AuthorityId, AuthorityList, AuthorityWeight,
};

use alloc::{boxed::Box, vec::Vec};
use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::{
	dispatch::{DispatchResultWithPostInfo, Pays},
	pallet_prelude::Get,
	traits::OneSessionHandler,
	weights::Weight,
	WeakBoundedVec,
};
use frame_system::pallet_prelude::BlockNumberFor;
use scale_info::TypeInfo;
use sp_consensus_grandpa::{
	ConsensusLog, EquivocationProof, ScheduledChange, SetId, GRANDPA_ENGINE_ID,
	RUNTIME_LOG_TARGET as LOG_TARGET,
};
use sp_runtime::{generic::DigestItem, traits::Zero, DispatchResult};
use sp_session::{GetSessionNumber, GetValidatorCount};
use sp_staking::{offence::OffenceReportSystem, SessionIndex};

mod default_weights;
mod equivocation;
pub mod migrations;

#[cfg(any(feature = "runtime-benchmarks", test))]
mod benchmarking;
#[cfg(all(feature = "std", test))]
mod mock;
#[cfg(all(feature = "std", test))]
mod tests;

pub use equivocation::{EquivocationOffence, EquivocationReportSystem, TimeSlot};

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{dispatch::DispatchResult, pallet_prelude::*};
	use frame_system::pallet_prelude::*;

	/// The in-code storage version.
	const STORAGE_VERSION: StorageVersion = StorageVersion::new(5);

	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The event type of this module.
		#[allow(deprecated)]
		type RuntimeEvent: From<Event>
			+ Into<<Self as frame_system::Config>::RuntimeEvent>
			+ IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Weights for this pallet.
		type WeightInfo: WeightInfo;

		/// Max Authorities in use
		#[pallet::constant]
		type MaxAuthorities: Get<u32>;

		/// The maximum number of nominators for each validator.
		#[pallet::constant]
		type MaxNominators: Get<u32>;

		/// The maximum number of entries to keep in the set id to session index mapping.
		///
		/// Since the `SetIdSession` map is only used for validating equivocations this
		/// value should relate to the bonding duration of whatever staking system is
		/// being used (if any). If equivocation handling is not enabled then this value
		/// can be zero.
		#[pallet::constant]
		type MaxSetIdSessionEntries: Get<u64>;

		/// The proof of key ownership, used for validating equivocation reports
		/// The proof include the session index and validator count of the
		/// session at which the equivocation occurred.
		type KeyOwnerProof: Parameter + GetSessionNumber + GetValidatorCount;

		/// The equivocation handling subsystem, defines methods to check/report an
		/// offence and for submitting a transaction to report an equivocation
		/// (from an offchain context).
		type EquivocationReportSystem: OffenceReportSystem<
			Option<Self::AccountId>,
			(EquivocationProof<Self::Hash, BlockNumberFor<Self>>, Self::KeyOwnerProof),
		>;
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_finalize(block_number: BlockNumberFor<T>) {
			// check for scheduled pending authority set changes
			if let Some(pending_change) = PendingChange::<T>::get() {
				// emit signal if we're at the block that scheduled the change
				if block_number == pending_change.scheduled_at {
					let next_authorities = pending_change.next_authorities.to_vec();
					if let Some(median) = pending_change.forced {
						Self::deposit_log(ConsensusLog::ForcedChange(
							median,
							ScheduledChange { delay: pending_change.delay, next_authorities },
						))
					} else {
						Self::deposit_log(ConsensusLog::ScheduledChange(ScheduledChange {
							delay: pending_change.delay,
							next_authorities,
						}));
					}
				}

				// enact the change if we've reached the enacting block
				if block_number == pending_change.scheduled_at + pending_change.delay {
					Authorities::<T>::put(&pending_change.next_authorities);
					Self::deposit_event(Event::NewAuthorities {
						authority_set: pending_change.next_authorities.into_inner(),
					});
					PendingChange::<T>::kill();
				}
			}

			// check for scheduled pending state changes
			match State::<T>::get() {
				StoredState::PendingPause { scheduled_at, delay } => {
					// signal change to pause
					if block_number == scheduled_at {
						Self::deposit_log(ConsensusLog::Pause(delay));
					}

					// enact change to paused state
					if block_number == scheduled_at + delay {
						State::<T>::put(StoredState::Paused);
						Self::deposit_event(Event::Paused);
					}
				},
				StoredState::PendingResume { scheduled_at, delay } => {
					// signal change to resume
					if block_number == scheduled_at {
						Self::deposit_log(ConsensusLog::Resume(delay));
					}

					// enact change to live state
					if block_number == scheduled_at + delay {
						State::<T>::put(StoredState::Live);
						Self::deposit_event(Event::Resumed);
					}
				},
				_ => {},
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Report voter equivocation/misbehavior. This method will verify the
		/// equivocation proof and validate the given key ownership proof
		/// against the extracted offender. If both are valid, the offence
		/// will be reported.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::report_equivocation(
			key_owner_proof.validator_count(),
			T::MaxNominators::get(),
		))]
		pub fn report_equivocation(
			origin: OriginFor<T>,
			equivocation_proof: Box<EquivocationProof<T::Hash, BlockNumberFor<T>>>,
			key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			let reporter = ensure_signed(origin)?;

			T::EquivocationReportSystem::process_evidence(
				Some(reporter),
				(*equivocation_proof, key_owner_proof),
			)?;
			// Waive the fee since the report is valid and beneficial
			Ok(Pays::No.into())
		}

		/// Report voter equivocation/misbehavior. This method will verify the
		/// equivocation proof and validate the given key ownership proof
		/// against the extracted offender. If both are valid, the offence
		/// will be reported.
		///
		/// This extrinsic must be called unsigned and it is expected that only
		/// block authors will call it (validated in `ValidateUnsigned`), as such
		/// if the block author is defined it will be defined as the equivocation
		/// reporter.
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::report_equivocation(
			key_owner_proof.validator_count(),
			T::MaxNominators::get(),
		))]
		pub fn report_equivocation_unsigned(
			origin: OriginFor<T>,
			equivocation_proof: Box<EquivocationProof<T::Hash, BlockNumberFor<T>>>,
			key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;

			T::EquivocationReportSystem::process_evidence(
				None,
				(*equivocation_proof, key_owner_proof),
			)?;
			Ok(Pays::No.into())
		}

		/// Note that the current authority set of the GRANDPA finality gadget has stalled.
		///
		/// This will trigger a forced authority set change at the beginning of the next session, to
		/// be enacted `delay` blocks after that. The `delay` should be high enough to safely assume
		/// that the block signalling the forced change will not be re-orged e.g. 1000 blocks.
		/// The block production rate (which may be slowed down because of finality lagging) should
		/// be taken into account when choosing the `delay`. The GRANDPA voters based on the new
		/// authority will start voting on top of `best_finalized_block_number` for new finalized
		/// blocks. `best_finalized_block_number` should be the highest of the latest finalized
		/// block of all validators of the new authority set.
		///
		/// Only callable by root.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::note_stalled())]
		pub fn note_stalled(
			origin: OriginFor<T>,
			delay: BlockNumberFor<T>,
			best_finalized_block_number: BlockNumberFor<T>,
		) -> DispatchResult {
			ensure_root(origin)?;

			Self::on_stalled(delay, best_finalized_block_number);
			Ok(())
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(fn deposit_event)]
	pub enum Event {
		/// New authority set has been applied.
		NewAuthorities { authority_set: AuthorityList },
		/// Current authority set has been paused.
		Paused,
		/// Current authority set has been resumed.
		Resumed,
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Attempt to signal GRANDPA pause when the authority set isn't live
		/// (either paused or already pending pause).
		PauseFailed,
		/// Attempt to signal GRANDPA resume when the authority set isn't paused
		/// (either live or already pending resume).
		ResumeFailed,
		/// Attempt to signal GRANDPA change with one already pending.
		ChangePending,
		/// Cannot signal forced change so soon after last.
		TooSoon,
		/// A key ownership proof provided as part of an equivocation report is invalid.
		InvalidKeyOwnershipProof,
		/// An equivocation proof provided as part of an equivocation report is invalid.
		InvalidEquivocationProof,
		/// A given equivocation report is valid but already previously reported.
		DuplicateOffenceReport,
	}

	#[pallet::type_value]
	pub fn DefaultForState<T: Config>() -> StoredState<BlockNumberFor<T>> {
		StoredState::Live
	}

	/// State of the current authority set.
	#[pallet::storage]
	pub type State<T: Config> =
		StorageValue<_, StoredState<BlockNumberFor<T>>, ValueQuery, DefaultForState<T>>;

	/// Pending change: (signaled at, scheduled change).
	#[pallet::storage]
	pub type PendingChange<T: Config> =
		StorageValue<_, StoredPendingChange<BlockNumberFor<T>, T::MaxAuthorities>>;

	/// next block number where we can force a change.
	#[pallet::storage]
	pub type NextForced<T: Config> = StorageValue<_, BlockNumberFor<T>>;

	/// `true` if we are currently stalled.
	#[pallet::storage]
	pub type Stalled<T: Config> = StorageValue<_, (BlockNumberFor<T>, BlockNumberFor<T>)>;

	/// The number of changes (both in terms of keys and underlying economic responsibilities)
	/// in the "set" of Grandpa validators from genesis.
	#[pallet::storage]
	pub type CurrentSetId<T: Config> = StorageValue<_, SetId, ValueQuery>;

	/// A mapping from grandpa set ID to the index of the *most recent* session for which its
	/// members were responsible.
	///
	/// This is only used for validating equivocation proofs. An equivocation proof must
	/// contains a key-ownership proof for a given session, therefore we need a way to tie
	/// together sessions and GRANDPA set ids, i.e. we need to validate that a validator
	/// was the owner of a given key on a given session, and what the active set ID was
	/// during that session.
	///
	/// TWOX-NOTE: `SetId` is not under user control.
	#[pallet::storage]
	pub type SetIdSession<T: Config> = StorageMap<_, Twox64Concat, SetId, SessionIndex>;

	/// The current list of authorities.
	#[pallet::storage]
	pub type Authorities<T: Config> =
		StorageValue<_, BoundedAuthorityList<T::MaxAuthorities>, ValueQuery>;

	#[derive(frame_support::DefaultNoBound)]
	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub authorities: AuthorityList,
		#[serde(skip)]
		pub _config: core::marker::PhantomData<T>,
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			CurrentSetId::<T>::put(SetId::default());
			Pallet::<T>::initialize(self.authorities.clone())
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		fn validate_unsigned(source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			Self::validate_unsigned(source, call)
		}

		fn pre_dispatch(call: &Self::Call) -> Result<(), TransactionValidityError> {
			Self::pre_dispatch(call)
		}
	}
}

pub trait WeightInfo {
	fn report_equivocation(validator_count: u32, max_nominators_per_validator: u32) -> Weight;
	fn note_stalled() -> Weight;
}

/// Bounded version of `AuthorityList`, `Limit` being the bound
pub type BoundedAuthorityList<Limit> = WeakBoundedVec<(AuthorityId, AuthorityWeight), Limit>;

/// A stored pending change.
/// `Limit` is the bound for `next_authorities`
#[derive(Encode, Decode, TypeInfo, MaxEncodedLen)]
#[codec(mel_bound(N: MaxEncodedLen, Limit: Get<u32>))]
#[scale_info(skip_type_params(Limit))]
pub struct StoredPendingChange<N, Limit> {
	/// The block number this was scheduled at.
	pub scheduled_at: N,
	/// The delay in blocks until it will be applied.
	pub delay: N,
	/// The next authority set, weakly bounded in size by `Limit`.
	pub next_authorities: BoundedAuthorityList<Limit>,
	/// If defined it means the change was forced and the given block number
	/// indicates the median last finalized block when the change was signaled.
	pub forced: Option<N>,
}

/// Current state of the GRANDPA authority set. State transitions must happen in
/// the same order of states defined below, e.g. `Paused` implies a prior
/// `PendingPause`.
#[derive(Decode, Encode, TypeInfo, MaxEncodedLen)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum StoredState<N> {
	/// The current authority set is live, and GRANDPA is enabled.
	Live,
	/// There is a pending pause event which will be enacted at the given block
	/// height.
	PendingPause {
		/// Block at which the intention to pause was scheduled.
		scheduled_at: N,
		/// Number of blocks after which the change will be enacted.
		delay: N,
	},
	/// The current GRANDPA authority set is paused.
	Paused,
	/// There is a pending resume event which will be enacted at the given block
	/// height.
	PendingResume {
		/// Block at which the intention to resume was scheduled.
		scheduled_at: N,
		/// Number of blocks after which the change will be enacted.
		delay: N,
	},
}

impl<T: Config> Pallet<T> {
	/// State of the current authority set.
	pub fn state() -> StoredState<BlockNumberFor<T>> {
		State::<T>::get()
	}

	/// Pending change: (signaled at, scheduled change).
	pub fn pending_change() -> Option<StoredPendingChange<BlockNumberFor<T>, T::MaxAuthorities>> {
		PendingChange::<T>::get()
	}

	/// next block number where we can force a change.
	pub fn next_forced() -> Option<BlockNumberFor<T>> {
		NextForced::<T>::get()
	}

	/// `true` if we are currently stalled.
	pub fn stalled() -> Option<(BlockNumberFor<T>, BlockNumberFor<T>)> {
		Stalled::<T>::get()
	}

	/// The number of changes (both in terms of keys and underlying economic responsibilities)
	/// in the "set" of Grandpa validators from genesis.
	pub fn current_set_id() -> SetId {
		CurrentSetId::<T>::get()
	}

	/// A mapping from grandpa set ID to the index of the *most recent* session for which its
	/// members were responsible.
	///
	/// This is only used for validating equivocation proofs. An equivocation proof must
	/// contains a key-ownership proof for a given session, therefore we need a way to tie
	/// together sessions and GRANDPA set ids, i.e. we need to validate that a validator
	/// was the owner of a given key on a given session, and what the active set ID was
	/// during that session.
	pub fn session_for_set(set_id: SetId) -> Option<SessionIndex> {
		SetIdSession::<T>::get(set_id)
	}

	/// Get the current set of authorities, along with their respective weights.
	pub fn grandpa_authorities() -> AuthorityList {
		Authorities::<T>::get().into_inner()
	}

	/// Schedule GRANDPA to pause starting in the given number of blocks.
	/// Cannot be done when already paused.
	pub fn schedule_pause(in_blocks: BlockNumberFor<T>) -> DispatchResult {
		if let StoredState::Live = State::<T>::get() {
			let scheduled_at = frame_system::Pallet::<T>::block_number();
			State::<T>::put(StoredState::PendingPause { delay: in_blocks, scheduled_at });

			Ok(())
		} else {
			Err(Error::<T>::PauseFailed.into())
		}
	}

	/// Schedule a resume of GRANDPA after pausing.
	pub fn schedule_resume(in_blocks: BlockNumberFor<T>) -> DispatchResult {
		if let StoredState::Paused = State::<T>::get() {
			let scheduled_at = frame_system::Pallet::<T>::block_number();
			State::<T>::put(StoredState::PendingResume { delay: in_blocks, scheduled_at });

			Ok(())
		} else {
			Err(Error::<T>::ResumeFailed.into())
		}
	}

	/// Schedule a change in the authorities.
	///
	/// The change will be applied at the end of execution of the block
	/// `in_blocks` after the current block. This value may be 0, in which
	/// case the change is applied at the end of the current block.
	///
	/// If the `forced` parameter is defined, this indicates that the current
	/// set has been synchronously determined to be offline and that after
	/// `in_blocks` the given change should be applied. The given block number
	/// indicates the median last finalized block number and it should be used
	/// as the canon block when starting the new grandpa voter.
	///
	/// No change should be signaled while any change is pending. Returns
	/// an error if a change is already pending.
	pub fn schedule_change(
		next_authorities: AuthorityList,
		in_blocks: BlockNumberFor<T>,
		forced: Option<BlockNumberFor<T>>,
	) -> DispatchResult {
		if !PendingChange::<T>::exists() {
			let scheduled_at = frame_system::Pallet::<T>::block_number();

			if forced.is_some() {
				if NextForced::<T>::get().map_or(false, |next| next > scheduled_at) {
					return Err(Error::<T>::TooSoon.into())
				}

				// only allow the next forced change when twice the window has passed since
				// this one.
				NextForced::<T>::put(scheduled_at + in_blocks * 2u32.into());
			}

			let next_authorities = WeakBoundedVec::<_, T::MaxAuthorities>::force_from(
				next_authorities,
				Some(
					"Warning: The number of authorities given is too big. \
					A runtime configuration adjustment may be needed.",
				),
			);

			PendingChange::<T>::put(StoredPendingChange {
				delay: in_blocks,
				scheduled_at,
				next_authorities,
				forced,
			});

			Ok(())
		} else {
			Err(Error::<T>::ChangePending.into())
		}
	}

	/// Deposit one of this module's logs.
	fn deposit_log(log: ConsensusLog<BlockNumberFor<T>>) {
		let log = DigestItem::Consensus(GRANDPA_ENGINE_ID, log.encode());
		frame_system::Pallet::<T>::deposit_log(log);
	}

	// Perform module initialization, abstracted so that it can be called either through genesis
	// config builder or through `on_genesis_session`.
	fn initialize(authorities: AuthorityList) {
		if !authorities.is_empty() {
			assert!(Self::grandpa_authorities().is_empty(), "Authorities are already initialized!");
			Authorities::<T>::put(
				&BoundedAuthorityList::<T::MaxAuthorities>::try_from(authorities).expect(
					"Grandpa: `Config::MaxAuthorities` is smaller than the number of genesis authorities!",
				),
			);
		}

		// NOTE: initialize first session of first set. this is necessary for
		// the genesis set and session since we only update the set -> session
		// mapping whenever a new session starts, i.e. through `on_new_session`.
		SetIdSession::<T>::insert(0, 0);
	}

	/// Submits an extrinsic to report an equivocation. This method will create
	/// an unsigned extrinsic with a call to `report_equivocation_unsigned` and
	/// will push the transaction to the pool. Only useful in an offchain
	/// context.
	pub fn submit_unsigned_equivocation_report(
		equivocation_proof: EquivocationProof<T::Hash, BlockNumberFor<T>>,
		key_owner_proof: T::KeyOwnerProof,
	) -> Option<()> {
		T::EquivocationReportSystem::publish_evidence((equivocation_proof, key_owner_proof)).ok()
	}

	fn on_stalled(further_wait: BlockNumberFor<T>, median: BlockNumberFor<T>) {
		// when we record old authority sets we could try to figure out _who_
		// failed. until then, we can't meaningfully guard against
		// `next == last` the way that normal session changes do.
		Stalled::<T>::put((further_wait, median));
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
	type Public = AuthorityId;
}

impl<T: Config> OneSessionHandler<T::AccountId> for Pallet<T>
where
	T: pallet_session::Config,
{
	type Key = AuthorityId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AuthorityId)>,
	{
		let authorities = validators.map(|(_, k)| (k, 1)).collect::<Vec<_>>();
		Self::initialize(authorities);
	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, _queued_validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AuthorityId)>,
	{
		// Always issue a change if `session` says that the validators have changed.
		// Even if their session keys are the same as before, the underlying economic
		// identities have changed.
		let current_set_id = if changed || Stalled::<T>::exists() {
			let next_authorities = validators.map(|(_, k)| (k, 1)).collect::<Vec<_>>();

			let res = if let Some((further_wait, median)) = Stalled::<T>::take() {
				Self::schedule_change(next_authorities, further_wait, Some(median))
			} else {
				Self::schedule_change(next_authorities, Zero::zero(), None)
			};

			if res.is_ok() {
				let current_set_id = CurrentSetId::<T>::mutate(|s| {
					*s += 1;
					*s
				});

				let max_set_id_session_entries = T::MaxSetIdSessionEntries::get().max(1);
				if current_set_id >= max_set_id_session_entries {
					SetIdSession::<T>::remove(current_set_id - max_set_id_session_entries);
				}

				current_set_id
			} else {
				// either the session module signalled that the validators have changed
				// or the set was stalled. but since we didn't successfully schedule
				// an authority set change we do not increment the set id.
				CurrentSetId::<T>::get()
			}
		} else {
			// nothing's changed, neither economic conditions nor session keys. update the pointer
			// of the current set.
			CurrentSetId::<T>::get()
		};

		// update the mapping to note that the current set corresponds to the
		// latest equivalent session (i.e. now).
		let session_index = pallet_session::Pallet::<T>::current_index();
		SetIdSession::<T>::insert(current_set_id, &session_index);
	}

	fn on_disabled(i: u32) {
		Self::deposit_log(ConsensusLog::OnDisabled(i as u64))
	}
}

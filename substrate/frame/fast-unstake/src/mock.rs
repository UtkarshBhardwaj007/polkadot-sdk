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

use crate::{self as fast_unstake};
use frame_election_provider_support::PageIndex;
use frame_support::{
	assert_ok, derive_impl,
	pallet_prelude::*,
	parameter_types,
	traits::{ConstU64, Currency},
	weights::constants::WEIGHT_REF_TIME_PER_SECOND,
};
use sp_runtime::{traits::IdentityLookup, BuildStorage};

use pallet_staking::{Exposure, IndividualExposure, StakerStatus};

pub type AccountId = u128;
pub type BlockNumber = u64;
pub type Balance = u128;
pub type T = Runtime;

parameter_types! {
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(
			Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX),
		);
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Runtime {
	type Block = Block;
	type AccountData = pallet_balances::AccountData<Balance>;
	// we use U128 account id in order to get a better iteration order out of a map.
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
}

impl pallet_timestamp::Config for Runtime {
	type Moment = u64;
	type OnTimestampSet = ();
	type MinimumPeriod = ConstU64<5>;
	type WeightInfo = ();
}

parameter_types! {
	pub static ExistentialDeposit: Balance = 1;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Runtime {
	type Balance = Balance;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
}

pallet_staking_reward_curve::build! {
	const I_NPOS: sp_runtime::curve::PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000,
		max_inflation: 0_100_000,
		ideal_stake: 0_500_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
}

parameter_types! {
	pub const RewardCurve: &'static sp_runtime::curve::PiecewiseLinear<'static> = &I_NPOS;
	pub static BondingDuration: u32 = 3;
	pub static CurrentEra: u32 = 0;
	pub static Ongoing: bool = false;
}

pub struct MockElection;

impl frame_election_provider_support::ElectionProvider for MockElection {
	type BlockNumber = BlockNumber;
	type AccountId = AccountId;
	type DataProvider = Staking;
	type MaxBackersPerWinner = ConstU32<100>;
	type MaxWinnersPerPage = ConstU32<100>;
	type Pages = ConstU32<1>;
	type MaxBackersPerWinnerFinal = Self::MaxBackersPerWinner;
	type Error = ();

	fn elect(
		_remaining_pages: PageIndex,
	) -> Result<frame_election_provider_support::BoundedSupportsOf<Self>, Self::Error> {
		Err(())
	}

	fn start() -> Result<(), Self::Error> {
		Ok(())
	}

	fn duration() -> Self::BlockNumber {
		0
	}

	fn status() -> Result<bool, ()> {
		if Ongoing::get() {
			Ok(false)
		} else {
			Err(())
		}
	}
}

#[derive_impl(pallet_staking::config_preludes::TestDefaultConfig)]
impl pallet_staking::Config for Runtime {
	type OldCurrency = Balances;
	type Currency = Balances;
	type UnixTime = pallet_timestamp::Pallet<Self>;
	type AdminOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type BondingDuration = BondingDuration;
	type EraPayout = pallet_staking::ConvertCurve<RewardCurve>;
	type ElectionProvider = MockElection;
	type GenesisElectionProvider = Self::ElectionProvider;
	type VoterList = pallet_staking::UseNominatorsAndValidatorsMap<Self>;
	type TargetList = pallet_staking::UseValidatorsMap<Self>;
}

parameter_types! {
	pub static Deposit: u128 = 7;
	pub static BatchSize: u32 = 1;
}

impl fast_unstake::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Deposit = Deposit;
	type Currency = Balances;
	type Staking = Staking;
	type ControlOrigin = frame_system::EnsureRoot<Self::AccountId>;
	type BatchSize = BatchSize;
	type WeightInfo = ();
	type MaxErasToCheckPerBlock = ConstU32<16>;
}

type Block = frame_system::mocking::MockBlock<Runtime>;

frame_support::construct_runtime!(
	pub enum Runtime {
		System: frame_system,
		Timestamp: pallet_timestamp,
		Balances: pallet_balances,
		Staking: pallet_staking,
		FastUnstake: fast_unstake,
	}
);

parameter_types! {
	static FastUnstakeEvents: u32 = 0;
}

pub(crate) fn fast_unstake_events_since_last_call() -> Vec<super::Event<Runtime>> {
	let events = System::events()
		.into_iter()
		.map(|r| r.event)
		.filter_map(|e| if let RuntimeEvent::FastUnstake(inner) = e { Some(inner) } else { None })
		.collect::<Vec<_>>();
	let already_seen = FastUnstakeEvents::get();
	FastUnstakeEvents::set(events.len() as u32);
	events.into_iter().skip(already_seen as usize).collect()
}

pub struct ExtBuilder {
	unexposed: Vec<(AccountId, AccountId, Balance)>,
}

impl Default for ExtBuilder {
	fn default() -> Self {
		Self {
			unexposed: vec![
				(1, 1, 7 + 100),
				(3, 3, 7 + 100),
				(5, 5, 7 + 100),
				(7, 7, 7 + 100),
				(9, 9, 7 + 100),
			],
		}
	}
}

pub(crate) const VALIDATORS_PER_ERA: AccountId = 32;
pub(crate) const VALIDATOR_PREFIX: AccountId = 100;
pub(crate) const NOMINATORS_PER_VALIDATOR_PER_ERA: AccountId = 4;
pub(crate) const NOMINATOR_PREFIX: AccountId = 1000;

impl ExtBuilder {
	pub(crate) fn register_stakers_for_era(era: u32) {
		// validators are prefixed with 100 and nominators with 1000 to prevent conflict. Make sure
		// all the other accounts used in tests are below 100. Also ensure here that we don't
		// overlap.
		assert!(VALIDATOR_PREFIX + VALIDATORS_PER_ERA < NOMINATOR_PREFIX);

		(VALIDATOR_PREFIX..VALIDATOR_PREFIX + VALIDATORS_PER_ERA)
			.map(|v| {
				// for the sake of sanity, let's register this taker as an actual validator.
				let others = (NOMINATOR_PREFIX..
					(NOMINATOR_PREFIX + NOMINATORS_PER_VALIDATOR_PER_ERA))
					.map(|n| IndividualExposure { who: n, value: 0 as Balance })
					.collect::<Vec<_>>();
				(v, Exposure { total: 0, own: 0, others })
			})
			.for_each(|(validator, exposure)| {
				pallet_staking::EraInfo::<T>::set_exposure(era, &validator, exposure);
			});
	}

	pub(crate) fn batch(self, size: u32) -> Self {
		BatchSize::set(size);
		self
	}

	pub(crate) fn build(self) -> sp_io::TestExternalities {
		sp_tracing::try_init_simple();
		let mut storage =
			frame_system::GenesisConfig::<Runtime>::default().build_storage().unwrap();

		let validators_range = VALIDATOR_PREFIX..VALIDATOR_PREFIX + VALIDATORS_PER_ERA;
		let nominators_range =
			NOMINATOR_PREFIX..NOMINATOR_PREFIX + NOMINATORS_PER_VALIDATOR_PER_ERA;

		let _ = pallet_balances::GenesisConfig::<Runtime> {
			balances: self
				.unexposed
				.clone()
				.into_iter()
				.map(|(stash, _, balance)| (stash, balance * 2))
				// give stakers enough balance for stake, ed and fast unstake deposit.
				.chain(validators_range.clone().map(|x| (x, 7 + 1 + 100)))
				.chain(nominators_range.clone().map(|x| (x, 7 + 1 + 100)))
				.collect::<Vec<_>>(),
			..Default::default()
		}
		.assimilate_storage(&mut storage);

		let _ = pallet_staking::GenesisConfig::<Runtime> {
			stakers: self
				.unexposed
				.into_iter()
				.map(|(x, y, z)| (x, y, z, pallet_staking::StakerStatus::Nominator(vec![42])))
				.chain(validators_range.map(|x| (x, x, 100, StakerStatus::Validator)))
				.chain(nominators_range.map(|x| (x, x, 100, StakerStatus::Nominator(vec![x]))))
				.collect::<Vec<_>>(),
			..Default::default()
		}
		.assimilate_storage(&mut storage);

		let mut ext = sp_io::TestExternalities::from(storage);

		ext.execute_with(|| {
			// for events to be deposited.
			frame_system::Pallet::<Runtime>::set_block_number(1);

			for era in 0..=(BondingDuration::get()) {
				Self::register_stakers_for_era(era);
			}

			// because we read this value as a measure of how many validators we have.
			pallet_staking::ValidatorCount::<Runtime>::put(VALIDATORS_PER_ERA as u32);
		});

		ext
	}

	pub fn build_and_execute(self, test: impl FnOnce() -> ()) {
		self.build().execute_with(|| {
			test();
		})
	}
}

pub(crate) fn run_to_block(n: u64, on_idle: bool) {
	System::run_to_block_with::<AllPalletsWithSystem>(
		n,
		frame_system::RunToBlockHooks::default()
			.before_finalize(|_| {
				// Satisfy the timestamp pallet.
				Timestamp::set_timestamp(0);
			})
			.after_initialize(|bn| {
				if on_idle {
					FastUnstake::on_idle(bn, BlockWeights::get().max_block);
				}
			}),
	);
}

pub(crate) fn next_block(on_idle: bool) {
	let current = System::block_number();
	run_to_block(current + 1, on_idle);
}

pub fn assert_unstaked(stash: &AccountId) {
	assert!(!pallet_staking::Bonded::<T>::contains_key(stash));
	assert!(!pallet_staking::Payee::<T>::contains_key(stash));
	assert!(!pallet_staking::Validators::<T>::contains_key(stash));
	assert!(!pallet_staking::Nominators::<T>::contains_key(stash));
}

pub fn create_exposed_nominator(exposed: AccountId, era: u32) {
	// create an exposed nominator in passed era
	let mut exposure = pallet_staking::EraInfo::<T>::get_full_exposure(era, &VALIDATORS_PER_ERA);
	exposure.others.push(IndividualExposure { who: exposed, value: 0 as Balance });
	pallet_staking::EraInfo::<T>::set_exposure(era, &VALIDATORS_PER_ERA, exposure);

	Balances::make_free_balance_be(&exposed, 100);
	assert_ok!(Staking::bond(
		RuntimeOrigin::signed(exposed),
		10,
		pallet_staking::RewardDestination::Staked
	));
	assert_ok!(Staking::nominate(RuntimeOrigin::signed(exposed), vec![exposed]));
	// register the exposed one.
	assert_ok!(FastUnstake::register_fast_unstake(RuntimeOrigin::signed(exposed)));
}

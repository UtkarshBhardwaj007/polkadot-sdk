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

// Tests for Multisig Pallet

#![cfg(test)]

use super::*;
use crate as pallet_multisig;
use frame::{prelude::*, runtime::prelude::*, testing_prelude::*};

type Block = frame_system::mocking::MockBlockU32<Test>;

construct_runtime!(
	pub struct Test {
		System: frame_system,
		Balances: pallet_balances,
		Multisig: pallet_multisig,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountData = pallet_balances::AccountData<u64>;
	// This pallet wishes to overwrite this.
	type BaseCallFilter = TestBaseCallFilter;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
	type ReserveIdentifier = [u8; 8];
	type AccountStore = System;
}

pub struct TestBaseCallFilter;
impl Contains<RuntimeCall> for TestBaseCallFilter {
	fn contains(c: &RuntimeCall) -> bool {
		match *c {
			RuntimeCall::Balances(_) => true,
			// Needed for benchmarking
			RuntimeCall::System(frame_system::Call::remark { .. }) => true,
			_ => false,
		}
	}
}
impl Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = ConstU64<1>;
	type DepositFactor = ConstU64<1>;
	type MaxSignatories = ConstU32<3>;
	type WeightInfo = ();
	type BlockNumberProvider = frame_system::Pallet<Test>;
}

use pallet_balances::Call as BalancesCall;

pub fn new_test_ext() -> TestState {
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	pallet_balances::GenesisConfig::<Test> {
		balances: vec![(1, 10), (2, 10), (3, 10), (4, 10), (5, 2)],
		..Default::default()
	}
	.assimilate_storage(&mut t)
	.unwrap();
	let mut ext = TestState::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

fn now() -> Timepoint<u32> {
	Multisig::timepoint()
}

fn call_transfer(dest: u64, value: u64) -> Box<RuntimeCall> {
	Box::new(RuntimeCall::Balances(BalancesCall::transfer_allow_death { dest, value }))
}

#[test]
fn multisig_deposit_is_taken_and_returned() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let call_weight = call.get_dispatch_info().call_weight;
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call.clone(),
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(1), 2);
		assert_eq!(Balances::reserved_balance(1), 3);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			Some(now()),
			call,
			call_weight
		));
		assert_eq!(Balances::free_balance(1), 5);
		assert_eq!(Balances::reserved_balance(1), 0);
	});
}

#[test]
fn cancel_multisig_returns_deposit() {
	new_test_ext().execute_with(|| {
		let call = call_transfer(6, 15).encode();
		let hash = blake2_256(&call);
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			3,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(2),
			3,
			vec![1, 3],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(1), 6);
		assert_eq!(Balances::reserved_balance(1), 4);
		assert_ok!(Multisig::cancel_as_multi(RuntimeOrigin::signed(1), 3, vec![2, 3], now(), hash));
		assert_eq!(Balances::free_balance(1), 10);
		assert_eq!(Balances::reserved_balance(1), 0);
	});
}

#[test]
fn timepoint_checking_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let hash = blake2_256(&call.encode());

		assert_noop!(
			Multisig::approve_as_multi(
				RuntimeOrigin::signed(2),
				2,
				vec![1, 3],
				Some(now()),
				hash,
				Weight::zero()
			),
			Error::<Test>::UnexpectedTimepoint,
		);

		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));

		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(2),
				2,
				vec![1, 3],
				None,
				call.clone(),
				Weight::zero()
			),
			Error::<Test>::NoTimepoint,
		);
		let later = Timepoint { index: 1, ..now() };
		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(2),
				2,
				vec![1, 3],
				Some(later),
				call,
				Weight::zero()
			),
			Error::<Test>::WrongTimepoint,
		);
	});
}

#[test]
fn multisig_2_of_3_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let call_weight = call.get_dispatch_info().call_weight;
		let hash = blake2_256(&call.encode());
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(6), 0);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			Some(now()),
			call,
			call_weight
		));
		assert_eq!(Balances::free_balance(6), 15);
	});
}

#[test]
fn multisig_3_of_3_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 3);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let call_weight = call.get_dispatch_info().call_weight;
		let hash = blake2_256(&call.encode());
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			3,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(2),
			3,
			vec![1, 3],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(6), 0);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(3),
			3,
			vec![1, 2],
			Some(now()),
			call,
			call_weight
		));
		assert_eq!(Balances::free_balance(6), 15);
	});
}

#[test]
fn cancel_multisig_works() {
	new_test_ext().execute_with(|| {
		let call = call_transfer(6, 15).encode();
		let hash = blake2_256(&call);
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			3,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(2),
			3,
			vec![1, 3],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_noop!(
			Multisig::cancel_as_multi(RuntimeOrigin::signed(2), 3, vec![1, 3], now(), hash),
			Error::<Test>::NotOwner,
		);
		assert_ok!(Multisig::cancel_as_multi(RuntimeOrigin::signed(1), 3, vec![2, 3], now(), hash),);
	});
}

#[test]
fn multisig_2_of_3_as_multi_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let call_weight = call.get_dispatch_info().call_weight;
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call.clone(),
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(6), 0);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			Some(now()),
			call,
			call_weight
		));
		assert_eq!(Balances::free_balance(6), 15);
	});
}

#[test]
fn multisig_2_of_3_as_multi_with_many_calls_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call1 = call_transfer(6, 10);
		let call1_weight = call1.get_dispatch_info().call_weight;
		let call2 = call_transfer(7, 5);
		let call2_weight = call2.get_dispatch_info().call_weight;

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call1.clone(),
			Weight::zero()
		));
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			None,
			call2.clone(),
			Weight::zero()
		));
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(3),
			2,
			vec![1, 2],
			Some(now()),
			call1,
			call1_weight
		));
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(3),
			2,
			vec![1, 2],
			Some(now()),
			call2,
			call2_weight
		));

		assert_eq!(Balances::free_balance(6), 10);
		assert_eq!(Balances::free_balance(7), 5);
	});
}

#[test]
fn multisig_2_of_3_cannot_reissue_same_call() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 10);
		let call_weight = call.get_dispatch_info().call_weight;
		let hash = blake2_256(&call.encode());
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call.clone(),
			Weight::zero()
		));
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			Some(now()),
			call.clone(),
			call_weight
		));
		assert_eq!(Balances::free_balance(multi), 5);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call.clone(),
			Weight::zero()
		));
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(3),
			2,
			vec![1, 2],
			Some(now()),
			call.clone(),
			call_weight
		));

		System::assert_last_event(
			pallet_multisig::Event::MultisigExecuted {
				approving: 3,
				timepoint: now(),
				multisig: multi,
				call_hash: hash,
				result: Err(TokenError::FundsUnavailable.into()),
			}
			.into(),
		);
	});
}

#[test]
fn minimum_threshold_check_works() {
	new_test_ext().execute_with(|| {
		let call = call_transfer(6, 15);
		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(1),
				0,
				vec![2],
				None,
				call.clone(),
				Weight::zero()
			),
			Error::<Test>::MinimumThreshold,
		);
		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(1),
				1,
				vec![2],
				None,
				call.clone(),
				Weight::zero()
			),
			Error::<Test>::MinimumThreshold,
		);
	});
}

#[test]
fn too_many_signatories_fails() {
	new_test_ext().execute_with(|| {
		let call = call_transfer(6, 15);
		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(1),
				2,
				vec![2, 3, 4],
				None,
				call.clone(),
				Weight::zero()
			),
			Error::<Test>::TooManySignatories,
		);
	});
}

#[test]
fn duplicate_approvals_are_ignored() {
	new_test_ext().execute_with(|| {
		let call = call_transfer(6, 15).encode();
		let hash = blake2_256(&call);
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_noop!(
			Multisig::approve_as_multi(
				RuntimeOrigin::signed(1),
				2,
				vec![2, 3],
				Some(now()),
				hash,
				Weight::zero()
			),
			Error::<Test>::AlreadyApproved,
		);
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(2),
			2,
			vec![1, 3],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_noop!(
			Multisig::approve_as_multi(
				RuntimeOrigin::signed(3),
				2,
				vec![1, 2],
				Some(now()),
				hash,
				Weight::zero()
			),
			Error::<Test>::AlreadyApproved,
		);
	});
}

#[test]
fn multisig_1_of_3_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 1);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let hash = blake2_256(&call.encode());
		assert_noop!(
			Multisig::approve_as_multi(
				RuntimeOrigin::signed(1),
				1,
				vec![2, 3],
				None,
				hash,
				Weight::zero()
			),
			Error::<Test>::MinimumThreshold,
		);
		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(1),
				1,
				vec![2, 3],
				None,
				call.clone(),
				Weight::zero()
			),
			Error::<Test>::MinimumThreshold,
		);
		assert_ok!(Multisig::as_multi_threshold_1(
			RuntimeOrigin::signed(1),
			vec![2, 3],
			call_transfer(6, 15)
		));

		assert_eq!(Balances::free_balance(6), 15);
	});
}

#[test]
fn multisig_filters() {
	new_test_ext().execute_with(|| {
		let call = Box::new(RuntimeCall::System(frame_system::Call::set_code { code: vec![] }));
		assert_noop!(
			Multisig::as_multi_threshold_1(RuntimeOrigin::signed(1), vec![2], call.clone()),
			DispatchError::from(frame_system::Error::<Test>::CallFiltered),
		);
	});
}

#[test]
fn weight_check_works() {
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 2);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(1),
			2,
			vec![2, 3],
			None,
			call.clone(),
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(6), 0);

		assert_noop!(
			Multisig::as_multi(
				RuntimeOrigin::signed(2),
				2,
				vec![1, 3],
				Some(now()),
				call,
				Weight::zero()
			),
			Error::<Test>::MaxWeightTooLow,
		);
	});
}

#[test]
fn multisig_handles_no_preimage_after_all_approve() {
	// This test checks the situation where everyone approves a multi-sig, but no-one provides the
	// call data. In the end, any of the multisig callers can approve again with the call data and
	// the call will go through.
	new_test_ext().execute_with(|| {
		let multi = Multisig::multi_account_id(&[1, 2, 3][..], 3);
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(1), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(2), multi, 5));
		assert_ok!(Balances::transfer_allow_death(RuntimeOrigin::signed(3), multi, 5));

		let call = call_transfer(6, 15);
		let call_weight = call.get_dispatch_info().call_weight;
		let hash = blake2_256(&call.encode());
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(1),
			3,
			vec![2, 3],
			None,
			hash,
			Weight::zero()
		));
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(2),
			3,
			vec![1, 3],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_ok!(Multisig::approve_as_multi(
			RuntimeOrigin::signed(3),
			3,
			vec![1, 2],
			Some(now()),
			hash,
			Weight::zero()
		));
		assert_eq!(Balances::free_balance(6), 0);

		assert_ok!(Multisig::as_multi(
			RuntimeOrigin::signed(3),
			3,
			vec![1, 2],
			Some(now()),
			call,
			call_weight
		));
		assert_eq!(Balances::free_balance(6), 15);
	});
}

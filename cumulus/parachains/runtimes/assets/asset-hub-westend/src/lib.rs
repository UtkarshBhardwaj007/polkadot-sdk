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

//! # Asset Hub Westend Runtime
//!
//! Testnet for Asset Hub Polkadot.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "512"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

mod bridge_to_ethereum_config;
mod genesis_config_presets;
mod weights;
pub mod xcm_config;

// Configurations for next functionality.
mod bag_thresholds;
pub mod governance;
mod staking;
use governance::{pallet_custom_origins, FellowshipAdmin, GeneralAdmin, StakingAdmin, Treasurer};

extern crate alloc;

use alloc::{vec, vec::Vec};
use assets_common::{
	local_and_foreign_assets::{LocalFromLeft, TargetFromLeft},
	AssetIdForPoolAssets, AssetIdForPoolAssetsConvert, AssetIdForTrustBackedAssetsConvert,
};
use bp_asset_hub_westend::CreateForeignAssetDeposit;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use cumulus_pallet_parachain_system::{RelayNumberMonotonicallyIncreases, RelaychainDataProvider};
use cumulus_primitives_core::{
	relay_chain::AccountIndex, AggregateMessageOrigin, ClaimQueueOffset, CoreSelector, ParaId,
};
use frame_support::{
	construct_runtime, derive_impl,
	dispatch::DispatchClass,
	genesis_builder_helper::{build_state, get_preset},
	ord_parameter_types, parameter_types,
	traits::{
		fungible::{self, HoldConsideration},
		fungibles,
		tokens::{imbalance::ResolveAssetTo, nonfungibles_v2::Inspect},
		AsEnsureOriginWithArg, ConstBool, ConstU128, ConstU32, ConstU64, ConstU8,
		ConstantStoragePrice, EitherOfDiverse, Equals, InstanceFilter, LinearStoragePrice, Nothing,
		TransformOrigin, WithdrawReasons,
	},
	weights::{ConstantMultiplier, Weight},
	BoundedVec, PalletId,
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot, EnsureSigned, EnsureSignedBy,
};
use pallet_asset_conversion_tx_payment::SwapAssetAdapter;
use pallet_assets::precompiles::{InlineIdConfig, ERC20};
use pallet_nfts::{DestroyWitness, PalletFeatures};
use pallet_nomination_pools::PoolId;
use pallet_revive::evm::runtime::EthExtra;
use pallet_xcm::{precompiles::XcmPrecompile, EnsureXcm};
use parachains_common::{
	impls::DealWithFees, message_queue::*, AccountId, AssetIdForTrustBackedAssets, AuraId, Balance,
	BlockNumber, CollectionId, Hash, Header, ItemId, Nonce, Signature, AVERAGE_ON_INITIALIZE_RATIO,
	NORMAL_DISPATCH_RATIO,
};
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
	generic, impl_opaque_keys,
	traits::{AccountIdConversion, BlakeTwo256, Block as BlockT, ConvertInto, Saturating, Verify},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, Perbill, Permill, RuntimeDebug,
};
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;
use testnet_parachains_constants::westend::{
	consensus::*, currency::*, fee::WeightToFee, snowbridge::EthereumNetwork, time::*,
};
use westend_runtime_constants::time::DAYS as RC_DAYS;
use xcm_config::{
	ForeignAssetsConvertedConcreteId, LocationToAccountId, PoolAssetsConvertedConcreteId,
	PoolAssetsPalletLocation, TrustBackedAssetsConvertedConcreteId,
	TrustBackedAssetsPalletLocation, WestendLocation, XcmOriginToTransactDispatchOrigin,
};

#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

use assets_common::{
	foreign_creators::ForeignCreators,
	matching::{FromNetwork, FromSiblingParachain},
};
use polkadot_runtime_common::{BlockHashCount, SlowAdjustingFeeUpdate};
use weights::{BlockExecutionWeight, ExtrinsicBaseWeight, InMemoryDbWeight};
use xcm::{
	latest::prelude::AssetId,
	prelude::{
		VersionedAsset, VersionedAssetId, VersionedAssets, VersionedLocation, VersionedXcm,
		XcmVersion,
	},
};

#[cfg(feature = "runtime-benchmarks")]
use frame_support::traits::PalletInfoAccess;

#[cfg(feature = "runtime-benchmarks")]
use xcm::latest::prelude::{
	Asset, Assets as XcmAssets, Fungible, Here, InteriorLocation, Junction, Junction::*, Location,
	NetworkId, NonFungible, Parent, ParentThen, Response, WeightLimit, XCM_VERSION,
};

use xcm_runtime_apis::{
	dry_run::{CallDryRunEffects, Error as XcmDryRunApiError, XcmDryRunEffects},
	fees::Error as XcmPaymentApiError,
};

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	// Note: "westmint" is the legacy name for this chain. It has been renamed to
	// "asset-hub-westend". Many wallets/tools depend on the `spec_name`, so it remains "westmint"
	// for the time being. Wallets/tools should update to treat "asset-hub-westend" equally.
	spec_name: alloc::borrow::Cow::Borrowed("westmint"),
	impl_name: alloc::borrow::Cow::Borrowed("westmint"),
	authoring_version: 1,
	spec_version: 1_019_002,
	impl_version: 0,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 16,
	system_version: 1,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved = Some(
				MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
			);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
	pub const SS58Prefix: u8 = 42;
}

// Configure FRAME pallets to include in runtime.
#[derive_impl(frame_system::config_preludes::ParaChainDefaultConfig)]
impl frame_system::Config for Runtime {
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type AccountId = AccountId;
	type Nonce = Nonce;
	type Hash = Hash;
	type Block = Block;
	type BlockHashCount = BlockHashCount;
	type DbWeight = InMemoryDbWeight;
	type Version = Version;
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type ExtensionsWeightInfo = weights::frame_system_extensions::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
	type MaxConsumers = frame_support::traits::ConstU32<16>;
	type MultiBlockMigrator = MultiBlockMigrations;
}

impl cumulus_pallet_weight_reclaim::Config for Runtime {
	type WeightInfo = weights::cumulus_pallet_weight_reclaim::WeightInfo<Runtime>;
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<0>;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

impl pallet_authorship::Config for Runtime {
	type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Self, Aura>;
	type EventHandler = (CollatorSelection,);
}

parameter_types! {
	pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = ConstU32<50>;
	/// The type for recording an account's balance.
	type Balance = Balance;
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
	type MaxReserves = ConstU32<50>;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type FreezeIdentifier = RuntimeFreezeReason;
	type MaxFreezes = frame_support::traits::VariantCountOf<RuntimeFreezeReason>;
	type DoneSlashHandler = ();
}

parameter_types! {
	/// Relay Chain `TransactionByteFee` / 10
	pub const TransactionByteFee: Balance = MILLICENTS;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction =
		pallet_transaction_payment::FungibleAdapter<Balances, DealWithFees<Runtime>>;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
	type OperationalFeeMultiplier = ConstU8<5>;
	type WeightInfo = weights::pallet_transaction_payment::WeightInfo<Runtime>;
}

parameter_types! {
	pub const AssetDeposit: Balance = UNITS / 10; // 1 / 10 WND deposit to create asset
	pub const AssetAccountDeposit: Balance = deposit(1, 16);
	pub const ApprovalDeposit: Balance = EXISTENTIAL_DEPOSIT;
	pub const AssetsStringLimit: u32 = 50;
	/// Key = 32 bytes, Value = 36 bytes (32+1+1+1+1)
	// https://github.com/paritytech/substrate/blob/069917b/frame/assets/src/lib.rs#L257L271
	pub const MetadataDepositBase: Balance = deposit(1, 68);
	pub const MetadataDepositPerByte: Balance = deposit(0, 1);
}

pub type AssetsForceOrigin = EnsureRoot<AccountId>;

// Called "Trust Backed" assets because these are generally registered by some account, and users of
// the asset assume it has some claimed backing. The pallet is called `Assets` in
// `construct_runtime` to avoid breaking changes on storage reads.
pub type TrustBackedAssetsInstance = pallet_assets::Instance1;
type TrustBackedAssetsCall = pallet_assets::Call<Runtime, TrustBackedAssetsInstance>;
impl pallet_assets::Config<TrustBackedAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = AssetIdForTrustBackedAssets;
	type AssetIdParameter = codec::Compact<AssetIdForTrustBackedAssets>;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = AssetDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = AssetsStringLimit;
	type Holder = ();
	type Freezer = AssetsFreezer;
	type Extra = ();
	type WeightInfo = weights::pallet_assets_local::WeightInfo<Runtime>;
	type CallbackHandle = pallet_assets::AutoIncAssetId<Runtime, TrustBackedAssetsInstance>;
	type AssetAccountDeposit = AssetAccountDeposit;
	type RemoveItemsLimit = ConstU32<1000>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

// Allow Freezes for the `Assets` pallet
pub type AssetsFreezerInstance = pallet_assets_freezer::Instance1;
impl pallet_assets_freezer::Config<AssetsFreezerInstance> for Runtime {
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type RuntimeEvent = RuntimeEvent;
}

parameter_types! {
	pub const AssetConversionPalletId: PalletId = PalletId(*b"py/ascon");
	pub const LiquidityWithdrawalFee: Permill = Permill::from_percent(0);
}

ord_parameter_types! {
	pub const AssetConversionOrigin: sp_runtime::AccountId32 =
		AccountIdConversion::<sp_runtime::AccountId32>::into_account_truncating(&AssetConversionPalletId::get());
}

pub type PoolAssetsInstance = pallet_assets::Instance3;
impl pallet_assets::Config<PoolAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type RemoveItemsLimit = ConstU32<1000>;
	type AssetId = u32;
	type AssetIdParameter = u32;
	type Currency = Balances;
	type CreateOrigin =
		AsEnsureOriginWithArg<EnsureSignedBy<AssetConversionOrigin, sp_runtime::AccountId32>>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = ConstU128<0>;
	type AssetAccountDeposit = ConstU128<0>;
	type MetadataDepositBase = ConstU128<0>;
	type MetadataDepositPerByte = ConstU128<0>;
	type ApprovalDeposit = ConstU128<0>;
	type StringLimit = ConstU32<50>;
	type Holder = ();
	type Freezer = PoolAssetsFreezer;
	type Extra = ();
	type WeightInfo = weights::pallet_assets_pool::WeightInfo<Runtime>;
	type CallbackHandle = ();
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

// Allow Freezes for the `PoolAssets` pallet
pub type PoolAssetsFreezerInstance = pallet_assets_freezer::Instance3;
impl pallet_assets_freezer::Config<PoolAssetsFreezerInstance> for Runtime {
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type RuntimeEvent = RuntimeEvent;
}

/// Union fungibles implementation for `Assets` and `ForeignAssets`.
pub type LocalAndForeignAssets = fungibles::UnionOf<
	Assets,
	ForeignAssets,
	LocalFromLeft<
		AssetIdForTrustBackedAssetsConvert<TrustBackedAssetsPalletLocation, xcm::v5::Location>,
		AssetIdForTrustBackedAssets,
		xcm::v5::Location,
	>,
	xcm::v5::Location,
	AccountId,
>;

/// Union fungibles implementation for `AssetsFreezer` and `ForeignAssetsFreezer`.
pub type LocalAndForeignAssetsFreezer = fungibles::UnionOf<
	AssetsFreezer,
	ForeignAssetsFreezer,
	LocalFromLeft<
		AssetIdForTrustBackedAssetsConvert<TrustBackedAssetsPalletLocation, xcm::v5::Location>,
		AssetIdForTrustBackedAssets,
		xcm::v5::Location,
	>,
	xcm::v5::Location,
	AccountId,
>;

/// Union fungibles implementation for [`LocalAndForeignAssets`] and `Balances`.
pub type NativeAndNonPoolAssets = fungible::UnionOf<
	Balances,
	LocalAndForeignAssets,
	TargetFromLeft<WestendLocation, xcm::v5::Location>,
	xcm::v5::Location,
	AccountId,
>;

/// Union fungibles implementation for [`LocalAndForeignAssetsFreezer`] and [`Balances`].
pub type NativeAndNonPoolAssetsFreezer = fungible::UnionOf<
	Balances,
	LocalAndForeignAssetsFreezer,
	TargetFromLeft<WestendLocation, xcm::v5::Location>,
	xcm::v5::Location,
	AccountId,
>;

/// Union fungibles implementation for [`PoolAssets`] and [`NativeAndNonPoolAssets`].
///
/// NOTE: Should be kept updated to include ALL balances and assets in the runtime.
pub type NativeAndAllAssets = fungibles::UnionOf<
	PoolAssets,
	NativeAndNonPoolAssets,
	LocalFromLeft<
		AssetIdForPoolAssetsConvert<PoolAssetsPalletLocation, xcm::v5::Location>,
		AssetIdForPoolAssets,
		xcm::v5::Location,
	>,
	xcm::v5::Location,
	AccountId,
>;

/// Union fungibles implementation for [`PoolAssetsFreezer`] and [`NativeAndNonPoolAssetsFreezer`].
///
/// NOTE: Should be kept updated to include ALL balances and assets in the runtime.
pub type NativeAndAllAssetsFreezer = fungibles::UnionOf<
	PoolAssetsFreezer,
	NativeAndNonPoolAssetsFreezer,
	LocalFromLeft<
		AssetIdForPoolAssetsConvert<PoolAssetsPalletLocation, xcm::v5::Location>,
		AssetIdForPoolAssets,
		xcm::v5::Location,
	>,
	xcm::v5::Location,
	AccountId,
>;

pub type PoolIdToAccountId = pallet_asset_conversion::AccountIdConverter<
	AssetConversionPalletId,
	(xcm::v5::Location, xcm::v5::Location),
>;

impl pallet_asset_conversion::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type HigherPrecisionBalance = sp_core::U256;
	type AssetKind = xcm::v5::Location;
	type Assets = NativeAndNonPoolAssets;
	type PoolId = (Self::AssetKind, Self::AssetKind);
	type PoolLocator = pallet_asset_conversion::WithFirstAsset<
		WestendLocation,
		AccountId,
		Self::AssetKind,
		PoolIdToAccountId,
	>;
	type PoolAssetId = u32;
	type PoolAssets = PoolAssets;
	type PoolSetupFee = ConstU128<0>; // Asset class deposit fees are sufficient to prevent spam
	type PoolSetupFeeAsset = WestendLocation;
	type PoolSetupFeeTarget = ResolveAssetTo<AssetConversionOrigin, Self::Assets>;
	type LiquidityWithdrawalFee = LiquidityWithdrawalFee;
	type LPFee = ConstU32<3>;
	type PalletId = AssetConversionPalletId;
	type MaxSwapPathLength = ConstU32<3>;
	type MintMinLiquidity = ConstU128<100>;
	type WeightInfo = weights::pallet_asset_conversion::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = assets_common::benchmarks::AssetPairFactory<
		WestendLocation,
		parachain_info::Pallet<Runtime>,
		xcm_config::TrustBackedAssetsPalletIndex,
		xcm::v5::Location,
	>;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct PalletAssetRewardsBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl pallet_asset_rewards::benchmarking::BenchmarkHelper<xcm::v5::Location>
	for PalletAssetRewardsBenchmarkHelper
{
	fn staked_asset() -> Location {
		Location::new(
			0,
			[PalletInstance(<Assets as PalletInfoAccess>::index() as u8), GeneralIndex(100)],
		)
	}
	fn reward_asset() -> Location {
		Location::new(
			0,
			[PalletInstance(<Assets as PalletInfoAccess>::index() as u8), GeneralIndex(101)],
		)
	}
}

parameter_types! {
	pub const MinVestedTransfer: Balance = 100 * CENTS;
	pub UnvestedFundsAllowedWithdrawReasons: WithdrawReasons =
		WithdrawReasons::except(WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE);
}

impl pallet_vesting::Config for Runtime {
	const MAX_VESTING_SCHEDULES: u32 = 100;
	type BlockNumberProvider = RelaychainDataProvider<Runtime>;
	type BlockNumberToBalance = ConvertInto;
	type Currency = Balances;
	type MinVestedTransfer = MinVestedTransfer;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_vesting::WeightInfo<Runtime>;
	type UnvestedFundsAllowedWithdrawReasons = UnvestedFundsAllowedWithdrawReasons;
}

parameter_types! {
	pub const AssetRewardsPalletId: PalletId = PalletId(*b"py/astrd");
	pub const RewardsPoolCreationHoldReason: RuntimeHoldReason =
		RuntimeHoldReason::AssetRewards(pallet_asset_rewards::HoldReason::PoolCreation);
	// 1 item, 135 bytes into the storage on pool creation.
	pub const StakePoolCreationDeposit: Balance = deposit(1, 135);
}

impl pallet_asset_rewards::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type PalletId = AssetRewardsPalletId;
	type Balance = Balance;
	type Assets = NativeAndAllAssets;
	type AssetsFreezer = NativeAndAllAssetsFreezer;
	type AssetId = xcm::v5::Location;
	type CreatePoolOrigin = EnsureSigned<AccountId>;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type Consideration = HoldConsideration<
		AccountId,
		Balances,
		RewardsPoolCreationHoldReason,
		ConstantStoragePrice<StakePoolCreationDeposit, Balance>,
	>;
	type WeightInfo = weights::pallet_asset_rewards::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = PalletAssetRewardsBenchmarkHelper;
}

impl pallet_asset_conversion_ops::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type PriorAccountIdConverter = pallet_asset_conversion::AccountIdConverterNoSeed<
		<Runtime as pallet_asset_conversion::Config>::PoolId,
	>;
	type AssetsRefund = <Runtime as pallet_asset_conversion::Config>::Assets;
	type PoolAssetsRefund = <Runtime as pallet_asset_conversion::Config>::PoolAssets;
	type PoolAssetsTeam = <Runtime as pallet_asset_conversion::Config>::PoolAssets;
	type DepositAsset = Balances;
	type WeightInfo = weights::pallet_asset_conversion_ops::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ForeignAssetsAssetDeposit: Balance = CreateForeignAssetDeposit::get();
	pub const ForeignAssetsAssetAccountDeposit: Balance = AssetAccountDeposit::get();
	pub const ForeignAssetsApprovalDeposit: Balance = ApprovalDeposit::get();
	pub const ForeignAssetsAssetsStringLimit: u32 = AssetsStringLimit::get();
	pub const ForeignAssetsMetadataDepositBase: Balance = MetadataDepositBase::get();
	pub const ForeignAssetsMetadataDepositPerByte: Balance = MetadataDepositPerByte::get();
}

/// Assets managed by some foreign location. Note: we do not declare a `ForeignAssetsCall` type, as
/// this type is used in proxy definitions. We assume that a foreign location would not want to set
/// an individual, local account as a proxy for the issuance of their assets. This issuance should
/// be managed by the foreign location's governance.
pub type ForeignAssetsInstance = pallet_assets::Instance2;
impl pallet_assets::Config<ForeignAssetsInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = xcm::v5::Location;
	type AssetIdParameter = xcm::v5::Location;
	type Currency = Balances;
	type CreateOrigin = ForeignCreators<
		(
			FromSiblingParachain<parachain_info::Pallet<Runtime>, xcm::v5::Location>,
			FromNetwork<xcm_config::UniversalLocation, EthereumNetwork, xcm::v5::Location>,
			xcm_config::bridging::to_rococo::RococoAssetFromAssetHubRococo,
		),
		LocationToAccountId,
		AccountId,
		xcm::v5::Location,
	>;
	type ForceOrigin = AssetsForceOrigin;
	type AssetDeposit = ForeignAssetsAssetDeposit;
	type MetadataDepositBase = ForeignAssetsMetadataDepositBase;
	type MetadataDepositPerByte = ForeignAssetsMetadataDepositPerByte;
	type ApprovalDeposit = ForeignAssetsApprovalDeposit;
	type StringLimit = ForeignAssetsAssetsStringLimit;
	type Holder = ();
	type Freezer = ForeignAssetsFreezer;
	type Extra = ();
	type WeightInfo = weights::pallet_assets_foreign::WeightInfo<Runtime>;
	type CallbackHandle = ();
	type AssetAccountDeposit = ForeignAssetsAssetAccountDeposit;
	type RemoveItemsLimit = frame_support::traits::ConstU32<1000>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = xcm_config::XcmBenchmarkHelper;
}

// Allow Freezes for the `ForeignAssets` pallet
pub type ForeignAssetsFreezerInstance = pallet_assets_freezer::Instance2;
impl pallet_assets_freezer::Config<ForeignAssetsFreezerInstance> for Runtime {
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type RuntimeEvent = RuntimeEvent;
}

parameter_types! {
	// One storage item; key size is 32; value is size 4+4+16+32 bytes = 56 bytes.
	pub const DepositBase: Balance = deposit(1, 88);
	// Additional storage item size of 32 bytes.
	pub const DepositFactor: Balance = deposit(0, 32);
	pub const MaxSignatories: u32 = 100;
}

impl pallet_multisig::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type DepositBase = DepositBase;
	type DepositFactor = DepositFactor;
	type MaxSignatories = MaxSignatories;
	type WeightInfo = weights::pallet_multisig::WeightInfo<Runtime>;
	type BlockNumberProvider = System;
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	// One storage item; key size 32, value size 8; .
	pub const ProxyDepositBase: Balance = deposit(1, 40);
	// Additional storage item size of 33 bytes.
	pub const ProxyDepositFactor: Balance = deposit(0, 33);
	pub const MaxProxies: u16 = 32;
	// One storage item; key size 32, value size 16
	pub const AnnouncementDepositBase: Balance = deposit(1, 48);
	pub const AnnouncementDepositFactor: Balance = deposit(0, 66);
	pub const MaxPending: u16 = 32;
}

/// The type used to represent the kinds of proxying allowed.
#[derive(
	Copy,
	Clone,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Encode,
	Decode,
	DecodeWithMemTracking,
	RuntimeDebug,
	MaxEncodedLen,
	scale_info::TypeInfo,
)]
pub enum ProxyType {
	/// Fully permissioned proxy. Can execute any call on behalf of _proxied_.
	Any,
	/// Can execute any call that does not transfer funds or assets.
	NonTransfer,
	/// Proxy with the ability to reject time-delay proxy announcements.
	CancelProxy,
	/// Assets proxy. Can execute any call from `assets`, **including asset transfers**.
	Assets,
	/// Owner proxy. Can execute calls related to asset ownership.
	AssetOwner,
	/// Asset manager. Can execute calls related to asset management.
	AssetManager,
	/// Collator selection proxy. Can execute calls related to collator selection mechanism.
	Collator,
	// New variants introduced by the Asset Hub Migration from the Relay Chain.
	/// Allow to do governance.
	///
	/// Contains pallets `Treasury`, `Bounties`, `Utility`, `ChildBounties`, `ConvictionVoting`,
	/// `Referenda` and `Whitelist`.
	Governance,
	/// Allows access to staking related calls.
	///
	/// Contains the `Staking`, `Session`, `Utility`, `FastUnstake`, `VoterList`, `NominationPools`
	/// pallets.
	Staking,
	/// Allows access to nomination pools related calls.
	///
	/// Contains the `NominationPools` and `Utility` pallets.
	NominationPools,

	/// Placeholder variant to track the state before the Asset Hub Migration.
	OldSudoBalances,
	/// Placeholder variant to track the state before the Asset Hub Migration.
	OldIdentityJudgement,
	/// Placeholder variant to track the state before the Asset Hub Migration.
	OldAuction,
	/// Placeholder variant to track the state before the Asset Hub Migration.
	OldParaRegistration,
}
impl Default for ProxyType {
	fn default() -> Self {
		Self::Any
	}
}

impl InstanceFilter<RuntimeCall> for ProxyType {
	fn filter(&self, c: &RuntimeCall) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::OldSudoBalances |
			ProxyType::OldIdentityJudgement |
			ProxyType::OldAuction |
			ProxyType::OldParaRegistration => false,
			ProxyType::NonTransfer => !matches!(
				c,
				RuntimeCall::Balances { .. } |
					RuntimeCall::Assets { .. } |
					RuntimeCall::NftFractionalization { .. } |
					RuntimeCall::Nfts { .. } |
					RuntimeCall::Uniques { .. } |
					RuntimeCall::Scheduler(..) |
					RuntimeCall::Treasury(..) |
					// We allow calling `vest` and merging vesting schedules, but obviously not
					// vested transfers.
					RuntimeCall::Vesting(pallet_vesting::Call::vested_transfer { .. }) |
					RuntimeCall::ConvictionVoting(..) |
					RuntimeCall::Referenda(..) |
					RuntimeCall::Whitelist(..)
			),
			ProxyType::CancelProxy => matches!(
				c,
				RuntimeCall::Proxy(pallet_proxy::Call::reject_announcement { .. }) |
					RuntimeCall::Utility { .. } |
					RuntimeCall::Multisig { .. }
			),
			ProxyType::Assets => {
				matches!(
					c,
					RuntimeCall::Assets { .. } |
						RuntimeCall::Utility { .. } |
						RuntimeCall::Multisig { .. } |
						RuntimeCall::NftFractionalization { .. } |
						RuntimeCall::Nfts { .. } |
						RuntimeCall::Uniques { .. }
				)
			},
			ProxyType::AssetOwner => matches!(
				c,
				RuntimeCall::Assets(TrustBackedAssetsCall::create { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::start_destroy { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::destroy_accounts { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::destroy_approvals { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::finish_destroy { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::transfer_ownership { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::set_team { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::set_metadata { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::clear_metadata { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::set_min_balance { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::create { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::destroy { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::redeposit { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::transfer_ownership { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::set_team { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::set_collection_max_supply { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::lock_collection { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::create { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::destroy { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::transfer_ownership { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::set_team { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::set_metadata { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::set_attribute { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::set_collection_metadata { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::clear_metadata { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::clear_attribute { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::clear_collection_metadata { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::set_collection_max_supply { .. }) |
					RuntimeCall::Utility { .. } |
					RuntimeCall::Multisig { .. }
			),
			ProxyType::AssetManager => matches!(
				c,
				RuntimeCall::Assets(TrustBackedAssetsCall::mint { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::burn { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::freeze { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::block { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::thaw { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::freeze_asset { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::thaw_asset { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::touch_other { .. }) |
					RuntimeCall::Assets(TrustBackedAssetsCall::refund_other { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::force_mint { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::update_mint_settings { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::mint_pre_signed { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::set_attributes_pre_signed { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::lock_item_transfer { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::unlock_item_transfer { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::lock_item_properties { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::set_metadata { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::clear_metadata { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::set_collection_metadata { .. }) |
					RuntimeCall::Nfts(pallet_nfts::Call::clear_collection_metadata { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::mint { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::burn { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::freeze { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::thaw { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::freeze_collection { .. }) |
					RuntimeCall::Uniques(pallet_uniques::Call::thaw_collection { .. }) |
					RuntimeCall::Utility { .. } |
					RuntimeCall::Multisig { .. }
			),
			ProxyType::Collator => matches!(
				c,
				RuntimeCall::CollatorSelection { .. } |
					RuntimeCall::Utility { .. } |
					RuntimeCall::Multisig { .. }
			),
			// New variants introduced by the Asset Hub Migration from the Relay Chain.
			ProxyType::Governance => matches!(
				c,
				RuntimeCall::Treasury(..) |
					RuntimeCall::Utility(..) |
					RuntimeCall::ConvictionVoting(..) |
					RuntimeCall::Referenda(..) |
					RuntimeCall::Whitelist(..)
			),
			ProxyType::Staking => {
				matches!(
					c,
					RuntimeCall::Staking(..) |
						RuntimeCall::Session(..) |
						RuntimeCall::Utility(..) |
						RuntimeCall::NominationPools(..) |
						RuntimeCall::FastUnstake(..) |
						RuntimeCall::VoterList(..)
				)
			},
			ProxyType::NominationPools => {
				matches!(c, RuntimeCall::NominationPools(..) | RuntimeCall::Utility(..))
			},
		}
	}

	fn is_superset(&self, o: &Self) -> bool {
		match (self, o) {
			(x, y) if x == y => true,
			(ProxyType::Any, _) => true,
			(_, ProxyType::Any) => false,
			(ProxyType::Assets, ProxyType::AssetOwner) => true,
			(ProxyType::Assets, ProxyType::AssetManager) => true,
			(
				ProxyType::NonTransfer,
				ProxyType::Collator |
				ProxyType::Governance |
				ProxyType::Staking |
				ProxyType::NominationPools,
			) => true,
			_ => false,
		}
	}
}

impl pallet_proxy::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type ProxyType = ProxyType;
	type ProxyDepositBase = ProxyDepositBase;
	type ProxyDepositFactor = ProxyDepositFactor;
	type MaxProxies = MaxProxies;
	type WeightInfo = weights::pallet_proxy::WeightInfo<Runtime>;
	type MaxPending = MaxPending;
	type CallHasher = BlakeTwo256;
	type AnnouncementDepositBase = AnnouncementDepositBase;
	type AnnouncementDepositFactor = AnnouncementDepositFactor;
	type BlockNumberProvider = RelaychainDataProvider<Runtime>;
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
}

impl cumulus_pallet_parachain_system::Config for Runtime {
	type WeightInfo = weights::cumulus_pallet_parachain_system::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
	type ReservedDmpWeight = ReservedDmpWeight;
	type OutboundXcmpMessageSource = XcmpQueue;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberMonotonicallyIncreases;
	type ConsensusHook = ConsensusHook;
	type SelectCore = cumulus_pallet_parachain_system::DefaultCoreSelector<Runtime>;
	type RelayParentOffset = ConstU32<0>;
}

type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
	Runtime,
	RELAY_CHAIN_SLOT_DURATION_MILLIS,
	BLOCK_PROCESSING_VELOCITY,
	UNINCLUDED_SEGMENT_CAPACITY,
>;

impl parachain_info::Config for Runtime {}

parameter_types! {
	pub MessageQueueServiceWeight: Weight = Perbill::from_percent(35) * RuntimeBlockWeights::get().max_block;
}

impl pallet_message_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_message_queue::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type MessageProcessor = pallet_message_queue::mock_helpers::NoopMessageProcessor<
		cumulus_primitives_core::AggregateMessageOrigin,
	>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type MessageProcessor = xcm_builder::ProcessXcmMessage<
		AggregateMessageOrigin,
		xcm_executor::XcmExecutor<xcm_config::XcmConfig>,
		RuntimeCall,
	>;
	type Size = u32;
	// The XCMP queue pallet is only ever able to handle the `Sibling(ParaId)` origin:
	type QueueChangeHandler = NarrowOriginToSibling<XcmpQueue>;
	type QueuePausedQuery = NarrowOriginToSibling<XcmpQueue>;
	type HeapSize = sp_core::ConstU32<{ 103 * 1024 }>;
	type MaxStale = sp_core::ConstU32<8>;
	type ServiceWeight = MessageQueueServiceWeight;
	type IdleMaxServiceWeight = MessageQueueServiceWeight;
}

impl cumulus_pallet_aura_ext::Config for Runtime {}

parameter_types! {
	/// The asset ID for the asset that we use to pay for message delivery fees.
	pub FeeAssetId: AssetId = AssetId(xcm_config::WestendLocation::get());
	/// The base fee for the message delivery fees.
	pub const BaseDeliveryFee: u128 = CENTS.saturating_mul(3);
}

pub type PriceForSiblingParachainDelivery = polkadot_runtime_common::xcm_sender::ExponentialPrice<
	FeeAssetId,
	BaseDeliveryFee,
	TransactionByteFee,
	XcmpQueue,
>;

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = PolkadotXcm;
	// Enqueue XCMP messages from siblings for later processing.
	type XcmpQueue = TransformOrigin<MessageQueue, AggregateMessageOrigin, ParaId, ParaIdToSibling>;
	type MaxInboundSuspended = ConstU32<1_000>;
	type MaxActiveOutboundChannels = ConstU32<128>;
	// Most on-chain HRMP channels are configured to use 102400 bytes of max message size, so we
	// need to set the page size larger than that until we reduce the channel size on-chain.
	type MaxPageSize = ConstU32<{ 103 * 1024 }>;
	type ControllerOrigin = EnsureRoot<AccountId>;
	type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
	type WeightInfo = weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
	type PriceForSiblingDelivery = PriceForSiblingParachainDelivery;
}

impl cumulus_pallet_xcmp_queue::migration::v5::V5Config for Runtime {
	// This must be the same as the `ChannelInfo` from the `Config`:
	type ChannelList = ParachainSystem;
}

parameter_types! {
	pub const RelayOrigin: AggregateMessageOrigin = AggregateMessageOrigin::Parent;
}

parameter_types! {
	pub const Period: u32 = 6 * HOURS;
	pub const Offset: u32 = 0;
}

impl pallet_session::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	// we don't have stash and controller, thus we don't need the convert as well.
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
	type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
	type SessionManager = CollatorSelection;
	// Essentially just Aura, but let's be pedantic.
	type SessionHandler = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;
	type Keys = SessionKeys;
	type DisablingStrategy = ();
	type WeightInfo = weights::pallet_session::WeightInfo<Runtime>;
	type Currency = Balances;
	type KeyDeposit = ();
}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<100_000>;
	type AllowMultipleBlocksPerSlot = ConstBool<true>;
	type SlotDuration = ConstU64<SLOT_DURATION>;
}

parameter_types! {
	pub const PotId: PalletId = PalletId(*b"PotStake");
	pub const SessionLength: BlockNumber = 6 * HOURS;
}

pub type CollatorSelectionUpdateOrigin = EnsureRoot<AccountId>;

impl pallet_collator_selection::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type UpdateOrigin = CollatorSelectionUpdateOrigin;
	type PotId = PotId;
	type MaxCandidates = ConstU32<100>;
	type MinEligibleCollators = ConstU32<4>;
	type MaxInvulnerables = ConstU32<20>;
	// should be a multiple of session or things will get inconsistent
	type KickThreshold = Period;
	type ValidatorId = <Self as frame_system::Config>::AccountId;
	type ValidatorIdOf = pallet_collator_selection::IdentityCollator;
	type ValidatorRegistration = Session;
	type WeightInfo = weights::pallet_collator_selection::WeightInfo<Runtime>;
}

parameter_types! {
	pub StakingPot: AccountId = CollatorSelection::account_id();
}

impl pallet_asset_conversion_tx_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AssetId = xcm::v5::Location;
	type OnChargeAssetTransaction = SwapAssetAdapter<
		WestendLocation,
		NativeAndNonPoolAssets,
		AssetConversion,
		ResolveAssetTo<StakingPot, NativeAndNonPoolAssets>,
	>;
	type WeightInfo = weights::pallet_asset_conversion_tx_payment::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = AssetConversionTxHelper;
}

parameter_types! {
	pub const UniquesCollectionDeposit: Balance = UNITS / 10; // 1 / 10 UNIT deposit to create a collection
	pub const UniquesItemDeposit: Balance = UNITS / 1_000; // 1 / 1000 UNIT deposit to mint an item
	pub const UniquesMetadataDepositBase: Balance = deposit(1, 129);
	pub const UniquesAttributeDepositBase: Balance = deposit(1, 0);
	pub const UniquesDepositPerByte: Balance = deposit(0, 1);
}

impl pallet_uniques::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = CollectionId;
	type ItemId = ItemId;
	type Currency = Balances;
	type ForceOrigin = AssetsForceOrigin;
	type CollectionDeposit = UniquesCollectionDeposit;
	type ItemDeposit = UniquesItemDeposit;
	type MetadataDepositBase = UniquesMetadataDepositBase;
	type AttributeDepositBase = UniquesAttributeDepositBase;
	type DepositPerByte = UniquesDepositPerByte;
	type StringLimit = ConstU32<128>;
	type KeyLimit = ConstU32<32>;
	type ValueLimit = ConstU32<64>;
	type WeightInfo = weights::pallet_uniques::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type Locker = ();
}

parameter_types! {
	pub const NftFractionalizationPalletId: PalletId = PalletId(*b"fraction");
	pub NewAssetSymbol: BoundedVec<u8, AssetsStringLimit> = (*b"FRAC").to_vec().try_into().unwrap();
	pub NewAssetName: BoundedVec<u8, AssetsStringLimit> = (*b"Frac").to_vec().try_into().unwrap();
}

impl pallet_nft_fractionalization::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Deposit = AssetDeposit;
	type Currency = Balances;
	type NewAssetSymbol = NewAssetSymbol;
	type NewAssetName = NewAssetName;
	type StringLimit = AssetsStringLimit;
	type NftCollectionId = <Self as pallet_nfts::Config>::CollectionId;
	type NftId = <Self as pallet_nfts::Config>::ItemId;
	type AssetBalance = <Self as pallet_balances::Config>::Balance;
	type AssetId = <Self as pallet_assets::Config<TrustBackedAssetsInstance>>::AssetId;
	type Assets = Assets;
	type Nfts = Nfts;
	type PalletId = NftFractionalizationPalletId;
	type WeightInfo = weights::pallet_nft_fractionalization::WeightInfo<Runtime>;
	type RuntimeHoldReason = RuntimeHoldReason;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
}

parameter_types! {
	pub NftsPalletFeatures: PalletFeatures = PalletFeatures::all_enabled();
	pub const NftsMaxDeadlineDuration: BlockNumber = 12 * 30 * DAYS;
	// re-use the Uniques deposits
	pub const NftsCollectionDeposit: Balance = UniquesCollectionDeposit::get();
	pub const NftsItemDeposit: Balance = UniquesItemDeposit::get();
	pub const NftsMetadataDepositBase: Balance = UniquesMetadataDepositBase::get();
	pub const NftsAttributeDepositBase: Balance = UniquesAttributeDepositBase::get();
	pub const NftsDepositPerByte: Balance = UniquesDepositPerByte::get();
}

impl pallet_nfts::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = CollectionId;
	type ItemId = ItemId;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type ForceOrigin = AssetsForceOrigin;
	type Locker = ();
	type CollectionDeposit = NftsCollectionDeposit;
	type ItemDeposit = NftsItemDeposit;
	type MetadataDepositBase = NftsMetadataDepositBase;
	type AttributeDepositBase = NftsAttributeDepositBase;
	type DepositPerByte = NftsDepositPerByte;
	type StringLimit = ConstU32<256>;
	type KeyLimit = ConstU32<64>;
	type ValueLimit = ConstU32<256>;
	type ApprovalsLimit = ConstU32<20>;
	type ItemAttributesApprovalsLimit = ConstU32<30>;
	type MaxTips = ConstU32<10>;
	type MaxDeadlineDuration = NftsMaxDeadlineDuration;
	type MaxAttributesPerCall = ConstU32<10>;
	type Features = NftsPalletFeatures;
	type OffchainSignature = Signature;
	type OffchainPublic = <Signature as Verify>::Signer;
	type WeightInfo = weights::pallet_nfts::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
	type BlockNumberProvider = RelaychainDataProvider<Runtime>;
}

/// XCM router instance to BridgeHub with bridging capabilities for `Rococo` global
/// consensus with dynamic fees and back-pressure.
pub type ToRococoXcmRouterInstance = pallet_xcm_bridge_hub_router::Instance1;
impl pallet_xcm_bridge_hub_router::Config<ToRococoXcmRouterInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_xcm_bridge_hub_router::WeightInfo<Runtime>;

	type UniversalLocation = xcm_config::UniversalLocation;
	type SiblingBridgeHubLocation = xcm_config::bridging::SiblingBridgeHub;
	type BridgedNetworkId = xcm_config::bridging::to_rococo::RococoNetwork;
	type Bridges = xcm_config::bridging::NetworkExportTable;
	type DestinationVersion = PolkadotXcm;

	type BridgeHubOrigin = frame_support::traits::EitherOfDiverse<
		EnsureRoot<AccountId>,
		EnsureXcm<Equals<Self::SiblingBridgeHubLocation>>,
	>;
	type ToBridgeHubSender = XcmpQueue;
	type LocalXcmChannelManager =
		cumulus_pallet_xcmp_queue::bridging::InAndOutXcmpChannelStatusProvider<Runtime>;

	type ByteFee = xcm_config::bridging::XcmBridgeHubRouterByteFee;
	type FeeAsset = xcm_config::bridging::XcmBridgeHubRouterFeeAssetId;
}

parameter_types! {
	pub const DepositPerItem: Balance = deposit(1, 0);
	pub const DepositPerByte: Balance = deposit(0, 1);
	pub CodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
}

impl pallet_revive::Config for Runtime {
	type Time = Timestamp;
	type Currency = Balances;
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type DepositPerItem = DepositPerItem;
	type DepositPerByte = DepositPerByte;
	type WeightPrice = pallet_transaction_payment::Pallet<Self>;
	type WeightInfo = pallet_revive::weights::SubstrateWeight<Self>;
	type Precompiles = (
		ERC20<Self, InlineIdConfig<0x120>, TrustBackedAssetsInstance>,
		ERC20<Self, InlineIdConfig<0x320>, PoolAssetsInstance>,
		XcmPrecompile<Self>,
	);
	type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
	type RuntimeMemory = ConstU32<{ 128 * 1024 * 1024 }>;
	type PVFMemory = ConstU32<{ 512 * 1024 * 1024 }>;
	type UnsafeUnstableInterface = ConstBool<false>;
	type UploadOrigin = EnsureSigned<Self::AccountId>;
	type InstantiateOrigin = EnsureSigned<Self::AccountId>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type CodeHashLockupDepositPercent = CodeHashLockupDepositPercent;
	type ChainId = ConstU64<420_420_421>;
	type NativeToEthRatio = ConstU32<1_000_000>; // 10^(18 - 12) Eth is 10^18, Native is 10^12.
	type EthGasEncoder = ();
	type FindAuthor = <Runtime as pallet_authorship::Config>::FindAuthor;
}

parameter_types! {
	pub MbmServiceWeight: Weight = Perbill::from_percent(80) * RuntimeBlockWeights::get().max_block;
}

impl pallet_migrations::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type Migrations = pallet_revive::migrations::v1::Migration<Runtime>;
	// Benchmarks need mocked migrations to guarantee that they succeed.
	#[cfg(feature = "runtime-benchmarks")]
	type Migrations = pallet_migrations::mock_helpers::MockedMigrations;
	type CursorMaxLen = ConstU32<65_536>;
	type IdentifierMaxLen = ConstU32<256>;
	type MigrationStatusHandler = ();
	type FailedMigrationHandler = frame_support::migrations::FreezeChainOnFailedMigration;
	type MaxServiceWeight = MbmServiceWeight;
	type WeightInfo = weights::pallet_migrations::WeightInfo<Runtime>;
}

parameter_types! {
	pub MaximumSchedulerWeight: frame_support::weights::Weight = Perbill::from_percent(80) *
		RuntimeBlockWeights::get().max_block;
}

impl pallet_scheduler::Config for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type PalletsOrigin = OriginCaller;
	type RuntimeCall = RuntimeCall;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = EnsureRoot<AccountId>;
	#[cfg(feature = "runtime-benchmarks")]
	type MaxScheduledPerBlock = ConstU32<{ 512 * 15 }>; // MaxVotes * TRACKS_DATA length
	#[cfg(not(feature = "runtime-benchmarks"))]
	type MaxScheduledPerBlock = ConstU32<50>;
	type WeightInfo = weights::pallet_scheduler::WeightInfo<Runtime>;
	type OriginPrivilegeCmp = frame_support::traits::EqualPrivilegeOnly;
	type Preimages = Preimage;
	type BlockNumberProvider = RelaychainDataProvider<Runtime>;
}

parameter_types! {
	pub const PreimageBaseDeposit: Balance = deposit(2, 64);
	pub const PreimageByteDeposit: Balance = deposit(0, 1);
	pub const PreimageHoldReason: RuntimeHoldReason = RuntimeHoldReason::Preimage(pallet_preimage::HoldReason::Preimage);
}

impl pallet_preimage::Config for Runtime {
	type WeightInfo = weights::pallet_preimage::WeightInfo<Runtime>;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type ManagerOrigin = EnsureRoot<AccountId>;
	type Consideration = HoldConsideration<
		AccountId,
		Balances,
		PreimageHoldReason,
		LinearStoragePrice<PreimageBaseDeposit, PreimageByteDeposit, Balance>,
	>;
}

parameter_types! {
	/// Deposit for an index in the indices pallet.
	///
	/// 32 bytes for the account ID and 16 for the deposit. We cannot use `max_encoded_len` since it
	/// is not const.
	pub const IndexDeposit: Balance = deposit(1, 32 + 16);
}

impl pallet_indices::Config for Runtime {
	type AccountIndex = AccountIndex;
	type Currency = Balances;
	type Deposit = IndexDeposit;
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_indices::WeightInfo<Runtime>;
}

impl pallet_ah_ops::Config for Runtime {
	type Currency = Balances;
	type RcBlockNumberProvider = RelaychainDataProvider<Runtime>;
	type WeightInfo = weights::pallet_ah_ops::WeightInfo<Runtime>;
}

impl pallet_sudo::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type WeightInfo = weights::pallet_sudo::WeightInfo<Runtime>;
}

// Create the runtime by composing the FRAME pallets that were previously configured.
construct_runtime!(
	pub enum Runtime
	{
		// System support stuff.
		System: frame_system = 0,
		ParachainSystem: cumulus_pallet_parachain_system = 1,
		// RandomnessCollectiveFlip = 2 removed
		Timestamp: pallet_timestamp = 3,
		ParachainInfo: parachain_info = 4,
		WeightReclaim: cumulus_pallet_weight_reclaim = 5,
		MultiBlockMigrations: pallet_migrations = 6,
		Preimage: pallet_preimage = 7,
		Scheduler: pallet_scheduler = 8,
		Sudo: pallet_sudo = 9,

		// Monetary stuff.
		Balances: pallet_balances = 10,
		TransactionPayment: pallet_transaction_payment = 11,
		// AssetTxPayment: pallet_asset_tx_payment = 12,
		AssetTxPayment: pallet_asset_conversion_tx_payment = 13,
		Vesting: pallet_vesting = 14,

		// Collator support. the order of these 5 are important and shall not change.
		Authorship: pallet_authorship = 20,
		CollatorSelection: pallet_collator_selection = 21,
		Session: pallet_session = 22,
		Aura: pallet_aura = 23,
		AuraExt: cumulus_pallet_aura_ext = 24,

		// XCM helpers.
		XcmpQueue: cumulus_pallet_xcmp_queue = 30,
		PolkadotXcm: pallet_xcm = 31,
		CumulusXcm: cumulus_pallet_xcm = 32,
		// Bridge utilities.
		ToRococoXcmRouter: pallet_xcm_bridge_hub_router::<Instance1> = 34,
		MessageQueue: pallet_message_queue = 35,
		// Snowbridge
		SnowbridgeSystemFrontend: snowbridge_pallet_system_frontend = 36,

		// Handy utilities.
		Utility: pallet_utility = 40,
		Multisig: pallet_multisig = 41,
		Proxy: pallet_proxy = 42,
		Indices: pallet_indices = 43,

		// The main stage.
		Assets: pallet_assets::<Instance1> = 50,
		Uniques: pallet_uniques = 51,
		Nfts: pallet_nfts = 52,
		ForeignAssets: pallet_assets::<Instance2> = 53,
		NftFractionalization: pallet_nft_fractionalization = 54,
		PoolAssets: pallet_assets::<Instance3> = 55,
		AssetConversion: pallet_asset_conversion = 56,

		AssetsFreezer: pallet_assets_freezer::<Instance1> = 57,
		ForeignAssetsFreezer: pallet_assets_freezer::<Instance2> = 58,
		PoolAssetsFreezer: pallet_assets_freezer::<Instance3> = 59,
		Revive: pallet_revive = 60,

		AssetRewards: pallet_asset_rewards = 61,

		StateTrieMigration: pallet_state_trie_migration = 70,

		// Staking.
		Staking: pallet_staking_async = 80,
		NominationPools: pallet_nomination_pools = 81,
		FastUnstake: pallet_fast_unstake = 82,
		VoterList: pallet_bags_list::<Instance1> = 83,
		DelegatedStaking: pallet_delegated_staking = 84,
		StakingRcClient: pallet_staking_async_rc_client = 89,

		// Staking election apparatus.
		MultiBlockElection: pallet_election_provider_multi_block = 85,
		MultiBlockElectionVerifier: pallet_election_provider_multi_block::verifier = 86,
		MultiBlockElectionUnsigned: pallet_election_provider_multi_block::unsigned = 87,
		MultiBlockElectionSigned: pallet_election_provider_multi_block::signed = 88,

		// Governance.
		ConvictionVoting: pallet_conviction_voting = 90,
		Referenda: pallet_referenda = 91,
		Origins: pallet_custom_origins = 92,
		Whitelist: pallet_whitelist = 93,
		Treasury: pallet_treasury = 94,
		AssetRate: pallet_asset_rate = 95,

		// TODO: the pallet instance should be removed once all pools have migrated
		// to the new account IDs.
		AssetConversionMigration: pallet_asset_conversion_ops = 200,

		AhOps: pallet_ah_ops = 254,
	}
);

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The extension to the basic transaction logic.
pub type TxExtension = cumulus_pallet_weight_reclaim::StorageWeightReclaim<
	Runtime,
	(
		frame_system::AuthorizeCall<Runtime>,
		frame_system::CheckNonZeroSender<Runtime>,
		frame_system::CheckSpecVersion<Runtime>,
		frame_system::CheckTxVersion<Runtime>,
		frame_system::CheckGenesis<Runtime>,
		frame_system::CheckEra<Runtime>,
		frame_system::CheckNonce<Runtime>,
		frame_system::CheckWeight<Runtime>,
		pallet_asset_conversion_tx_payment::ChargeAssetTxPayment<Runtime>,
		frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
	),
>;

/// Default extensions applied to Ethereum transactions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EthExtraImpl;

impl EthExtra for EthExtraImpl {
	type Config = Runtime;
	type Extension = TxExtension;

	fn get_eth_extension(nonce: u32, tip: Balance) -> Self::Extension {
		(
			frame_system::AuthorizeCall::<Runtime>::new(),
			frame_system::CheckNonZeroSender::<Runtime>::new(),
			frame_system::CheckSpecVersion::<Runtime>::new(),
			frame_system::CheckTxVersion::<Runtime>::new(),
			frame_system::CheckGenesis::<Runtime>::new(),
			frame_system::CheckMortality::from(generic::Era::Immortal),
			frame_system::CheckNonce::<Runtime>::from(nonce),
			frame_system::CheckWeight::<Runtime>::new(),
			pallet_asset_conversion_tx_payment::ChargeAssetTxPayment::<Runtime>::from(tip, None),
			frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
		)
			.into()
	}
}

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	pallet_revive::evm::runtime::UncheckedExtrinsic<Address, Signature, EthExtraImpl>;

/// Migrations to apply on runtime upgrade.
pub type Migrations = (
	// v9420
	pallet_nfts::migration::v1::MigrateToV1<Runtime>,
	// unreleased
	pallet_collator_selection::migration::v2::MigrationToV2<Runtime>,
	// unreleased
	pallet_multisig::migrations::v1::MigrateToV1<Runtime>,
	// unreleased
	InitStorageVersions,
	// unreleased
	DeleteUndecodableStorage,
	// unreleased
	cumulus_pallet_xcmp_queue::migration::v4::MigrationToV4<Runtime>,
	cumulus_pallet_xcmp_queue::migration::v5::MigrateV4ToV5<Runtime>,
	// unreleased
	pallet_assets::migration::next_asset_id::SetNextAssetId<
		ConstU32<50_000_000>,
		Runtime,
		TrustBackedAssetsInstance,
	>,
	pallet_session::migrations::v1::MigrateV0ToV1<
		Runtime,
		pallet_session::migrations::v1::InitOffenceSeverity<Runtime>,
	>,
	// permanent
	pallet_xcm::migration::MigrateToLatestXcmVersion<Runtime>,
	cumulus_pallet_aura_ext::migration::MigrateV0ToV1<Runtime>,
);

/// Asset Hub Westend has some undecodable storage, delete it.
/// See <https://github.com/paritytech/polkadot-sdk/issues/2241> for more info.
///
/// First we remove the bad Hold, then the bad NFT collection.
pub struct DeleteUndecodableStorage;

impl frame_support::traits::OnRuntimeUpgrade for DeleteUndecodableStorage {
	fn on_runtime_upgrade() -> Weight {
		use sp_core::crypto::Ss58Codec;

		let mut writes = 0;

		// Remove Holds for account with undecodable hold
		// Westend doesn't have any HoldReasons implemented yet, so it's safe to just blanket remove
		// any for this account.
		match AccountId::from_ss58check("5GCCJthVSwNXRpbeg44gysJUx9vzjdGdfWhioeM7gCg6VyXf") {
			Ok(a) => {
				tracing::info!(target: "bridges::on_runtime_upgrade", "Removing holds for account with bad hold");
				pallet_balances::Holds::<Runtime, ()>::remove(a);
				writes.saturating_inc();
			},
			Err(_) => {
				tracing::error!(target: "bridges::on_runtime_upgrade", "CleanupUndecodableStorage: Somehow failed to convert valid SS58 address into an AccountId!");
			},
		};

		// Destroy undecodable NFT item 1
		writes.saturating_inc();
		match pallet_nfts::Pallet::<Runtime, ()>::do_burn(3, 1, |_| Ok(())) {
			Ok(_) => {
				tracing::info!(target: "bridges::on_runtime_upgrade", "Destroyed undecodable NFT item 1");
			},
			Err(e) => {
				tracing::error!(target: "bridges::on_runtime_upgrade", error=?e, "Failed to destroy undecodable NFT item");
				return <Runtime as frame_system::Config>::DbWeight::get().reads_writes(0, writes);
			},
		}

		// Destroy undecodable NFT item 2
		writes.saturating_inc();
		match pallet_nfts::Pallet::<Runtime, ()>::do_burn(3, 2, |_| Ok(())) {
			Ok(_) => {
				tracing::info!(target: "bridges::on_runtime_upgrade", "Destroyed undecodable NFT item 2");
			},
			Err(e) => {
				tracing::error!(target: "bridges::on_runtime_upgrade", error=?e, "Failed to destroy undecodable NFT item");
				return <Runtime as frame_system::Config>::DbWeight::get().reads_writes(0, writes);
			},
		}

		// Finally, we can destroy the collection
		writes.saturating_inc();
		match pallet_nfts::Pallet::<Runtime, ()>::do_destroy_collection(
			3,
			DestroyWitness { attributes: 0, item_metadatas: 1, item_configs: 0 },
			None,
		) {
			Ok(_) => {
				tracing::info!(target: "bridges::on_runtime_upgrade", "Destroyed undecodable NFT collection");
			},
			Err(e) => {
				tracing::error!(target: "bridges::on_runtime_upgrade", error=?e, "Failed to destroy undecodable NFT collection");
			},
		};

		<Runtime as frame_system::Config>::DbWeight::get().reads_writes(0, writes)
	}
}

/// Migration to initialize storage versions for pallets added after genesis.
///
/// Ideally this would be done automatically (see
/// <https://github.com/paritytech/polkadot-sdk/pull/1297>), but it probably won't be ready for some
/// time and it's beneficial to get try-runtime-cli on-runtime-upgrade checks into the CI, so we're
/// doing it manually.
pub struct InitStorageVersions;

impl frame_support::traits::OnRuntimeUpgrade for InitStorageVersions {
	fn on_runtime_upgrade() -> Weight {
		use frame_support::traits::{GetStorageVersion, StorageVersion};

		let mut writes = 0;

		if PolkadotXcm::on_chain_storage_version() == StorageVersion::new(0) {
			PolkadotXcm::in_code_storage_version().put::<PolkadotXcm>();
			writes.saturating_inc();
		}

		if ForeignAssets::on_chain_storage_version() == StorageVersion::new(0) {
			ForeignAssets::in_code_storage_version().put::<ForeignAssets>();
			writes.saturating_inc();
		}

		if PoolAssets::on_chain_storage_version() == StorageVersion::new(0) {
			PoolAssets::in_code_storage_version().put::<PoolAssets>();
			writes.saturating_inc();
		}

		<Runtime as frame_system::Config>::DbWeight::get().reads_writes(3, writes)
	}
}

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	Migrations,
>;

#[cfg(feature = "runtime-benchmarks")]
pub struct AssetConversionTxHelper;

#[cfg(feature = "runtime-benchmarks")]
impl
	pallet_asset_conversion_tx_payment::BenchmarkHelperTrait<
		AccountId,
		cumulus_primitives_core::Location,
		cumulus_primitives_core::Location,
	> for AssetConversionTxHelper
{
	fn create_asset_id_parameter(
		seed: u32,
	) -> (cumulus_primitives_core::Location, cumulus_primitives_core::Location) {
		// Use a different parachain' foreign assets pallet so that the asset is indeed foreign.
		let asset_id = cumulus_primitives_core::Location::new(
			1,
			[
				cumulus_primitives_core::Junction::Parachain(3000),
				cumulus_primitives_core::Junction::PalletInstance(53),
				cumulus_primitives_core::Junction::GeneralIndex(seed.into()),
			],
		);
		(asset_id.clone(), asset_id)
	}

	fn setup_balances_and_pool(asset_id: cumulus_primitives_core::Location, account: AccountId) {
		use frame_support::{assert_ok, traits::fungibles::Mutate};
		assert_ok!(ForeignAssets::force_create(
			RuntimeOrigin::root(),
			asset_id.clone().into(),
			account.clone().into(), /* owner */
			true,                   /* is_sufficient */
			1,
		));

		let lp_provider = account.clone();
		use frame_support::traits::Currency;
		let _ = Balances::deposit_creating(&lp_provider, u64::MAX.into());
		assert_ok!(ForeignAssets::mint_into(
			asset_id.clone().into(),
			&lp_provider,
			u64::MAX.into()
		));

		let token_native = alloc::boxed::Box::new(cumulus_primitives_core::Location::new(
			1,
			cumulus_primitives_core::Junctions::Here,
		));
		let token_second = alloc::boxed::Box::new(asset_id);

		assert_ok!(AssetConversion::create_pool(
			RuntimeOrigin::signed(lp_provider.clone()),
			token_native.clone(),
			token_second.clone()
		));

		assert_ok!(AssetConversion::add_liquidity(
			RuntimeOrigin::signed(lp_provider.clone()),
			token_native,
			token_second,
			(u32::MAX / 2).into(), // 1 desired
			u32::MAX.into(),       // 2 desired
			1,                     // 1 min
			1,                     // 2 min
			lp_provider,
		));
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	frame_benchmarking::define_benchmarks!(
		[frame_system, SystemBench::<Runtime>]
		[frame_system_extensions, SystemExtensionsBench::<Runtime>]
		[pallet_asset_conversion_ops, AssetConversionMigration]
		[pallet_asset_rate, AssetRate]
		[pallet_assets, Local]
		[pallet_assets, Foreign]
		[pallet_assets, Pool]
		[pallet_asset_conversion, AssetConversion]
		[pallet_asset_rewards, AssetRewards]
		[pallet_asset_conversion_tx_payment, AssetTxPayment]
		[pallet_bags_list, VoterList]
		[pallet_balances, Balances]
		[pallet_conviction_voting, ConvictionVoting]
		// Temporarily disabled due to https://github.com/paritytech/polkadot-sdk/issues/7714
		// [pallet_election_provider_multi_block, MultiBlockElection]
		// [pallet_election_provider_multi_block_verifier, MultiBlockElectionVerifier]
		[pallet_election_provider_multi_block_unsigned, MultiBlockElectionUnsigned]
		[pallet_election_provider_multi_block_signed, MultiBlockElectionSigned]
		[pallet_fast_unstake, FastUnstake]
		[pallet_message_queue, MessageQueue]
		[pallet_migrations, MultiBlockMigrations]
		[pallet_multisig, Multisig]
		[pallet_nft_fractionalization, NftFractionalization]
		[pallet_nfts, Nfts]
		[pallet_proxy, Proxy]
		[pallet_session, SessionBench::<Runtime>]
		[pallet_staking_async, Staking]
		[pallet_uniques, Uniques]
		[pallet_utility, Utility]
		[pallet_timestamp, Timestamp]
		[pallet_transaction_payment, TransactionPayment]
		[pallet_collator_selection, CollatorSelection]
		[cumulus_pallet_parachain_system, ParachainSystem]
		[cumulus_pallet_xcmp_queue, XcmpQueue]
		[pallet_treasury, Treasury]
		[pallet_vesting, Vesting]
		[pallet_whitelist, Whitelist]
		[pallet_xcm_bridge_hub_router, ToRococo]
		[pallet_asset_conversion_ops, AssetConversionMigration]
		[pallet_revive, Revive]
		// XCM
		[pallet_xcm, PalletXcmExtrinsicsBenchmark::<Runtime>]
		// NOTE: Make sure you point to the individual modules below.
		[pallet_xcm_benchmarks::fungible, XcmBalances]
		[pallet_xcm_benchmarks::generic, XcmGeneric]
		[cumulus_pallet_weight_reclaim, WeightReclaim]
		[snowbridge_pallet_system_frontend, SnowbridgeSystemFrontend]
	);
}

pallet_revive::impl_runtime_apis_plus_revive!(
	Runtime,
	Executive,
	EthExtraImpl,

	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(SLOT_DURATION)
		}

		fn authorities() -> Vec<AuraId> {
			pallet_aura::Authorities::<Runtime>::get().into_inner()
		}
	}

	impl cumulus_primitives_core::RelayParentOffsetApi<Block> for Runtime {
		fn relay_parent_offset() -> u32 {
			0
		}
	}

	impl cumulus_primitives_core::GetParachainInfo<Block> for Runtime {
		fn parachain_id() -> ParaId {
			ParachainInfo::parachain_id()
		}
	}

	impl cumulus_primitives_aura::AuraUnincludedSegmentApi<Block> for Runtime {
		fn can_build_upon(
			included_hash: <Block as BlockT>::Hash,
			slot: cumulus_primitives_aura::Slot,
		) -> bool {
			ConsensusHook::can_build_upon(included_hash, slot)
		}
	}

	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> alloc::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_nfts_runtime_api::NftsApi<Block, AccountId, u32, u32> for Runtime {
		fn owner(collection: u32, item: u32) -> Option<AccountId> {
			<Nfts as Inspect<AccountId>>::owner(&collection, &item)
		}

		fn collection_owner(collection: u32) -> Option<AccountId> {
			<Nfts as Inspect<AccountId>>::collection_owner(&collection)
		}

		fn attribute(
			collection: u32,
			item: u32,
			key: Vec<u8>,
		) -> Option<Vec<u8>> {
			<Nfts as Inspect<AccountId>>::attribute(&collection, &item, &key)
		}

		fn custom_attribute(
			account: AccountId,
			collection: u32,
			item: u32,
			key: Vec<u8>,
		) -> Option<Vec<u8>> {
			<Nfts as Inspect<AccountId>>::custom_attribute(
				&account,
				&collection,
				&item,
				&key,
			)
		}

		fn system_attribute(
			collection: u32,
			item: Option<u32>,
			key: Vec<u8>,
		) -> Option<Vec<u8>> {
			<Nfts as Inspect<AccountId>>::system_attribute(&collection, item.as_ref(), &key)
		}

		fn collection_attribute(collection: u32, key: Vec<u8>) -> Option<Vec<u8>> {
			<Nfts as Inspect<AccountId>>::collection_attribute(&collection, &key)
		}
	}

	impl pallet_asset_conversion::AssetConversionApi<
		Block,
		Balance,
		xcm::v5::Location,
	> for Runtime
	{
		fn quote_price_exact_tokens_for_tokens(asset1: xcm::v5::Location, asset2: xcm::v5::Location, amount: Balance, include_fee: bool) -> Option<Balance> {
			AssetConversion::quote_price_exact_tokens_for_tokens(asset1, asset2, amount, include_fee)
		}

		fn quote_price_tokens_for_exact_tokens(asset1: xcm::v5::Location, asset2: xcm::v5::Location, amount: Balance, include_fee: bool) -> Option<Balance> {
			AssetConversion::quote_price_tokens_for_exact_tokens(asset1, asset2, amount, include_fee)
		}

		fn get_reserves(asset1: xcm::v5::Location, asset2: xcm::v5::Location) -> Option<(Balance, Balance)> {
			AssetConversion::get_reserves(asset1, asset2).ok()
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl xcm_runtime_apis::fees::XcmPaymentApi<Block> for Runtime {
		fn query_acceptable_payment_assets(xcm_version: xcm::Version) -> Result<Vec<VersionedAssetId>, XcmPaymentApiError> {
			let native_token = xcm_config::WestendLocation::get();
			// We accept the native token to pay fees.
			let mut acceptable_assets = vec![AssetId(native_token.clone())];
			// We also accept all assets in a pool with the native token.
			acceptable_assets.extend(
				assets_common::PoolAdapter::<Runtime>::get_assets_in_pool_with(native_token)
				.map_err(|()| XcmPaymentApiError::VersionedConversionFailed)?
			);
			PolkadotXcm::query_acceptable_payment_assets(xcm_version, acceptable_assets)
		}

		fn query_weight_to_asset_fee(weight: Weight, asset: VersionedAssetId) -> Result<u128, XcmPaymentApiError> {
			use crate::xcm_config::XcmConfig;

			type Trader = <XcmConfig as xcm_executor::Config>::Trader;

			PolkadotXcm::query_weight_to_asset_fee::<Trader>(weight, asset)
		}

		fn query_xcm_weight(message: VersionedXcm<()>) -> Result<Weight, XcmPaymentApiError> {
			PolkadotXcm::query_xcm_weight(message)
		}

		fn query_delivery_fees(destination: VersionedLocation, message: VersionedXcm<()>) -> Result<VersionedAssets, XcmPaymentApiError> {
			PolkadotXcm::query_delivery_fees(destination, message)
		}
	}

	impl xcm_runtime_apis::dry_run::DryRunApi<Block, RuntimeCall, RuntimeEvent, OriginCaller> for Runtime {
		fn dry_run_call(origin: OriginCaller, call: RuntimeCall, result_xcms_version: XcmVersion) -> Result<CallDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
			PolkadotXcm::dry_run_call::<Runtime, xcm_config::XcmRouter, OriginCaller, RuntimeCall>(origin, call, result_xcms_version)
		}

		fn dry_run_xcm(origin_location: VersionedLocation, xcm: VersionedXcm<RuntimeCall>) -> Result<XcmDryRunEffects<RuntimeEvent>, XcmDryRunApiError> {
			PolkadotXcm::dry_run_xcm::<Runtime, xcm_config::XcmRouter, RuntimeCall, xcm_config::XcmConfig>(origin_location, xcm)
		}
	}

	impl xcm_runtime_apis::conversions::LocationToAccountApi<Block, AccountId> for Runtime {
		fn convert_location(location: VersionedLocation) -> Result<
			AccountId,
			xcm_runtime_apis::conversions::Error
		> {
			xcm_runtime_apis::conversions::LocationToAccountHelper::<
				AccountId,
				xcm_config::LocationToAccountId,
			>::convert_location(location)
		}
	}

	impl xcm_runtime_apis::trusted_query::TrustedQueryApi<Block> for Runtime {
		fn is_trusted_reserve(asset: VersionedAsset, location: VersionedLocation) -> xcm_runtime_apis::trusted_query::XcmTrustedQueryResult {
			PolkadotXcm::is_trusted_reserve(asset, location)
		}
		fn is_trusted_teleporter(asset: VersionedAsset, location: VersionedLocation) -> xcm_runtime_apis::trusted_query::XcmTrustedQueryResult {
			PolkadotXcm::is_trusted_teleporter(asset, location)
		}
	}

	impl xcm_runtime_apis::authorized_aliases::AuthorizedAliasersApi<Block> for Runtime {
		fn authorized_aliasers(target: VersionedLocation) -> Result<
			Vec<xcm_runtime_apis::authorized_aliases::OriginAliaser>,
			xcm_runtime_apis::authorized_aliases::Error
		> {
			PolkadotXcm::authorized_aliasers(target)
		}
		fn is_authorized_alias(origin: VersionedLocation, target: VersionedLocation) -> Result<
			bool,
			xcm_runtime_apis::authorized_aliases::Error
		> {
			PolkadotXcm::is_authorized_alias(origin, target)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl assets_common::runtime_api::FungiblesApi<
		Block,
		AccountId,
	> for Runtime
	{
		fn query_account_balances(account: AccountId) -> Result<xcm::VersionedAssets, assets_common::runtime_api::FungiblesAccessError> {
			use assets_common::fungible_conversion::{convert, convert_balance};
			Ok([
				// collect pallet_balance
				{
					let balance = Balances::free_balance(account.clone());
					if balance > 0 {
						vec![convert_balance::<WestendLocation, Balance>(balance)?]
					} else {
						vec![]
					}
				},
				// collect pallet_assets (TrustBackedAssets)
				convert::<_, _, _, _, TrustBackedAssetsConvertedConcreteId>(
					Assets::account_balances(account.clone())
						.iter()
						.filter(|(_, balance)| balance > &0)
				)?,
				// collect pallet_assets (ForeignAssets)
				convert::<_, _, _, _, ForeignAssetsConvertedConcreteId>(
					ForeignAssets::account_balances(account.clone())
						.iter()
						.filter(|(_, balance)| balance > &0)
				)?,
				// collect pallet_assets (PoolAssets)
				convert::<_, _, _, _, PoolAssetsConvertedConcreteId>(
					PoolAssets::account_balances(account)
						.iter()
						.filter(|(_, balance)| balance > &0)
				)?,
				// collect ... e.g. other tokens
			].concat().into())
		}
	}

	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
			ParachainSystem::collect_collation_info(header)
		}
	}

	impl pallet_asset_rewards::AssetRewards<Block, Balance> for Runtime {
		fn pool_creation_cost() -> Balance {
			StakePoolCreationDeposit::get()
		}
	}

	impl cumulus_primitives_core::GetCoreSelectorApi<Block> for Runtime {
		fn core_selector() -> (CoreSelector, ClaimQueueOffset) {
			ParachainSystem::core_selector()
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect,
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
		}
	}


	impl pallet_nomination_pools_runtime_api::NominationPoolsApi<
		Block,
		AccountId,
		Balance,
	> for Runtime {
		fn pending_rewards(member: AccountId) -> Balance {
			NominationPools::api_pending_rewards(member).unwrap_or_default()
		}

		fn points_to_balance(pool_id: PoolId, points: Balance) -> Balance {
			NominationPools::api_points_to_balance(pool_id, points)
		}

		fn balance_to_points(pool_id: PoolId, new_funds: Balance) -> Balance {
			NominationPools::api_balance_to_points(pool_id, new_funds)
		}

		fn pool_pending_slash(pool_id: PoolId) -> Balance {
			NominationPools::api_pool_pending_slash(pool_id)
		}

		fn member_pending_slash(member: AccountId) -> Balance {
			NominationPools::api_member_pending_slash(member)
		}

		fn pool_needs_delegate_migration(pool_id: PoolId) -> bool {
			NominationPools::api_pool_needs_delegate_migration(pool_id)
		}

		fn member_needs_delegate_migration(member: AccountId) -> bool {
			NominationPools::api_member_needs_delegate_migration(member)
		}

		fn member_total_balance(member: AccountId) -> Balance {
			NominationPools::api_member_total_balance(member)
		}

		fn pool_balance(pool_id: PoolId) -> Balance {
			NominationPools::api_pool_balance(pool_id)
		}

		fn pool_accounts(pool_id: PoolId) -> (AccountId, AccountId) {
			NominationPools::api_pool_accounts(pool_id)
		}
	}

	impl pallet_staking_runtime_api::StakingApi<Block, Balance, AccountId> for Runtime {
		fn nominations_quota(balance: Balance) -> u32 {
			Staking::api_nominations_quota(balance)
		}

		fn eras_stakers_page_count(era: sp_staking::EraIndex, account: AccountId) -> sp_staking::Page {
			Staking::api_eras_stakers_page_count(era, account)
		}

		fn pending_rewards(era: sp_staking::EraIndex, account: AccountId) -> bool {
			Staking::api_pending_rewards(era, account)
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::BenchmarkList;
			use frame_support::traits::StorageInfoTrait;
			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsicsBenchmark;
			use pallet_xcm_bridge_hub_router::benchmarking::Pallet as XcmBridgeHubRouterBench;

			// This is defined once again in dispatch_benchmark, because list_benchmarks!
			// and add_benchmarks! are macros exported by define_benchmarks! macros and those types
			// are referenced in that call.
			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			// Benchmark files generated for `Assets/ForeignAssets` instances are by default
			// `pallet_assets_assets.rs / pallet_assets_foreign_assets`, which is not really nice,
			// so with this redefinition we can change names to nicer:
			// `pallet_assets_local.rs / pallet_assets_foreign.rs`.
			type Local = pallet_assets::Pallet::<Runtime, TrustBackedAssetsInstance>;
			type Foreign = pallet_assets::Pallet::<Runtime, ForeignAssetsInstance>;
			type Pool = pallet_assets::Pallet::<Runtime, PoolAssetsInstance>;

			type ToRococo = XcmBridgeHubRouterBench<Runtime, ToRococoXcmRouterInstance>;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			(list, storage_info)
		}

		#[allow(non_local_definitions)]
		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
			use frame_benchmarking::{BenchmarkBatch, BenchmarkError};
			use frame_support::assert_ok;
			use sp_storage::TrackedStorageKey;

			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
			impl frame_system_benchmarking::Config for Runtime {
				fn setup_set_code_requirements(code: &alloc::vec::Vec<u8>) -> Result<(), BenchmarkError> {
					ParachainSystem::initialize_for_set_code_benchmark(code.len() as u32);
					Ok(())
				}

				fn verify_set_code() {
					System::assert_last_event(cumulus_pallet_parachain_system::Event::<Runtime>::ValidationFunctionStored.into());
				}
			}

			use cumulus_pallet_session_benchmarking::Pallet as SessionBench;
			impl cumulus_pallet_session_benchmarking::Config for Runtime {}

			parameter_types! {
				pub ExistentialDepositAsset: Option<Asset> = Some((
					WestendLocation::get(),
					ExistentialDeposit::get()
				).into());
				pub const RandomParaId: ParaId = ParaId::new(43211234);
			}

			use pallet_xcm::benchmarking::Pallet as PalletXcmExtrinsicsBenchmark;
			impl pallet_xcm::benchmarking::Config for Runtime {
				type DeliveryHelper = (
					cumulus_primitives_utility::ToParentDeliveryHelper<
						xcm_config::XcmConfig,
						ExistentialDepositAsset,
						xcm_config::PriceForParentDelivery,
					>,
					polkadot_runtime_common::xcm_sender::ToParachainDeliveryHelper<
						xcm_config::XcmConfig,
						ExistentialDepositAsset,
						PriceForSiblingParachainDelivery,
						RandomParaId,
						ParachainSystem,
					>
				);

				fn reachable_dest() -> Option<Location> {
					Some(Parent.into())
				}

				fn teleportable_asset_and_dest() -> Option<(Asset, Location)> {
					// Relay/native token can be teleported between AH and Relay.
					Some((
						Asset {
							fun: Fungible(ExistentialDeposit::get()),
							id: AssetId(Parent.into())
						},
						Parent.into(),
					))
				}

				fn reserve_transferable_asset_and_dest() -> Option<(Asset, Location)> {
					Some((
						Asset {
							fun: Fungible(ExistentialDeposit::get()),
							id: AssetId(Parent.into())
						},
						// AH can reserve transfer native token to some random parachain.
						ParentThen(Parachain(RandomParaId::get().into()).into()).into(),
					))
				}

				fn set_up_complex_asset_transfer(
				) -> Option<(XcmAssets, u32, Location, alloc::boxed::Box<dyn FnOnce()>)> {
					// Transfer to Relay some local AH asset (local-reserve-transfer) while paying
					// fees using teleported native token.
					// (We don't care that Relay doesn't accept incoming unknown AH local asset)
					let dest = Parent.into();

					let fee_amount = EXISTENTIAL_DEPOSIT;
					let fee_asset: Asset = (Location::parent(), fee_amount).into();

					let who = frame_benchmarking::whitelisted_caller();
					// Give some multiple of the existential deposit
					let balance = fee_amount + EXISTENTIAL_DEPOSIT * 1000;
					let _ = <Balances as frame_support::traits::Currency<_>>::make_free_balance_be(
						&who, balance,
					);
					// verify initial balance
					assert_eq!(Balances::free_balance(&who), balance);

					// set up local asset
					let asset_amount = 10u128;
					let initial_asset_amount = asset_amount * 10;
					let (asset_id, _, _) = pallet_assets::benchmarking::create_default_minted_asset::<
						Runtime,
						pallet_assets::Instance1
					>(true, initial_asset_amount);
					let asset_location = Location::new(
						0,
						[PalletInstance(50), GeneralIndex(u32::from(asset_id).into())]
					);
					let transfer_asset: Asset = (asset_location, asset_amount).into();

					let assets: XcmAssets = vec![fee_asset.clone(), transfer_asset].into();
					let fee_index = if assets.get(0).unwrap().eq(&fee_asset) { 0 } else { 1 };

					// verify transferred successfully
					let verify = alloc::boxed::Box::new(move || {
						// verify native balance after transfer, decreased by transferred fee amount
						// (plus transport fees)
						assert!(Balances::free_balance(&who) <= balance - fee_amount);
						// verify asset balance decreased by exactly transferred amount
						assert_eq!(
							Assets::balance(asset_id.into(), &who),
							initial_asset_amount - asset_amount,
						);
					});
					Some((assets, fee_index as u32, dest, verify))
				}

				fn get_asset() -> Asset {
					use frame_benchmarking::whitelisted_caller;
					use frame_support::traits::tokens::fungible::{Inspect, Mutate};
					let account = whitelisted_caller();
					assert_ok!(<Balances as Mutate<_>>::mint_into(
						&account,
						<Balances as Inspect<_>>::minimum_balance(),
					));
					let asset_id = 1984;
					assert_ok!(Assets::force_create(
						RuntimeOrigin::root(),
						asset_id.into(),
						account.into(),
						true,
						1u128,
					));
					let amount = 1_000_000u128;
					let asset_location = Location::new(0, [PalletInstance(50), GeneralIndex(u32::from(asset_id).into())]);

					Asset {
						id: AssetId(asset_location),
						fun: Fungible(amount),
					}
				}
			}

			use pallet_xcm_bridge_hub_router::benchmarking::{
				Pallet as XcmBridgeHubRouterBench,
				Config as XcmBridgeHubRouterConfig,
			};

			impl XcmBridgeHubRouterConfig<ToRococoXcmRouterInstance> for Runtime {
				fn make_congested() {
					cumulus_pallet_xcmp_queue::bridging::suspend_channel_for_benchmarks::<Runtime>(
						xcm_config::bridging::SiblingBridgeHubParaId::get().into()
					);
				}
				fn ensure_bridged_target_destination() -> Result<Location, BenchmarkError> {
					ParachainSystem::open_outbound_hrmp_channel_for_benchmarks_or_tests(
						xcm_config::bridging::SiblingBridgeHubParaId::get().into()
					);
					let bridged_asset_hub = xcm_config::bridging::to_rococo::AssetHubRococo::get();
					let _ = PolkadotXcm::force_xcm_version(
						RuntimeOrigin::root(),
						alloc::boxed::Box::new(bridged_asset_hub.clone()),
						XCM_VERSION,
					).map_err(|e| {
						tracing::error!(
							target: "bridges::benchmark",
							error=?e,
							origin=?RuntimeOrigin::root(),
							location=?bridged_asset_hub,
							version=?XCM_VERSION,
							"Failed to dispatch `force_xcm_version`"
						);
						BenchmarkError::Stop("XcmVersion was not stored!")
					})?;
					Ok(bridged_asset_hub)
				}
			}

			use xcm_config::{MaxAssetsIntoHolding, WestendLocation};
			use pallet_xcm_benchmarks::asset_instance_from;

			impl pallet_xcm_benchmarks::Config for Runtime {
				type XcmConfig = xcm_config::XcmConfig;
				type AccountIdConverter = xcm_config::LocationToAccountId;
				type DeliveryHelper = cumulus_primitives_utility::ToParentDeliveryHelper<
					xcm_config::XcmConfig,
					ExistentialDepositAsset,
					xcm_config::PriceForParentDelivery,
				>;
				fn valid_destination() -> Result<Location, BenchmarkError> {
					Ok(WestendLocation::get())
				}
				fn worst_case_holding(depositable_count: u32) -> XcmAssets {
					// A mix of fungible, non-fungible, and concrete assets.
					let holding_non_fungibles = MaxAssetsIntoHolding::get() / 2 - depositable_count;
					let holding_fungibles = holding_non_fungibles - 2; // -2 for two `iter::once` bellow
					let fungibles_amount: u128 = 100;
					(0..holding_fungibles)
						.map(|i| {
							Asset {
								id: AssetId(GeneralIndex(i as u128).into()),
								fun: Fungible(fungibles_amount * (i + 1) as u128), // non-zero amount
							}
						})
						.chain(core::iter::once(Asset { id: AssetId(Here.into()), fun: Fungible(u128::MAX) }))
						.chain(core::iter::once(Asset { id: AssetId(WestendLocation::get()), fun: Fungible(1_000_000 * UNITS) }))
						.chain((0..holding_non_fungibles).map(|i| Asset {
							id: AssetId(GeneralIndex(i as u128).into()),
							fun: NonFungible(asset_instance_from(i)),
						}))
						.collect::<Vec<_>>()
						.into()
				}
			}

			parameter_types! {
				pub const TrustedTeleporter: Option<(Location, Asset)> = Some((
					WestendLocation::get(),
					Asset { fun: Fungible(UNITS), id: AssetId(WestendLocation::get()) },
				));
				pub const CheckedAccount: Option<(AccountId, xcm_builder::MintLocation)> = None;
				// AssetHubWestend trusts AssetHubRococo as reserve for ROCs
				pub TrustedReserve: Option<(Location, Asset)> = Some(
					(
						xcm_config::bridging::to_rococo::AssetHubRococo::get(),
						Asset::from((xcm_config::bridging::to_rococo::RocLocation::get(), 1000000000000 as u128))
					)
				);
			}

			impl pallet_xcm_benchmarks::fungible::Config for Runtime {
				type TransactAsset = Balances;

				type CheckedAccount = CheckedAccount;
				type TrustedTeleporter = TrustedTeleporter;
				type TrustedReserve = TrustedReserve;

				fn get_asset() -> Asset {
					use frame_support::traits::tokens::fungible::{Inspect, Mutate};
					let (account, _) = pallet_xcm_benchmarks::account_and_location::<Runtime>(1);
					assert_ok!(<Balances as Mutate<_>>::mint_into(
						&account,
						<Balances as Inspect<_>>::minimum_balance(),
					));
					let asset_id = 1984;
					assert_ok!(Assets::force_create(
						RuntimeOrigin::root(),
						asset_id.into(),
						account.clone().into(),
						true,
						1u128,
					));
					let amount = 1_000_000u128;
					let asset_location = Location::new(0, [PalletInstance(50), GeneralIndex(u32::from(asset_id).into())]);

					Asset {
						id: AssetId(asset_location),
						fun: Fungible(amount),
					}
				}
			}

			impl pallet_xcm_benchmarks::generic::Config for Runtime {
				type TransactAsset = Balances;
				type RuntimeCall = RuntimeCall;

				fn worst_case_response() -> (u64, Response) {
					(0u64, Response::Version(Default::default()))
				}

				fn worst_case_asset_exchange() -> Result<(XcmAssets, XcmAssets), BenchmarkError> {
					let native_asset_location = WestendLocation::get();
					let native_asset_id = AssetId(native_asset_location.clone());
					let (account, _) = pallet_xcm_benchmarks::account_and_location::<Runtime>(1);
					let origin = RuntimeOrigin::signed(account.clone());
					let asset_location = Location::new(1, [Parachain(2001)]);
					let asset_id = AssetId(asset_location.clone());

					assert_ok!(<Balances as fungible::Mutate<_>>::mint_into(
						&account,
						ExistentialDeposit::get() + (1_000 * UNITS)
					));

					assert_ok!(ForeignAssets::force_create(
						RuntimeOrigin::root(),
						asset_location.clone().into(),
						account.clone().into(),
						true,
						1,
					));

					assert_ok!(ForeignAssets::mint(
						origin.clone(),
						asset_location.clone().into(),
						account.clone().into(),
						3_000 * UNITS,
					));

					assert_ok!(AssetConversion::create_pool(
						origin.clone(),
						native_asset_location.clone().into(),
						asset_location.clone().into(),
					));

					assert_ok!(AssetConversion::add_liquidity(
						origin,
						native_asset_location.into(),
						asset_location.into(),
						1_000 * UNITS,
						2_000 * UNITS,
						1,
						1,
						account.into(),
					));

					let give_assets: XcmAssets = (native_asset_id, 500 * UNITS).into();
					let receive_assets: XcmAssets = (asset_id, 660 * UNITS).into();

					Ok((give_assets, receive_assets))
				}

				fn universal_alias() -> Result<(Location, Junction), BenchmarkError> {
					xcm_config::bridging::BridgingBenchmarksHelper::prepare_universal_alias()
					.ok_or(BenchmarkError::Skip)
				}

				fn transact_origin_and_runtime_call() -> Result<(Location, RuntimeCall), BenchmarkError> {
					Ok((WestendLocation::get(), frame_system::Call::remark_with_event { remark: vec![] }.into()))
				}

				fn subscribe_origin() -> Result<Location, BenchmarkError> {
					Ok(WestendLocation::get())
				}

				fn claimable_asset() -> Result<(Location, Location, XcmAssets), BenchmarkError> {
					let origin = WestendLocation::get();
					let assets: XcmAssets = (AssetId(WestendLocation::get()), 1_000 * UNITS).into();
					let ticket = Location { parents: 0, interior: Here };
					Ok((origin, ticket, assets))
				}

				fn worst_case_for_trader() -> Result<(Asset, WeightLimit), BenchmarkError> {
					Ok((Asset {
						id: AssetId(WestendLocation::get()),
						fun: Fungible(1_000 * UNITS),
					}, WeightLimit::Limited(Weight::from_parts(5000, 5000))))
				}

				fn unlockable_asset() -> Result<(Location, Location, Asset), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn export_message_origin_and_destination(
				) -> Result<(Location, NetworkId, InteriorLocation), BenchmarkError> {
					Err(BenchmarkError::Skip)
				}

				fn alias_origin() -> Result<(Location, Location), BenchmarkError> {
					// Any location can alias to an internal location.
					// Here parachain 1001 aliases to an internal account.
					Ok((
						Location::new(1, [Parachain(1001)]),
						Location::new(1, [Parachain(1001), AccountId32 { id: [111u8; 32], network: None }]),
					))
				}
			}

			type XcmBalances = pallet_xcm_benchmarks::fungible::Pallet::<Runtime>;
			type XcmGeneric = pallet_xcm_benchmarks::generic::Pallet::<Runtime>;

			type Local = pallet_assets::Pallet::<Runtime, TrustBackedAssetsInstance>;
			type Foreign = pallet_assets::Pallet::<Runtime, ForeignAssetsInstance>;
			type Pool = pallet_assets::Pallet::<Runtime, PoolAssetsInstance>;

			type ToRococo = XcmBridgeHubRouterBench<Runtime, ToRococoXcmRouterInstance>;

			use frame_support::traits::WhitelistedStorageKeys;
			let whitelist: Vec<TrackedStorageKey> = AllPalletsWithSystem::whitelisted_storage_keys();

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			Ok(batches)
		}
	}

	impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
		fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
			build_state::<RuntimeGenesisConfig>(config)
		}

		fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
			get_preset::<RuntimeGenesisConfig>(id, &genesis_config_presets::get_preset)
		}

		fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
			genesis_config_presets::preset_names()
		}
	}
);

cumulus_pallet_parachain_system::register_validate_block! {
	Runtime = Runtime,
	BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}

parameter_types! {
	// The deposit configuration for the singed migration. Specially if you want to allow any signed account to do the migration (see `SignedFilter`, these deposits should be high)
	pub const MigrationSignedDepositPerItem: Balance = CENTS;
	pub const MigrationSignedDepositBase: Balance = 2_000 * CENTS;
	pub const MigrationMaxKeyLen: u32 = 512;
}

impl pallet_state_trie_migration::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type RuntimeHoldReason = RuntimeHoldReason;
	type SignedDepositPerItem = MigrationSignedDepositPerItem;
	type SignedDepositBase = MigrationSignedDepositBase;
	// An origin that can control the whole pallet: should be Root, or a part of your council.
	type ControlOrigin = frame_system::EnsureSignedBy<RootMigController, AccountId>;
	// specific account for the migration, can trigger the signed migrations.
	type SignedFilter = frame_system::EnsureSignedBy<MigController, AccountId>;

	// Replace this with weight based on your runtime.
	type WeightInfo = pallet_state_trie_migration::weights::SubstrateWeight<Runtime>;

	type MaxKeyLen = MigrationMaxKeyLen;
}

frame_support::ord_parameter_types! {
	pub const MigController: AccountId = AccountId::from(hex_literal::hex!("8458ed39dc4b6f6c7255f7bc42be50c2967db126357c999d44e12ca7ac80dc52"));
	pub const RootMigController: AccountId = AccountId::from(hex_literal::hex!("8458ed39dc4b6f6c7255f7bc42be50c2967db126357c999d44e12ca7ac80dc52"));
}

#[test]
fn ensure_key_ss58() {
	use frame_support::traits::SortedMembers;
	use sp_core::crypto::Ss58Codec;
	let acc =
		AccountId::from_ss58check("5F4EbSkZz18X36xhbsjvDNs6NuZ82HyYtq5UiJ1h9SBHJXZD").unwrap();
	assert_eq!(acc, MigController::sorted_members()[0]);
	let acc =
		AccountId::from_ss58check("5F4EbSkZz18X36xhbsjvDNs6NuZ82HyYtq5UiJ1h9SBHJXZD").unwrap();
	assert_eq!(acc, RootMigController::sorted_members()[0]);
}

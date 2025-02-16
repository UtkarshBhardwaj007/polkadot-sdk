// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! A helper module for calling the GenesisBuilder API from arbitrary runtime wasm blobs.

use codec::{Decode, Encode};
pub use sc_executor::sp_wasm_interface::HostFunctions;
use sc_executor::{error::Result, WasmExecutor};
use serde_json::{from_slice, Value};
use sp_core::{
	storage::Storage,
	traits::{CallContext, CodeExecutor, Externalities, FetchRuntimeCode, RuntimeCode},
};
use sp_genesis_builder::{PresetId, Result as BuildResult};
pub use sp_genesis_builder::{DEV_RUNTIME_PRESET, LOCAL_TESTNET_RUNTIME_PRESET};
use sp_state_machine::BasicExternalities;
use std::borrow::Cow;

/// A utility that facilitates calling the GenesisBuilder API from the runtime wasm code blob.
///
/// `EHF` type allows to specify the extended host function required for building runtime's genesis
/// config. The type will be combined with default `sp_io::SubstrateHostFunctions`.
pub struct GenesisConfigBuilderRuntimeCaller<'a, EHF = ()>
where
	EHF: HostFunctions,
{
	code: Cow<'a, [u8]>,
	code_hash: Vec<u8>,
	executor: WasmExecutor<(sp_io::SubstrateHostFunctions, EHF)>,
}

impl<'a, EHF> FetchRuntimeCode for GenesisConfigBuilderRuntimeCaller<'a, EHF>
where
	EHF: HostFunctions,
{
	fn fetch_runtime_code(&self) -> Option<Cow<[u8]>> {
		Some(self.code.as_ref().into())
	}
}

impl<'a, EHF> GenesisConfigBuilderRuntimeCaller<'a, EHF>
where
	EHF: HostFunctions,
{
	/// Creates new instance using the provided code blob.
	///
	/// This code is later referred to as `runtime`.
	pub fn new(code: &'a [u8]) -> Self {
		GenesisConfigBuilderRuntimeCaller {
			code: code.into(),
			code_hash: sp_crypto_hashing::blake2_256(code).to_vec(),
			executor: WasmExecutor::<(sp_io::SubstrateHostFunctions, EHF)>::builder()
				.with_allow_missing_host_functions(true)
				.build(),
		}
	}

	fn call(&self, ext: &mut dyn Externalities, method: &str, data: &[u8]) -> Result<Vec<u8>> {
		self.executor
			.call(
				ext,
				&RuntimeCode { heap_pages: None, code_fetcher: self, hash: self.code_hash.clone() },
				method,
				data,
				CallContext::Offchain,
			)
			.0
	}

	/// Returns a json representation of the default `RuntimeGenesisConfig` provided by the
	/// `runtime`.
	///
	/// Calls [`GenesisBuilder::get_preset`](sp_genesis_builder::GenesisBuilder::get_preset) in the
	/// `runtime` with `None` argument.
	pub fn get_default_config(&self) -> core::result::Result<Value, String> {
		self.get_named_preset(None)
	}

	/// Returns a JSON blob representation of the builtin `GenesisConfig` identified by `id`.
	///
	/// Calls [`GenesisBuilder::get_preset`](sp_genesis_builder::GenesisBuilder::get_preset)
	/// provided by the `runtime`.
	pub fn get_named_preset(&self, id: Option<&String>) -> core::result::Result<Value, String> {
		let mut t = BasicExternalities::new_empty();
		let call_result = self
			.call(&mut t, "GenesisBuilder_get_preset", &id.encode())
			.map_err(|e| format!("wasm call error {e}"))?;

		let named_preset = Option::<Vec<u8>>::decode(&mut &call_result[..])
			.map_err(|e| format!("scale codec error: {e}"))?;

		if let Some(named_preset) = named_preset {
			Ok(from_slice(&named_preset[..]).expect("returned value is json. qed."))
		} else {
			Err(format!("The preset with name {id:?} is not available."))
		}
	}

	/// Calls [`sp_genesis_builder::GenesisBuilder::build_state`] provided by runtime.
	pub fn get_storage_for_config(&self, config: Value) -> core::result::Result<Storage, String> {
		let mut ext = BasicExternalities::new_empty();

		let json_pretty_str = serde_json::to_string_pretty(&config)
			.map_err(|e| format!("json to string failed: {e}"))?;

		let call_result = self
			.call(&mut ext, "GenesisBuilder_build_state", &json_pretty_str.encode())
			.map_err(|e| format!("wasm call error {e}"))?;

		BuildResult::decode(&mut &call_result[..])
			.map_err(|e| format!("scale codec error: {e}"))?
			.map_err(|e| format!("{e} for blob:\n{}", json_pretty_str))?;

		Ok(ext.into_storages())
	}

	/// Creates the genesis state by patching the default `RuntimeGenesisConfig`.
	///
	/// This function generates the `RuntimeGenesisConfig` for the runtime by applying a provided
	/// JSON patch. The patch modifies the default `RuntimeGenesisConfig` allowing customization of
	/// the specific keys. The resulting `RuntimeGenesisConfig` is then deserialized from the
	/// patched JSON representation and stored in the storage.
	///
	/// If the provided JSON patch is incorrect or the deserialization fails the error will be
	/// returned.
	///
	/// The patching process modifies the default `RuntimeGenesisConfig` according to the following
	/// rules:
	/// 1. Existing keys in the default configuration will be overridden by the corresponding values
	///    in the patch (also applies to `null` values).
	/// 2. If a key exists in the patch but not in the default configuration, it will be added to
	///    the resulting `RuntimeGenesisConfig`.
	///
	/// Please note that the patch may contain full `RuntimeGenesisConfig`.
	pub fn get_storage_for_patch(&self, patch: Value) -> core::result::Result<Storage, String> {
		let mut config = self.get_default_config()?;
		crate::json_patch::merge(&mut config, patch);
		self.get_storage_for_config(config)
	}

	pub fn get_storage_for_named_preset(
		&self,
		name: Option<&String>,
	) -> core::result::Result<Storage, String> {
		self.get_storage_for_patch(self.get_named_preset(name)?)
	}

	pub fn preset_names(&self) -> core::result::Result<Vec<PresetId>, String> {
		let mut t = BasicExternalities::new_empty();
		let call_result = self
			.call(&mut t, "GenesisBuilder_preset_names", &vec![])
			.map_err(|e| format!("wasm call error {e}"))?;

		let preset_names = Vec::<PresetId>::decode(&mut &call_result[..])
			.map_err(|e| format!("scale codec error: {e}"))?;

		Ok(preset_names)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{from_str, json};
	pub use sp_consensus_babe::{AllowedSlots, BabeEpochConfiguration};
	pub use sp_genesis_builder::PresetId;

	#[test]
	fn list_presets_works() {
		sp_tracing::try_init_simple();
		let presets =
			<GenesisConfigBuilderRuntimeCaller>::new(substrate_test_runtime::wasm_binary_unwrap())
				.preset_names()
				.unwrap();
		assert_eq!(presets, vec![PresetId::from("foobar"), PresetId::from("staging"),]);
	}

	#[test]
	fn get_default_config_works() {
		let config =
			<GenesisConfigBuilderRuntimeCaller>::new(substrate_test_runtime::wasm_binary_unwrap())
				.get_default_config()
				.unwrap();
		let expected = r#"{"babe": {"authorities": [], "epochConfig": {"allowed_slots": "PrimaryAndSecondaryVRFSlots", "c": [1, 4]}}, "balances": {"balances": [], "devAccounts": null}, "substrateTest": {"authorities": []}, "system": {}}"#;
		assert_eq!(from_str::<Value>(expected).unwrap(), config);
	}

	#[test]
	fn get_named_preset_works() {
		sp_tracing::try_init_simple();
		let config =
			<GenesisConfigBuilderRuntimeCaller>::new(substrate_test_runtime::wasm_binary_unwrap())
				.get_named_preset(Some(&"foobar".to_string()))
				.unwrap();
		let expected = r#"{"foo":"bar"}"#;
		assert_eq!(from_str::<Value>(expected).unwrap(), config);
	}

	#[test]
	fn get_storage_for_patch_works() {
		let patch = json!({
			"babe": {
				"epochConfig": {
					"c": [
						69,
						696
					],
					"allowed_slots": "PrimaryAndSecondaryPlainSlots"
				}
			},
		});

		let storage =
			<GenesisConfigBuilderRuntimeCaller>::new(substrate_test_runtime::wasm_binary_unwrap())
				.get_storage_for_patch(patch)
				.unwrap();

		//Babe|Authorities
		let value: Vec<u8> = storage
			.top
			.get(
				&array_bytes::hex2bytes(
					"1cb6f36e027abb2091cfb5110ab5087fdc6b171b77304263c292cc3ea5ed31ef",
				)
				.unwrap(),
			)
			.unwrap()
			.clone();

		assert_eq!(
			BabeEpochConfiguration::decode(&mut &value[..]).unwrap(),
			BabeEpochConfiguration {
				c: (69, 696),
				allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots
			}
		);
	}
}

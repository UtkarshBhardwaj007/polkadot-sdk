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

//! Substrate offchain API.

pub mod error;

use error::Error;
use jsonrpsee::proc_macros::rpc;
use sp_core::{offchain::StorageKind, Bytes};

/// Substrate offchain RPC API
#[rpc(client, server)]
pub trait OffchainApi {
	/// Set offchain local storage under given key and prefix.
	#[method(name = "offchain_localStorageSet", with_extensions)]
	fn set_local_storage(&self, kind: StorageKind, key: Bytes, value: Bytes) -> Result<(), Error>;

	/// Clear offchain local storage under given key and prefix.
	#[method(name = "offchain_localStorageClear", with_extensions)]
	fn clear_local_storage(&self, kind: StorageKind, key: Bytes) -> Result<(), Error>;

	/// Get offchain local storage under given key and prefix.
	#[method(name = "offchain_localStorageGet", with_extensions)]
	fn get_local_storage(&self, kind: StorageKind, key: Bytes) -> Result<Option<Bytes>, Error>;
}

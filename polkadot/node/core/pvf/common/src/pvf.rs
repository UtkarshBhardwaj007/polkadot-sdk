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

use crate::prepare::PrepareJobKind;
use codec::{Decode, Encode};
use polkadot_parachain_primitives::primitives::ValidationCodeHash;
use polkadot_primitives::ExecutorParams;
use std::{fmt, sync::Arc, time::Duration};

/// A struct that carries the exhaustive set of data to prepare an artifact out of plain
/// Wasm binary
///
/// Should be cheap to clone.
#[derive(Clone, Encode, Decode)]
pub struct PvfPrepData {
	/// Wasm code (maybe compressed)
	maybe_compressed_code: Arc<Vec<u8>>,
	/// Maximum uncompressed code size.
	validation_code_bomb_limit: u32,
	/// Wasm code hash.
	code_hash: ValidationCodeHash,
	/// Executor environment parameters for the session for which artifact is prepared
	executor_params: Arc<ExecutorParams>,
	/// Preparation timeout
	prep_timeout: Duration,
	/// The kind of preparation job.
	prep_kind: PrepareJobKind,
}

impl PvfPrepData {
	/// Returns an instance of the PVF out of the given PVF code and executor params.
	pub fn from_code(
		code: Vec<u8>,
		executor_params: ExecutorParams,
		prep_timeout: Duration,
		prep_kind: PrepareJobKind,
		validation_code_bomb_limit: u32,
	) -> Self {
		let maybe_compressed_code = Arc::new(code);
		let code_hash = sp_crypto_hashing::blake2_256(&maybe_compressed_code).into();
		let executor_params = Arc::new(executor_params);
		Self {
			maybe_compressed_code,
			code_hash,
			executor_params,
			prep_timeout,
			prep_kind,
			validation_code_bomb_limit,
		}
	}

	/// Returns validation code hash
	pub fn code_hash(&self) -> ValidationCodeHash {
		self.code_hash
	}

	/// Returns PVF code blob
	pub fn maybe_compressed_code(&self) -> Arc<Vec<u8>> {
		self.maybe_compressed_code.clone()
	}

	/// Returns executor params
	pub fn executor_params(&self) -> Arc<ExecutorParams> {
		self.executor_params.clone()
	}

	/// Returns preparation timeout.
	pub fn prep_timeout(&self) -> Duration {
		self.prep_timeout
	}

	/// Returns preparation kind.
	pub fn prep_kind(&self) -> PrepareJobKind {
		self.prep_kind
	}

	/// Returns validation code bomb limit.
	pub fn validation_code_bomb_limit(&self) -> u32 {
		self.validation_code_bomb_limit
	}

	/// Creates a structure for tests.
	#[cfg(feature = "test-utils")]
	pub fn from_discriminator_and_timeout(num: u32, timeout: Duration) -> Self {
		let discriminator_buf = num.to_le_bytes().to_vec();
		Self::from_code(
			discriminator_buf,
			ExecutorParams::default(),
			timeout,
			PrepareJobKind::Compilation,
			30 * 1024 * 1024,
		)
	}

	/// Creates a structure for tests.
	#[cfg(feature = "test-utils")]
	pub fn from_discriminator(num: u32) -> Self {
		Self::from_discriminator_and_timeout(num, crate::tests::TEST_PREPARATION_TIMEOUT)
	}

	/// Creates a structure for tests.
	#[cfg(feature = "test-utils")]
	pub fn from_discriminator_precheck(num: u32) -> Self {
		let mut pvf =
			Self::from_discriminator_and_timeout(num, crate::tests::TEST_PREPARATION_TIMEOUT);
		pvf.prep_kind = PrepareJobKind::Prechecking;
		pvf
	}
}

impl fmt::Debug for PvfPrepData {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Pvf {{ code: [...], code_hash: {:?}, executor_params: {:?}, prep_timeout: {:?} }}",
			self.code_hash, self.executor_params, self.prep_timeout
		)
	}
}

impl PartialEq for PvfPrepData {
	fn eq(&self, other: &Self) -> bool {
		self.code_hash == other.code_hash &&
			self.executor_params.hash() == other.executor_params.hash()
	}
}

impl Eq for PvfPrepData {}

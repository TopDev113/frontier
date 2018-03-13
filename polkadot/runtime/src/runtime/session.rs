// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Session manager: is told the validators and allows them to manage their session keys for the
//! consensus module.

use rstd::prelude::*;
use codec::KeyedVec;
use runtime_support::{storage, StorageVec};
use polkadot_primitives::{AccountId, SessionKey, BlockNumber};
use runtime::{system, staking, consensus};

const SESSION_LENGTH: &[u8] = b"ses:len";
const CURRENT_INDEX: &[u8] = b"ses:ind";
const CURRENT_SESSION_START: &[u8] = b"ses:sta";
const LAST_SESSION_START: &[u8] = b"ses:lst";
const LAST_LENGTH_CHANGE: &[u8] = b"ses:llc";
const NEXT_KEY_FOR: &[u8] = b"ses:nxt:";
const NEXT_SESSION_LENGTH: &[u8] = b"ses:nln";

struct ValidatorStorageVec;
impl StorageVec for ValidatorStorageVec {
	type Item = AccountId;
	const PREFIX: &'static [u8] = b"ses:val:";
}

// the session keys before the previous.
struct LastValidators;
impl StorageVec for LastValidators {
	type Item = (AccountId, SessionKey);
	const PREFIX: &'static [u8] = b"ses:old:";
}

/// Get the current set of validators.
pub fn validators() -> Vec<AccountId> {
	ValidatorStorageVec::items()
}

/// The number of blocks in each session.
pub fn length() -> BlockNumber {
	storage::get_or(SESSION_LENGTH, 0)
}

/// The number of validators currently.
pub fn validator_count() -> u32 {
	ValidatorStorageVec::count() as u32
}

/// The current session index.
pub fn current_index() -> BlockNumber {
	storage::get_or(CURRENT_INDEX, 0)
}

/// Get the starting block of the current session.
pub fn current_start_block() -> BlockNumber {
	// this seems like it's computable just by examining the current block number, session length,
	// and last length change, but it's not simple to tell whether we are before or after
	// a session rotation on a block which will have one.
	storage::get_or(CURRENT_SESSION_START, 0)
}

/// Get the last session's validators, paired with their authority keys.
pub fn last_session_keys() -> Vec<(AccountId, SessionKey)> {
	LastValidators::items()
}

/// Get the start block of the last session.
/// In general this is computable from the session length,
/// but when the current session is the first with a new length it is uncomputable.
pub fn last_session_start() -> Option<BlockNumber> {
	storage::get(LAST_SESSION_START)
}

/// The block number at which the era length last changed.
pub fn last_length_change() -> BlockNumber {
	storage::get_or(LAST_LENGTH_CHANGE, 0)
}

pub mod public {
	use super::*;

	/// Sets the session key of `_validator` to `_key`. This doesn't take effect until the next
	/// session.
	pub fn set_key(validator: &AccountId, key: &SessionKey) {
		// set new value for next session
		storage::put(&validator.to_keyed_vec(NEXT_KEY_FOR), key);
	}
}

pub mod privileged {
	use super::*;

	/// Set a new era length. Won't kick in until the next era change (at current length).
	pub fn set_length(new: BlockNumber) {
		storage::put(NEXT_SESSION_LENGTH, &new);
	}

	/// Forces a new session.
	pub fn force_new_session() {
		rotate_session();
	}
}

// INTERNAL API (available to other runtime modules)

pub mod internal {
	use super::*;

	/// Transition to a new era, with a new set of valiators.
	///
	/// Called by staking::next_era() only. `next_session` should be called after this in order to
	/// update the session keys to the next validator set.
	pub fn set_validators(new: &[AccountId]) {
		LastValidators::set_items(
			new.iter().cloned().zip(consensus::authorities())
		);
		ValidatorStorageVec::set_items(new);
		consensus::internal::set_authorities(new);
	}

	/// Hook to be called after transaction processing.
	pub fn check_rotate_session() {
		// do this last, after the staking system has had chance to switch out the authorities for the
		// new set.
		// check block number and call next_session if necessary.
		if (system::block_number() - last_length_change()) % length() == 0 {
			rotate_session();
		}
	}
}

/// Move onto next session: register the new authority set.
fn rotate_session() {
	// Increment current session index.
	storage::put(CURRENT_INDEX, &(current_index() + 1));
	// Enact era length change.
	if let Some(next_len) = storage::get::<u64>(NEXT_SESSION_LENGTH) {
		storage::put(SESSION_LENGTH, &next_len);
		storage::put(LAST_LENGTH_CHANGE, &system::block_number());
		storage::kill(NEXT_SESSION_LENGTH);
	}

	let validators = validators();

	storage::put(LAST_SESSION_START, &current_start_block());
	storage::put(CURRENT_SESSION_START, &system::block_number());
	LastValidators::set_items(
		validators.iter()
			.cloned()
			.zip(consensus::authorities())
	);


	// Update any changes in session keys.
	validators.iter().enumerate().for_each(|(i, v)| {
		let k = v.to_keyed_vec(NEXT_KEY_FOR);
		if let Some(n) = storage::take(&k) {
			// this is fine because the authorities vector currently
			// matches the validators length perfectly.
			consensus::internal::set_authority(i as u32, &n);
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use super::public::*;
	use super::privileged::*;
	use super::internal::*;
	use runtime_io::{with_externalities, twox_128, TestExternalities};
	use codec::{KeyedVec, Joiner};
	use keyring::Keyring;
	use environment::with_env;
	use polkadot_primitives::AccountId;
	use runtime::{consensus, session};

	fn simple_setup() -> TestExternalities {
		map![
			twox_128(SESSION_LENGTH).to_vec() => vec![].and(&2u64),
			// the validators (10, 20, ...)
			twox_128(b"ses:val:len").to_vec() => vec![].and(&2u32),
			twox_128(&0u32.to_keyed_vec(ValidatorStorageVec::PREFIX)).to_vec() => vec![10; 32],
			twox_128(&1u32.to_keyed_vec(ValidatorStorageVec::PREFIX)).to_vec() => vec![20; 32],
			// initial session keys (11, 21, ...)
			b":auth:len".to_vec() => vec![].and(&2u32),
			0u32.to_keyed_vec(b":auth:") => vec![11; 32],
			1u32.to_keyed_vec(b":auth:") => vec![21; 32]
		]
	}

	#[test]
	fn simple_setup_should_work() {
		let mut t = simple_setup();
		with_externalities(&mut t, || {
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [21u8; 32]]);
			assert_eq!(length(), 2u64);
			assert_eq!(validators(), vec![[10u8; 32], [20u8; 32]]);
		});
	}

	#[test]
	fn session_length_change_should_work() {
		let mut t = simple_setup();
		with_externalities(&mut t, || {
			// Block 1: Change to length 3; no visible change.
			with_env(|e| e.block_number = 1);
			set_length(3);
			check_rotate_session();
			assert_eq!(length(), 2);
			assert_eq!(current_index(), 0);

			// Block 2: Length now changed to 3. Index incremented.
			with_env(|e| e.block_number = 2);
			set_length(3);
			check_rotate_session();
			assert_eq!(length(), 3);
			assert_eq!(current_index(), 1);

			// Block 3: Length now changed to 3. Index incremented.
			with_env(|e| e.block_number = 3);
			check_rotate_session();
			assert_eq!(length(), 3);
			assert_eq!(current_index(), 1);

			// Block 4: Change to length 2; no visible change.
			with_env(|e| e.block_number = 4);
			set_length(2);
			check_rotate_session();
			assert_eq!(length(), 3);
			assert_eq!(current_index(), 1);

			// Block 5: Length now changed to 2. Index incremented.
			with_env(|e| e.block_number = 5);
			check_rotate_session();
			assert_eq!(length(), 2);
			assert_eq!(current_index(), 2);

			// Block 6: No change.
			with_env(|e| e.block_number = 6);
			check_rotate_session();
			assert_eq!(length(), 2);
			assert_eq!(current_index(), 2);

			// Block 7: Next index.
			with_env(|e| e.block_number = 7);
			check_rotate_session();
			assert_eq!(length(), 2);
			assert_eq!(current_index(), 3);
		});
	}

	#[test]
	fn session_change_should_work() {
		let mut t = simple_setup();
		with_externalities(&mut t, || {
			// Block 1: No change
			with_env(|e| e.block_number = 1);
			check_rotate_session();
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [21u8; 32]]);

			// Block 2: Session rollover, but no change.
			with_env(|e| e.block_number = 2);
			check_rotate_session();
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [21u8; 32]]);

			// Block 3: Set new key for validator 2; no visible change.
			with_env(|e| e.block_number = 3);
			set_key(&[20; 32], &[22; 32]);
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [21u8; 32]]);

			check_rotate_session();
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [21u8; 32]]);

			// Block 4: Session rollover, authority 2 changes.
			with_env(|e| e.block_number = 4);
			check_rotate_session();
			assert_eq!(consensus::authorities(), vec![[11u8; 32], [22u8; 32]]);
		});
	}
}

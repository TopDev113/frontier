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

//! Tool for creating the genesis block.

use codec::{KeyedVec, Joiner};
use polkadot_primitives::{BlockNumber, Block, AccountId};
use std::collections::HashMap;
use runtime_io::twox_128;
use runtime::staking::Balance;
use support::Hashable;

/// Configuration of a general Polkadot genesis block.
pub struct GenesisConfig {
	pub validators: Vec<AccountId>,
	pub authorities: Vec<AccountId>,
	pub balances: Vec<(AccountId, Balance)>,
	pub block_time: u64,
	pub session_length: BlockNumber,
	pub sessions_per_era: BlockNumber,
	pub bonding_duration: BlockNumber,
	pub approval_ratio: u32,
}

impl GenesisConfig {
	pub fn new_simple(authorities_validators: Vec<AccountId>, balance: Balance) -> Self {
		GenesisConfig {
			validators: authorities_validators.clone(),
			authorities: authorities_validators.clone(),
			balances: authorities_validators.iter().map(|v| (v.clone(), balance)).collect(),
			block_time: 5,			// 5 second block time.
			session_length: 720,	// that's 1 hour per session.
			sessions_per_era: 24,	// 24 hours per era.
			bonding_duration: 90,	// 90 days per bond.
			approval_ratio: 667,	// 66.7% approvals required for legislation.
		}
	}

	pub fn genesis_map(&self) -> HashMap<Vec<u8>, Vec<u8>> {
		let wasm_runtime = include_bytes!("../wasm/genesis.wasm").to_vec();
		vec![
			(&b"gov:apr"[..], vec![].join(&self.approval_ratio)),
			(&b"ses:len"[..], vec![].join(&self.session_length)),
			(&b"ses:val:len"[..], vec![].join(&(self.validators.len() as u32))),
			(&b"sta:wil:len"[..], vec![].join(&0u32)),
			(&b"sta:spe"[..], vec![].join(&self.sessions_per_era)),
			(&b"sta:vac"[..], vec![].join(&(self.validators.len() as u32))),
			(&b"sta:era"[..], vec![].join(&0u64)),
		].into_iter()
			.map(|(k, v)| (k.into(), v))
			.chain(self.validators.iter()
				.enumerate()
				.map(|(i, account)| ((i as u32).to_keyed_vec(b"ses:val:"), vec![].join(account)))
			).chain(self.authorities.iter()
				.enumerate()
				.map(|(i, account)| ((i as u32).to_keyed_vec(b":auth:"), vec![].join(account)))
			).chain(self.balances.iter()
				.map(|&(account, balance)| (account.to_keyed_vec(b"sta:bal:"), vec![].join(&balance)))
			)
			.map(|(k, v)| (twox_128(&k[..])[..].to_vec(), v.to_vec()))
			.chain(vec![
				(b":code"[..].into(), wasm_runtime),
				(b":auth:len"[..].into(), vec![].join(&(self.authorities.len() as u32))),
			].into_iter())
			.chain(self.authorities.iter()
				.enumerate()
				.map(|(i, account)| ((i as u32).to_keyed_vec(b":auth:"), vec![].join(account)))
			)
			.collect()
	}
}

pub fn additional_storage_with_genesis(genesis_block: &Block) -> HashMap<Vec<u8>, Vec<u8>> {
	use codec::Slicable;
	map![
		twox_128(&0u64.to_keyed_vec(b"sys:old:")).to_vec() => genesis_block.header.blake2_256().to_vec()
	]
}

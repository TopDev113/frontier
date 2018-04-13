// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate blockchain API.

use std::sync::Arc;
use primitives::block;
use client::{self, Client};
use state_machine;

mod error;

#[cfg(test)]
mod tests;

use self::error::{Result, ResultExt};

build_rpc_trait! {
	/// Polkadot blockchain API
	pub trait ChainApi {
		/// Get header of a relay chain block.
		#[rpc(name = "chain_getHeader")]
		fn header(&self, block::HeaderHash) -> Result<Option<block::Header>>;

		/// Get hash of the head.
		#[rpc(name = "chain_getHead")]
		fn head(&self) -> Result<block::HeaderHash>;
	}
}

impl<B, E> ChainApi for Arc<Client<B, E>> where
	B: client::backend::Backend + Send + Sync + 'static,
	E: state_machine::CodeExecutor + Send + Sync + 'static,
	client::error::Error: From<<<B as client::backend::Backend>::State as state_machine::backend::Backend>::Error>,
{
	fn header(&self, hash: block::HeaderHash) -> Result<Option<block::Header>> {
		client::Client::header(self, &block::Id::Hash(hash)).chain_err(|| "Blockchain error")
	}

	fn head(&self) -> Result<block::HeaderHash> {
		Ok(client::Client::info(self).chain_err(|| "Blockchain error")?.chain.best_hash)
	}
}

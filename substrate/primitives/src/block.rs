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

//! Block and header type definitions.

#[cfg(feature = "std")]
use bytes;
use rstd::vec::Vec;
use codec::{Input, Slicable};
use hash::H256;

/// Used to refer to a block number.
pub type Number = u64;

/// Hash used to refer to a block hash.
pub type HeaderHash = H256;

/// Hash used to refer to a transaction hash.
pub type TransactionHash = H256;

/// Simple generic transaction type.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Transaction(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl Slicable for Transaction {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Vec::<u8>::decode(input).map(Transaction)
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		self.0.using_encoded(f)
	}
}

impl ::codec::NonTrivialSlicable for Transaction { }

/// Execution log (event)
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Log(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

impl Slicable for Log {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Vec::<u8>::decode(input).map(Log)
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		self.0.using_encoded(f)
	}
}

impl ::codec::NonTrivialSlicable for Log { }

/// The digest of a block, useful for light-clients.
#[derive(Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Digest {
	/// All logs that have happened in the block.
	pub logs: Vec<Log>,
}

impl Slicable for Digest {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Vec::<Log>::decode(input).map(|logs| Digest { logs })
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		self.logs.using_encoded(f)
	}
}

/// The body of a block is just a bunch of transactions.
pub type Body = Vec<Transaction>;

/// A Substrate relay chain block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Block {
	/// The block header.
	pub header: Header,
	/// All relay-chain transactions.
	pub transactions: Body,
}

impl Slicable for Block {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Block {
			header: try_opt!(Slicable::decode(input)),
			transactions: try_opt!(Slicable::decode(input)),
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		v.extend(self.header.encode());
		v.extend(self.transactions.encode());

		v
	}
}

/// A relay chain block header.
///
/// https://github.com/w3f/polkadot-spec/blob/master/spec.md#header
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct Header {
	/// Block parent's hash.
	pub parent_hash: HeaderHash,
	/// Block number.
	pub number: Number,
	/// State root after this transition.
	pub state_root: H256,
	/// The root of the trie that represents this block's transactions, indexed by a 32-byte integer.
	pub transaction_root: H256,
	/// The digest of activity on the block.
	pub digest: Digest,
}

impl Header {
	/// Create a new instance with default fields except `number`, which is given as an argument.
	pub fn from_block_number(number: Number) -> Self {
		Header {
			parent_hash: Default::default(),
			number,
			state_root: Default::default(),
			transaction_root: Default::default(),
			digest: Default::default(),
		}
	}
}

impl Slicable for Header {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Header {
			parent_hash: try_opt!(Slicable::decode(input)),
			number: try_opt!(Slicable::decode(input)),
			state_root: try_opt!(Slicable::decode(input)),
			transaction_root: try_opt!(Slicable::decode(input)),
			digest: try_opt!(Slicable::decode(input)),
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		self.parent_hash.using_encoded(|s| v.extend(s));
		self.number.using_encoded(|s| v.extend(s));
		self.state_root.using_encoded(|s| v.extend(s));
		self.transaction_root.using_encoded(|s| v.extend(s));
		self.digest.using_encoded(|s| v.extend(s));

		v
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use codec::Slicable;
	use substrate_serializer as ser;

	#[test]
	fn test_header_encoding() {
		let header = Header {
			parent_hash: 5.into(),
			number: 67,
			state_root: 3.into(),
			transaction_root: 6.into(),
			digest: Digest { logs: vec![Log(vec![1]), Log(vec![2])] },
		};

		assert_eq!(header.encode(), vec![
			// parent_hash
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5,
			// number
			67, 0, 0, 0, 0, 0, 0, 0,
			// state_root
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3,
			// transaction_root
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6,
			// digest (length, log1, log2)
			2, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 2
		]);
	}

	#[test]
	fn test_header_serialization() {
		let header = Header {
			parent_hash: 5.into(),
			number: 67,
			state_root: 3.into(),
			transaction_root: 6.into(),
			digest: Digest { logs: vec![Log(vec![1])] },
		};

		assert_eq!(ser::to_string_pretty(&header), r#"{
  "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000005",
  "number": 67,
  "stateRoot": "0x0000000000000000000000000000000000000000000000000000000000000003",
  "transactionRoot": "0x0000000000000000000000000000000000000000000000000000000000000006",
  "digest": {
    "logs": [
      "0x01"
    ]
  }
}"#);

		let v = header.encode();
		assert_eq!(Header::decode(&mut &v[..]).unwrap(), header);
	}

	#[test]
	fn test_block_encoding() {
		let block = Block {
			header: Header::from_block_number(12),
			transactions: vec![Transaction(vec!(4))],
		};

		assert_eq!(block.encode(), vec![
			// parent_hash
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
			// number
			12, 0, 0, 0, 0, 0, 0, 0,
			// state_root
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
			// transaction_root
			0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
			// digest
			0, 0, 0, 0,
			// transactions (length, tx...)
			1, 0, 0, 0, 1, 0, 0, 0, 4
		]);
	}
}

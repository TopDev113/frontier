// Copyright 2017-2019 Parity Technologies (UK) Ltd.
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

//! Generic implementation of a digest.

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

use sp_std::prelude::*;

use crate::ConsensusEngineId;
use crate::codec::{Decode, Encode, Input, Error};
use sp_core::RuntimeDebug;

/// Generic header digest.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct Digest<Hash: Encode + Decode> {
	/// A list of logs in the digest.
	pub logs: Vec<DigestItem<Hash>>,
}

impl<Item: Encode + Decode> Default for Digest<Item> {
	fn default() -> Self {
		Digest { logs: Vec::new(), }
	}
}

impl<Hash: Encode + Decode> Digest<Hash> {
	/// Get reference to all digest items.
	pub fn logs(&self) -> &[DigestItem<Hash>] {
		&self.logs
	}

	/// Push new digest item.
	pub fn push(&mut self, item: DigestItem<Hash>) {
		self.logs.push(item);
	}

	/// Pop a digest item.
	pub fn pop(&mut self) -> Option<DigestItem<Hash>> {
		self.logs.pop()
	}

	/// Get reference to the first digest item that matches the passed predicate.
	pub fn log<T: ?Sized, F: Fn(&DigestItem<Hash>) -> Option<&T>>(&self, predicate: F) -> Option<&T> {
		self.logs().iter()
			.filter_map(predicate)
			.next()
	}

	/// Get a conversion of the first digest item that successfully converts using the function.
	pub fn convert_first<T, F: Fn(&DigestItem<Hash>) -> Option<T>>(&self, predicate: F) -> Option<T> {
		self.logs().iter()
			.filter_map(predicate)
			.next()
	}
}


/// Digest item that is able to encode/decode 'system' digest items and
/// provide opaque access to other items.
#[derive(PartialEq, Eq, Clone, RuntimeDebug)]
pub enum DigestItem<Hash> {
	/// System digest item that contains the root of changes trie at given
	/// block. It is created for every block iff runtime supports changes
	/// trie creation.
	ChangesTrieRoot(Hash),

	/// A pre-runtime digest.
	///
	/// These are messages from the consensus engine to the runtime, although
	/// the consensus engine can (and should) read them itself to avoid
	/// code and state duplication. It is erroneous for a runtime to produce
	/// these, but this is not (yet) checked.
	PreRuntime(ConsensusEngineId, Vec<u8>),

	/// A message from the runtime to the consensus engine. This should *never*
	/// be generated by the native code of any consensus engine, but this is not
	/// checked (yet).
	Consensus(ConsensusEngineId, Vec<u8>),

	/// Put a Seal on it. This is only used by native code, and is never seen
	/// by runtimes.
	Seal(ConsensusEngineId, Vec<u8>),

	/// Some other thing. Unsupported and experimental.
	Other(Vec<u8>),
}

#[cfg(feature = "std")]
impl<Hash: Encode> serde::Serialize for DigestItem<Hash> {
	fn serialize<S>(&self, seq: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		self.using_encoded(|bytes| {
			sp_core::bytes::serialize(bytes, seq)
		})
	}
}

#[cfg(feature = "std")]
impl<'a, Hash: Decode> serde::Deserialize<'a> for DigestItem<Hash> {
	fn deserialize<D>(de: D) -> Result<Self, D::Error> where
		D: serde::Deserializer<'a>,
	{
		let r = sp_core::bytes::deserialize(de)?;
		Decode::decode(&mut &r[..])
			.map_err(|e| serde::de::Error::custom(format!("Decode error: {}", e)))
	}
}

/// A 'referencing view' for digest item. Does not own its contents. Used by
/// final runtime implementations for encoding/decoding its log items.
#[derive(PartialEq, Eq, Clone, RuntimeDebug)]
pub enum DigestItemRef<'a, Hash: 'a> {
	/// Reference to `DigestItem::ChangesTrieRoot`.
	ChangesTrieRoot(&'a Hash),
	/// A pre-runtime digest.
	///
	/// These are messages from the consensus engine to the runtime, although
	/// the consensus engine can (and should) read them itself to avoid
	/// code and state duplication.  It is erroneous for a runtime to produce
	/// these, but this is not (yet) checked.
	PreRuntime(&'a ConsensusEngineId, &'a Vec<u8>),
	/// A message from the runtime to the consensus engine. This should *never*
	/// be generated by the native code of any consensus engine, but this is not
	/// checked (yet).
	Consensus(&'a ConsensusEngineId, &'a Vec<u8>),
	/// Put a Seal on it. This is only used by native code, and is never seen
	/// by runtimes.
	Seal(&'a ConsensusEngineId, &'a Vec<u8>),
	/// Any 'non-system' digest item, opaque to the native code.
	Other(&'a Vec<u8>),
}

/// Type of the digest item. Used to gain explicit control over `DigestItem` encoding
/// process. We need an explicit control, because final runtimes are encoding their own
/// digest items using `DigestItemRef` type and we can't auto-derive `Decode`
/// trait for `DigestItemRef`.
#[repr(u32)]
#[derive(Encode, Decode)]
pub enum DigestItemType {
	ChangesTrieRoot = 2,
	PreRuntime = 6,
	Consensus = 4,
	Seal = 5,
	Other = 0,
}

/// Type of a digest item that contains raw data; this also names the consensus engine ID where
/// applicable. Used to identify one or more digest items of interest.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum OpaqueDigestItemId<'a> {
	/// Type corresponding to DigestItem::PreRuntime.
	PreRuntime(&'a ConsensusEngineId),
	/// Type corresponding to DigestItem::Consensus.
	Consensus(&'a ConsensusEngineId),
	/// Type corresponding to DigestItem::Seal.
	Seal(&'a ConsensusEngineId),
	/// Some other (non-prescribed) type.
	Other,
}

impl<Hash> DigestItem<Hash> {
	/// Returns a 'referencing view' for this digest item.
	pub fn dref<'a>(&'a self) -> DigestItemRef<'a, Hash> {
		match *self {
			DigestItem::ChangesTrieRoot(ref v) => DigestItemRef::ChangesTrieRoot(v),
			DigestItem::PreRuntime(ref v, ref s) => DigestItemRef::PreRuntime(v, s),
			DigestItem::Consensus(ref v, ref s) => DigestItemRef::Consensus(v, s),
			DigestItem::Seal(ref v, ref s) => DigestItemRef::Seal(v, s),
			DigestItem::Other(ref v) => DigestItemRef::Other(v),
		}
	}

	/// Returns `Some` if the entry is the `ChangesTrieRoot` entry.
	pub fn as_changes_trie_root(&self) -> Option<&Hash> {
		self.dref().as_changes_trie_root()
	}

	/// Returns `Some` if this entry is the `PreRuntime` entry.
	pub fn as_pre_runtime(&self) -> Option<(ConsensusEngineId, &[u8])> {
		self.dref().as_pre_runtime()
	}

	/// Returns `Some` if this entry is the `Consensus` entry.
	pub fn as_consensus(&self) -> Option<(ConsensusEngineId, &[u8])> {
		self.dref().as_consensus()
	}

	/// Returns `Some` if this entry is the `Seal` entry.
	pub fn as_seal(&self) -> Option<(ConsensusEngineId, &[u8])> {
		self.dref().as_seal()
	}

	/// Returns Some if `self` is a `DigestItem::Other`.
	pub fn as_other(&self) -> Option<&[u8]> {
		match *self {
			DigestItem::Other(ref v) => Some(&v[..]),
			_ => None,
		}
	}

	/// Returns the opaque data contained in the item if `Some` if this entry has the id given.
	pub fn try_as_raw(&self, id: OpaqueDigestItemId) -> Option<&[u8]> {
		self.dref().try_as_raw(id)
	}

	/// Returns the data contained in the item if `Some` if this entry has the id given, decoded
	/// to the type provided `T`.
	pub fn try_to<T: Decode>(&self, id: OpaqueDigestItemId) -> Option<T> {
		self.dref().try_to::<T>(id)
	}
}

impl<Hash: Encode> Encode for DigestItem<Hash> {
	fn encode(&self) -> Vec<u8> {
		self.dref().encode()
	}
}

impl<Hash: Encode> codec::EncodeLike for DigestItem<Hash> {}

impl<Hash: Decode> Decode for DigestItem<Hash> {
	#[allow(deprecated)]
	fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
		let item_type: DigestItemType = Decode::decode(input)?;
		match item_type {
			DigestItemType::ChangesTrieRoot => Ok(DigestItem::ChangesTrieRoot(
				Decode::decode(input)?,
			)),
			DigestItemType::PreRuntime => {
				let vals: (ConsensusEngineId, Vec<u8>) = Decode::decode(input)?;
				Ok(DigestItem::PreRuntime(vals.0, vals.1))
			},
			DigestItemType::Consensus => {
				let vals: (ConsensusEngineId, Vec<u8>) = Decode::decode(input)?;
				Ok(DigestItem::Consensus(vals.0, vals.1))
			}
			DigestItemType::Seal => {
				let vals: (ConsensusEngineId, Vec<u8>) = Decode::decode(input)?;
				Ok(DigestItem::Seal(vals.0, vals.1))
			},
			DigestItemType::Other => Ok(DigestItem::Other(
				Decode::decode(input)?,
			)),
		}
	}
}

impl<'a, Hash> DigestItemRef<'a, Hash> {
	/// Cast this digest item into `ChangesTrieRoot`.
	pub fn as_changes_trie_root(&self) -> Option<&'a Hash> {
		match *self {
			DigestItemRef::ChangesTrieRoot(ref changes_trie_root) => Some(changes_trie_root),
			_ => None,
		}
	}

	/// Cast this digest item into `PreRuntime`
	pub fn as_pre_runtime(&self) -> Option<(ConsensusEngineId, &'a [u8])> {
		match *self {
			DigestItemRef::PreRuntime(consensus_engine_id, ref data) => Some((*consensus_engine_id, data)),
			_ => None,
		}
	}

	/// Cast this digest item into `Consensus`
	pub fn as_consensus(&self) -> Option<(ConsensusEngineId, &'a [u8])> {
		match *self {
			DigestItemRef::Consensus(consensus_engine_id, ref data) => Some((*consensus_engine_id, data)),
			_ => None,
		}
	}

	/// Cast this digest item into `Seal`
	pub fn as_seal(&self) -> Option<(ConsensusEngineId, &'a [u8])> {
		match *self {
			DigestItemRef::Seal(consensus_engine_id, ref data) => Some((*consensus_engine_id, data)),
			_ => None,
		}
	}

	/// Cast this digest item into `PreRuntime`
	pub fn as_other(&self) -> Option<&'a [u8]> {
		match *self {
			DigestItemRef::Other(ref data) => Some(data),
			_ => None,
		}
	}

	/// Try to match this digest item to the given opaque item identifier; if it matches, then
	/// return the opaque data it contains.
	pub fn try_as_raw(&self, id: OpaqueDigestItemId) -> Option<&'a [u8]> {
		match (id, self) {
			(OpaqueDigestItemId::Consensus(w), &DigestItemRef::Consensus(v, s)) |
				(OpaqueDigestItemId::Seal(w), &DigestItemRef::Seal(v, s)) |
				(OpaqueDigestItemId::PreRuntime(w), &DigestItemRef::PreRuntime(v, s))
				if v == w => Some(&s[..]),
			(OpaqueDigestItemId::Other, &DigestItemRef::Other(s)) => Some(&s[..]),
			_ => None,
		}
	}

	/// Try to match this digest item to the given opaque item identifier; if it matches, then
	/// try to cast to the given datatype; if that works, return it.
	pub fn try_to<T: Decode>(&self, id: OpaqueDigestItemId) -> Option<T> {
		self.try_as_raw(id).and_then(|mut x| Decode::decode(&mut x).ok())
	}
}

impl<'a, Hash: Encode> Encode for DigestItemRef<'a, Hash> {
	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		match *self {
			DigestItemRef::ChangesTrieRoot(changes_trie_root) => {
				DigestItemType::ChangesTrieRoot.encode_to(&mut v);
				changes_trie_root.encode_to(&mut v);
			},
			DigestItemRef::Consensus(val, data) => {
				DigestItemType::Consensus.encode_to(&mut v);
				(val, data).encode_to(&mut v);
			},
			DigestItemRef::Seal(val, sig) => {
				DigestItemType::Seal.encode_to(&mut v);
				(val, sig).encode_to(&mut v);
			},
			DigestItemRef::PreRuntime(val, data) => {
				DigestItemType::PreRuntime.encode_to(&mut v);
				(val, data).encode_to(&mut v);
			},
			DigestItemRef::Other(val) => {
				DigestItemType::Other.encode_to(&mut v);
				val.encode_to(&mut v);
			},
		}

		v
	}
}

impl<'a, Hash: Encode> codec::EncodeLike for DigestItemRef<'a, Hash> {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn should_serialize_digest() {
		let digest = Digest {
			logs: vec![
				DigestItem::ChangesTrieRoot(4),
				DigestItem::Other(vec![1, 2, 3]),
				DigestItem::Seal(*b"test", vec![1, 2, 3])
			],
		};

		assert_eq!(
			::serde_json::to_string(&digest).unwrap(),
			r#"{"logs":["0x0204000000","0x000c010203","0x05746573740c010203"]}"#
		);
	}
}
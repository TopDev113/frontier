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

//! A fixed hash type.

use serde::{Serialize, Serializer, Deserialize, Deserializer};

use bytes;

macro_rules! impl_serde {
	($name: ident, $len: expr) => {
		impl Serialize for $name {
			fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
				bytes::serialize(&self.0, serializer)
			}
		}

		impl<'de> Deserialize<'de> for $name {
			fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
				bytes::deserialize_check_len(deserializer, bytes::ExpectedLen::Exact($len))
					.map(|x| (&*x).into())
			}
		}
	}
}

impl_hash!(H160, 20);
impl_serde!(H160, 20);
impl_hash!(H256, 32);
impl_serde!(H256, 32);
impl_hash!(H520, 65);
impl_serde!(H520, 65);

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_serializer as ser;

	#[test]
	fn test_h160() {
		let tests = vec![
			(H160::from(0), "0x0000000000000000000000000000000000000000"),
			(H160::from(2), "0x0000000000000000000000000000000000000002"),
			(H160::from(15), "0x000000000000000000000000000000000000000f"),
			(H160::from(16), "0x0000000000000000000000000000000000000010"),
			(H160::from(1_000), "0x00000000000000000000000000000000000003e8"),
			(H160::from(100_000), "0x00000000000000000000000000000000000186a0"),
			(H160::from(u64::max_value()), "0x000000000000000000000000ffffffffffffffff"),
		];

		for (number, expected) in tests {
			assert_eq!(format!("{:?}", expected), ser::to_string_pretty(&number));
			assert_eq!(number, ser::from_str(&format!("{:?}", expected)).unwrap());
		}
	}

	#[test]
	fn test_h256() {
		let tests = vec![
			(H256::from(0), "0x0000000000000000000000000000000000000000000000000000000000000000"),
			(H256::from(2), "0x0000000000000000000000000000000000000000000000000000000000000002"),
			(H256::from(15), "0x000000000000000000000000000000000000000000000000000000000000000f"),
			(H256::from(16), "0x0000000000000000000000000000000000000000000000000000000000000010"),
			(H256::from(1_000), "0x00000000000000000000000000000000000000000000000000000000000003e8"),
			(H256::from(100_000), "0x00000000000000000000000000000000000000000000000000000000000186a0"),
			(H256::from(u64::max_value()), "0x000000000000000000000000000000000000000000000000ffffffffffffffff"),
		];

		for (number, expected) in tests {
			assert_eq!(format!("{:?}", expected), ser::to_string_pretty(&number));
			assert_eq!(number, ser::from_str(&format!("{:?}", expected)).unwrap());
		}
	}

	#[test]
	fn test_invalid() {
		assert!(ser::from_str::<H256>("\"0x000000000000000000000000000000000000000000000000000000000000000\"").unwrap_err().is_data());
		assert!(ser::from_str::<H256>("\"0x000000000000000000000000000000000000000000000000000000000000000g\"").unwrap_err().is_data());
		assert!(ser::from_str::<H256>("\"0x00000000000000000000000000000000000000000000000000000000000000000\"").unwrap_err().is_data());
		assert!(ser::from_str::<H256>("\"\"").unwrap_err().is_data());
		assert!(ser::from_str::<H256>("\"0\"").unwrap_err().is_data());
		assert!(ser::from_str::<H256>("\"10\"").unwrap_err().is_data());
	}
}

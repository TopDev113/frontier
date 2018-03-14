// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate Demo.

// Substrate Demo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate Demo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate Demo.  If not, see <http://www.gnu.org/licenses/>.

//! Transaction type.

use rstd::prelude::*;
use codec::{Input, Slicable, NonTrivialSlicable};
use {AccountId, SessionKey};

#[cfg(feature = "std")]
use std::fmt;

use block::Number as BlockNumber;

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[repr(u8)]
enum InternalFunctionId {
	SystemSetCode = 0x00,

	SessionSetLength = 0x10,
	SessionForceNewSession = 0x11,

	StakingSetSessionsPerEra = 0x20,
	StakingSetBondingDuration = 0x21,
	StakingSetValidatorCount = 0x22,
	StakingForceNewEra = 0x23,

	DemocracyCancelReferendum = 0x30,
	DemocracyStartReferendum = 0x31,

	CouncilSetDesiredSeats = 0x40,
	CouncilRemoveMember = 0x41,
	CouncilSetPresentationDuration = 0x42,
	CouncilSetTermDuration = 0x43,

	CouncilVoteSetCooloffPeriod = 0x50,
	CouncilVoteSetVotingPeriod = 0x51,
}

impl InternalFunctionId {
	/// Derive `Some` value from a `u8`, or `None` if it's invalid.
	fn from_u8(value: u8) -> Option<InternalFunctionId> {
		let functions = [
			InternalFunctionId::SystemSetCode,
			InternalFunctionId::SessionSetLength,
			InternalFunctionId::SessionForceNewSession,
			InternalFunctionId::StakingSetSessionsPerEra,
			InternalFunctionId::StakingSetBondingDuration,
			InternalFunctionId::StakingSetValidatorCount,
			InternalFunctionId::StakingForceNewEra,
			InternalFunctionId::DemocracyCancelReferendum,
			InternalFunctionId::DemocracyStartReferendum,
			InternalFunctionId::CouncilSetDesiredSeats,
			InternalFunctionId::CouncilRemoveMember,
			InternalFunctionId::CouncilSetPresentationDuration,
			InternalFunctionId::CouncilSetTermDuration,
			InternalFunctionId::CouncilVoteSetCooloffPeriod,
			InternalFunctionId::CouncilVoteSetVotingPeriod,
		];
		functions.iter().map(|&f| f).find(|&f| value == f as u8)
	}
}

/// A means of determining whether a referendum has gone through or not.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub enum VoteThreshold {
	/// A supermajority of approvals is needed to pass this vote.
	SuperMajorityApprove,
	/// A supermajority of rejects is needed to fail this vote.
	SuperMajorityAgainst,
	/// A simple majority of approvals is needed to pass this vote.
	SimpleMajority,
}

impl Slicable for VoteThreshold {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		u8::decode(input).and_then(|v| match v {
			0 => Some(VoteThreshold::SuperMajorityApprove),
			1 => Some(VoteThreshold::SuperMajorityAgainst),
			2 => Some(VoteThreshold::SimpleMajority),
			_ => None,
		})
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		match *self {
			VoteThreshold::SuperMajorityApprove => 0u8,
			VoteThreshold::SuperMajorityAgainst => 1u8,
			VoteThreshold::SimpleMajority => 2u8,
		}.using_encoded(f)
	}
}
impl NonTrivialSlicable for VoteThreshold {}

/// Internal functions that can be dispatched to.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[allow(missing_docs)]
pub enum Proposal {
	SystemSetCode(Vec<u8>),
	SessionSetLength(BlockNumber),
	SessionForceNewSession,
	StakingSetSessionsPerEra(BlockNumber),
	StakingSetBondingDuration(BlockNumber),
	StakingSetValidatorCount(u32),
	StakingForceNewEra,
	DemocracyStartReferendum(Box<Proposal>, VoteThreshold),
	DemocracyCancelReferendum(u32),
	CouncilSetDesiredSeats(u32),
	CouncilRemoveMember(AccountId),
	CouncilSetPresentationDuration(BlockNumber),
	CouncilSetTermDuration(BlockNumber),
	CouncilVoteSetCooloffPeriod(BlockNumber),
	CouncilVoteSetVotingPeriod(BlockNumber),
}

impl Slicable for Proposal {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		let id = u8::decode(input).and_then(InternalFunctionId::from_u8)?;
		let function = match id {
			InternalFunctionId::SystemSetCode =>
				Proposal::SystemSetCode(Slicable::decode(input)?),
			InternalFunctionId::SessionSetLength =>
				Proposal::SessionSetLength(Slicable::decode(input)?),
			InternalFunctionId::SessionForceNewSession =>
				Proposal::SessionForceNewSession,
			InternalFunctionId::StakingSetSessionsPerEra =>
				Proposal::StakingSetSessionsPerEra(Slicable::decode(input)?),
			InternalFunctionId::StakingSetBondingDuration =>
				Proposal::StakingSetBondingDuration(Slicable::decode(input)?),
			InternalFunctionId::StakingSetValidatorCount =>
				Proposal::StakingSetValidatorCount(Slicable::decode(input)?),
			InternalFunctionId::StakingForceNewEra =>
				Proposal::StakingForceNewEra,
			InternalFunctionId::DemocracyStartReferendum => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				Proposal::DemocracyStartReferendum(Box::new(a), b)
			}
			InternalFunctionId::DemocracyCancelReferendum =>
				Proposal::DemocracyCancelReferendum(Slicable::decode(input)?),
			InternalFunctionId::CouncilSetDesiredSeats =>
				Proposal::CouncilSetDesiredSeats(Slicable::decode(input)?),
			InternalFunctionId::CouncilRemoveMember =>
				Proposal::CouncilRemoveMember(Slicable::decode(input)?),
			InternalFunctionId::CouncilSetPresentationDuration =>
				Proposal::CouncilSetPresentationDuration(Slicable::decode(input)?),
			InternalFunctionId::CouncilSetTermDuration =>
				Proposal::CouncilSetTermDuration(Slicable::decode(input)?),
			InternalFunctionId::CouncilVoteSetCooloffPeriod =>
				Proposal::CouncilVoteSetCooloffPeriod(Slicable::decode(input)?),
			InternalFunctionId::CouncilVoteSetVotingPeriod =>
				Proposal::CouncilVoteSetVotingPeriod(Slicable::decode(input)?),
		};

		Some(function)
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();
		match *self {
			Proposal::SystemSetCode(ref data) => {
				(InternalFunctionId::SystemSetCode as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Proposal::SessionSetLength(ref data) => {
				(InternalFunctionId::SessionSetLength as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Proposal::SessionForceNewSession => {
				(InternalFunctionId::SessionForceNewSession as u8).using_encoded(|s| v.extend(s));
			}
			Proposal::StakingSetSessionsPerEra(ref data) => {
				(InternalFunctionId::StakingSetSessionsPerEra as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Proposal::StakingSetBondingDuration(ref data) => {
				(InternalFunctionId::StakingSetBondingDuration as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Proposal::StakingSetValidatorCount(ref data) => {
				(InternalFunctionId::StakingSetValidatorCount as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Proposal::StakingForceNewEra => {
				(InternalFunctionId::StakingForceNewEra as u8).using_encoded(|s| v.extend(s));
			}
			Proposal::DemocracyCancelReferendum(ref data) => {
				(InternalFunctionId::DemocracyCancelReferendum as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			_ => { unimplemented!() }
		}

		v
	}
}

/// Public functions that can be dispatched to.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[repr(u8)]
enum FunctionId {
	TimestampSet = 0x00,

	SessionSetKey = 0x10,

	StakingStake = 0x20,
	StakingUnstake = 0x21,
	StakingTransfer = 0x22,

	CouncilVotePropose = 0x30,
	CouncilVoteVote = 0x31,
	CouncilVoteVeto = 0x32,

	CouncilSetApprovals = 0x40,
	CouncilReapInactiveVoter = 0x41,
	CouncilRetractVoter = 0x42,
	CouncilSubmitCandidacy = 0x43,
	CouncilPresentWinner = 0x44,

	DemocracyPropose = 0x50,
	DemocracySecond = 0x51,
	DemocracyVote = 0x52,
}

impl FunctionId {
	/// Derive `Some` value from a `u8`, or `None` if it's invalid.
	fn from_u8(value: u8) -> Option<FunctionId> {
		use self::*;
		let functions = [FunctionId::StakingStake, FunctionId::StakingUnstake,
			FunctionId::StakingTransfer, FunctionId::SessionSetKey, FunctionId::TimestampSet,
			FunctionId::CouncilVotePropose, FunctionId::CouncilVoteVote, FunctionId::CouncilVoteVeto,
			FunctionId::CouncilSetApprovals, FunctionId::CouncilReapInactiveVoter,
			FunctionId::CouncilRetractVoter, FunctionId::CouncilSubmitCandidacy,
			FunctionId::CouncilPresentWinner, FunctionId::DemocracyPropose,
			FunctionId::DemocracySecond, FunctionId::DemocracyVote,
		];
		functions.iter().map(|&f| f).find(|&f| value == f as u8)
	}
}

/// Functions on the runtime.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[allow(missing_docs)]
pub enum Function {
	TimestampSet(u64),

	SessionSetKey(SessionKey),

	StakingStake,
	StakingUnstake,
	StakingTransfer(AccountId, u64),

	CouncilVotePropose(Proposal),
	CouncilVoteVote([u8; 32], bool),
	CouncilVoteVeto([u8; 32]),

	CouncilSetApprovals(Vec<bool>, u32),
	CouncilReapInactiveVoter(u32, AccountId, u32, u32),
	CouncilRetractVoter(u32),
	CouncilSubmitCandidacy(u32),
	CouncilPresentWinner(AccountId, u64, u32),

	DemocracyPropose(Proposal, u64),
	DemocracySecond(u32),
	DemocracyVote(u32, bool),
}

impl Slicable for Function {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		let id = u8::decode(input).and_then(FunctionId::from_u8)?;
		Some(match id {
			FunctionId::TimestampSet =>
				Function::TimestampSet(Slicable::decode(input)?),
			FunctionId::SessionSetKey =>
				Function::SessionSetKey(Slicable::decode(input)?),
			FunctionId::StakingStake => Function::StakingStake,
			FunctionId::StakingUnstake => Function::StakingUnstake,
			FunctionId::StakingTransfer => {
				let to = Slicable::decode(input)?;
				let amount = Slicable::decode(input)?;
				Function::StakingTransfer(to, amount)
			}
			FunctionId::CouncilVotePropose => Function::CouncilVotePropose(Slicable::decode(input)?),
			FunctionId::CouncilVoteVote => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				Function::CouncilVoteVote(a, b)
			}
			FunctionId::CouncilVoteVeto => Function::CouncilVoteVeto(Slicable::decode(input)?),
			FunctionId::CouncilSetApprovals => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				Function::CouncilSetApprovals(a, b)
			}
			FunctionId::CouncilReapInactiveVoter => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				let c = Slicable::decode(input)?;
				let d = Slicable::decode(input)?;
				Function::CouncilReapInactiveVoter(a, b, c, d)
			}
			FunctionId::CouncilRetractVoter => Function::CouncilRetractVoter(Slicable::decode(input)?),
			FunctionId::CouncilSubmitCandidacy => Function::CouncilSubmitCandidacy(Slicable::decode(input)?),
			FunctionId::CouncilPresentWinner => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				let c = Slicable::decode(input)?;
				Function::CouncilPresentWinner(a, b, c)
			}
			FunctionId::DemocracyPropose => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				Function::DemocracyPropose(a, b)
			}
			FunctionId::DemocracySecond => Function::DemocracySecond(Slicable::decode(input)?),
			FunctionId::DemocracyVote => {
				let a = Slicable::decode(input)?;
				let b = Slicable::decode(input)?;
				Function::DemocracyVote(a, b)
			}
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();
		match *self {
			Function::TimestampSet(ref data) => {
				(FunctionId::TimestampSet as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Function::SessionSetKey(ref data) => {
				(FunctionId::SessionSetKey as u8).using_encoded(|s| v.extend(s));
				data.using_encoded(|s| v.extend(s));
			}
			Function::StakingStake => {
				(FunctionId::StakingStake as u8).using_encoded(|s| v.extend(s));
			}
			Function::StakingUnstake => {
				(FunctionId::StakingUnstake as u8).using_encoded(|s| v.extend(s));
			}
			Function::StakingTransfer(ref to, ref amount) => {
				(FunctionId::StakingTransfer as u8).using_encoded(|s| v.extend(s));
				to.using_encoded(|s| v.extend(s));
				amount.using_encoded(|s| v.extend(s));
			}
			_ => { unimplemented!() }
		}

		v
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		f(self.encode().as_slice())
	}
}

/// A vetted and verified transaction from the external world.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Transaction {
	/// Who signed it (note this is not a signature).
	pub signed: super::AccountId,
	/// The number of transactions have come before from the same signer.
	pub nonce: super::TxOrder,
	/// The function that should be called.
	pub function: Function,
}

impl Slicable for Transaction {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		Some(Transaction {
			signed: try_opt!(Slicable::decode(input)),
			nonce: try_opt!(Slicable::decode(input)),
			function: try_opt!(Slicable::decode(input)),
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		self.signed.using_encoded(|s| v.extend(s));
		self.nonce.using_encoded(|s| v.extend(s));
		self.function.using_encoded(|s| v.extend(s));

		v
	}
}

impl ::codec::NonTrivialSlicable for Transaction {}

/// A transactions right from the external world. Unchecked.
#[derive(Eq, Clone)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct UncheckedTransaction {
	/// The actual transaction information.
	pub transaction: Transaction,
	/// The signature; should be an Ed25519 signature applied to the serialised `transaction` field.
	pub signature: super::Signature,
}

impl Slicable for UncheckedTransaction {
	fn decode<I: Input>(input: &mut I) -> Option<Self> {
		// This is a little more complicated than usual since the binary format must be compatible
		// with substrate's generic `Vec<u8>` type. Basically this just means accepting that there
		// will be a prefix of u32, which has the total number of bytes following (we don't need
		// to use this).
		let _length_do_not_remove_me_see_above: u32 = try_opt!(Slicable::decode(input));

		Some(UncheckedTransaction {
			transaction: try_opt!(Slicable::decode(input)),
			signature: try_opt!(Slicable::decode(input)),
		})
	}

	fn encode(&self) -> Vec<u8> {
		let mut v = Vec::new();

		// need to prefix with the total length as u32 to ensure it's binary comptible with
		// Vec<u8>. we'll make room for it here, then overwrite once we know the length.
		v.extend(&[0u8; 4]);

		self.transaction.signed.using_encoded(|s| v.extend(s));
		self.transaction.nonce.using_encoded(|s| v.extend(s));
		self.transaction.function.using_encoded(|s| v.extend(s));
		self.signature.using_encoded(|s| v.extend(s));

		let length = (v.len() - 4) as u32;
		length.using_encoded(|s| v[0..4].copy_from_slice(s));

		v
	}
}

impl ::codec::NonTrivialSlicable for UncheckedTransaction {}

impl PartialEq for UncheckedTransaction {
	fn eq(&self, other: &Self) -> bool {
		self.signature.iter().eq(other.signature.iter()) && self.transaction == other.transaction
	}
}

#[cfg(feature = "std")]
impl fmt::Debug for UncheckedTransaction {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "UncheckedTransaction({:?})", self.transaction)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use primitives;
	use ::codec::Slicable;
	use primitives::hexdisplay::HexDisplay;

	#[test]
	fn serialize_unchecked() {
		let tx = UncheckedTransaction {
			transaction: Transaction {
				signed: [1; 32],
				nonce: 999u64,
				function: Function::TimestampSet(135135),
			},
			signature: primitives::hash::H512([0; 64]),
		};
		// 71000000
		// 0101010101010101010101010101010101010101010101010101010101010101
		// e703000000000000
		// 00
		// df0f0200
		// 0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000

		let v = Slicable::encode(&tx);
		println!("{}", HexDisplay::from(&v));
		assert_eq!(UncheckedTransaction::decode(&mut &v[..]).unwrap(), tx);
	}
}

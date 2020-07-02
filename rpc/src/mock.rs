// This file is part of Substrate.

// Copyright (C) 2020 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Mock file

//use super::*;
use support::{
	parameter_types,
	weights::{Weight, constants::WEIGHT_PER_SECOND},
};
use sp_runtime::{
	traits::{IdentityLookup, Block as BlockT},
	testing::Header,
};


type AccountId = u64;
type AccountIndex = u32;
type BlockNumber = u64;
type Balance = u64;

parameter_types! {
	pub const MaximumBlockWeight: Weight = 2 * WEIGHT_PER_SECOND;
}

impl system::Trait for Test {
	type BaseCallFilter = ();
	type Origin = Origin;
	type Index = AccountIndex;
	type BlockNumber = BlockNumber;
	type Call = Call;
	type Hash = sp_core::H256;
	type Hashing = ::sp_runtime::traits::BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = sp_runtime::testing::Header;
	type Event = Event;
	type BlockHashCount = ();
	type MaximumBlockWeight = MaximumBlockWeight;
	type DbWeight = ();
	type AvailableBlockRatio = ();
	type MaximumBlockLength = ();
	type Version = ();
	type ModuleToIndex = ();
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = (Balances,);
	type BlockExecutionWeight = ();
	type ExtrinsicBaseWeight = ();
	type MaximumExtrinsicWeight = ();
}
parameter_types! {
	pub const ExistentialDeposit: Balance = 10;
}
impl pallet_balances::Trait for Test {
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
}

pub type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<u32, Call, u64, ()>;

support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: system::{Module, Call, Event<T>},
		Balances: pallet_balances::{Module, Call, Storage, Config<T>, Event<T>},
	}
);

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
	sp_io::TestExternalities::new(t)
}

// Copyright 2017-2020 Parity Technologies (UK) Ltd.
// This file is part of Frontier.

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

//! Test utilities

use super::*;
use crate::{Module, Trait, Block as EthereumBlock, Transaction as EthereumTransaction};
use ethereum::{TransactionAction, TransactionSignature};
use frame_support::{impl_outer_origin, parameter_types, weights::Weight};
use pallet_evm::{FeeCalculator, HashTruncateConvertAccountId};
use rlp::*;
use sp_core::{H160, H256, U256};
use sp_runtime::{
    generic::Header,
    create_runtime_str,
    testing::{Header as TestingHeader},
    traits::{BlakeTwo256, IdentityLookup, Block as BlockT},
    transaction_validity::{
		TransactionValidity, ValidTransaction, TransactionValidityError, InvalidTransaction,
		TransactionSource,
	},
    ModuleId, Perbill,
};
use sp_api::impl_runtime_apis;

impl_outer_origin! {
    pub enum Origin for Test where system = frame_system {}
}

// For testing the pallet, we construct most of a mock runtime. This means
// first constructing a configuration type (`Test`) which `impl`s each of the
// configuration traits of pallets we want to use.
#[derive(Clone, Eq, PartialEq)]
pub struct Test;
parameter_types! {
    pub const BlockHashCount: u64 = 250;
    pub const MaximumBlockWeight: Weight = 1024;
    pub const MaximumBlockLength: u32 = 2 * 1024;
    pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
}
impl frame_system::Trait for Test {
    type Origin = Origin;
    type Call = ();
    type Index = u64;
    type BlockNumber = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = H160;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Header = TestingHeader;
    type Event = ();
    type BlockHashCount = BlockHashCount;
    type MaximumBlockWeight = MaximumBlockWeight;
    type DbWeight = ();
    type BlockExecutionWeight = ();
    type ExtrinsicBaseWeight = ();
    type MaximumExtrinsicWeight = MaximumBlockWeight;
    type MaximumBlockLength = MaximumBlockLength;
    type AvailableBlockRatio = AvailableBlockRatio;
    type Version = ();
    type ModuleToIndex = ();
    type AccountData = pallet_balances::AccountData<u64>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
}

parameter_types! {
    pub const ExistentialDeposit: u64 = 500;
}

impl pallet_balances::Trait for Test {
    type Balance = u64;
    type Event = ();
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
}

parameter_types! {
    pub const MinimumPeriod: u64 = 6000 / 2;
}

impl pallet_timestamp::Trait for Test {
    type Moment = u64;
    type OnTimestampSet = ();
    type MinimumPeriod = MinimumPeriod;
}

pub struct FixedGasPrice;
impl FeeCalculator for FixedGasPrice {
    fn min_gas_price() -> U256 {
        1.into()
    }
}

parameter_types! {
    pub const TransactionByteFee: u64 = 1;
    pub const EVMModuleId: ModuleId = ModuleId(*b"py/evmpa");
}

impl pallet_evm::Trait for Test {
    type ModuleId = EVMModuleId;
    type FeeCalculator = FixedGasPrice;
    type ConvertAccountId = HashTruncateConvertAccountId<BlakeTwo256>;
    type Currency = Balances;
    type Event = ();
    type Precompiles = ();
}

impl Trait for Test {
    type Event = ();
}

pub type System = frame_system::Module<Test>;
pub type Balances = pallet_balances::Module<Test>;
pub type Ethereum = Module<Test>;
pub type Evm = pallet_evm::Module<Test>;

pub struct AccountInfo {
    pub address: H160,
    pub private_key: H256,
}

// This function basically just builds a genesis storage key/value store according to
// our desired mockup.
pub fn new_test_ext(accounts_len: usize) -> (Vec<AccountInfo>, sp_io::TestExternalities) {
    let ext = frame_system::GenesisConfig::default()
        .build_storage::<Test>()
        .unwrap()
        .into();

    let pairs = (0..accounts_len)
        .map(|i| {
            let private_key = H256::from_slice(&[(i + 1) as u8; 32]); //H256::from_low_u64_be((i + 1) as u64);
            let secret_key = secp256k1::SecretKey::parse_slice(&private_key[..]).unwrap();
            let public_key = secp256k1::PublicKey::from_secret_key(&secret_key);
            let address = H160::from(H256::from_slice(
                &Keccak256::digest(&public_key.serialize()[1..])[..],
            ));
            AccountInfo {
                private_key: private_key,
                address: address,
            }
        })
        .collect::<Vec<_>>();

    (pairs, ext)
}

pub fn contract_address(sender: H160, nonce: u64) -> H160 {
    let mut rlp = RlpStream::new_list(2);
    rlp.append(&sender);
    rlp.append(&nonce);

    H160::from_slice(&Keccak256::digest(rlp.out().as_slice())[12..])
}

pub fn storage_address(sender: H160, slot: H256) -> H256 {
    H256::from_slice(&Keccak256::digest(
        [&H256::from(sender)[..], &slot[..]].concat().as_slice(),
    ))
}

pub struct UnsignedTransaction {
    pub nonce: U256,
    pub gas_price: U256,
    pub gas_limit: U256,
    pub action: TransactionAction,
    pub value: U256,
    pub input: Vec<u8>,
}

impl UnsignedTransaction {
    fn signing_rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(9);
        s.append(&self.nonce);
        s.append(&self.gas_price);
        s.append(&self.gas_limit);
        s.append(&self.action);
        s.append(&self.value);
        s.append(&self.input);
        s.append(&42u8); // TODO: move this chain id into the frame ethereum configuration
        s.append(&0u8);
        s.append(&0u8);
    }

    fn signing_hash(&self) -> H256 {
        let mut stream = RlpStream::new();
        self.signing_rlp_append(&mut stream);
        H256::from_slice(&Keccak256::digest(&stream.drain()).as_slice())
    }

    pub fn sign(self, key: &H256) -> Transaction {
        let hash = self.signing_hash();
        let msg = {
            let mut a = [0u8; 32];
            for i in 0..32 {
                a[i] = hash[i];
            }
            secp256k1::Message::parse(&a)
        };
        let s = secp256k1::sign(&msg, &secp256k1::SecretKey::parse_slice(&key[..]).unwrap());
        let sig = s.0.serialize();

        let sig = TransactionSignature::new(
            0x78,
            H256::from_slice(&sig[0..32]),
            H256::from_slice(&sig[32..64]),
        )
        .unwrap();

        Transaction {
            nonce: self.nonce,
            gas_price: self.gas_price,
            gas_limit: self.gas_limit,
            action: self.action,
            value: self.value,
            input: self.input,
            signature: sig,
        }
    }
}

pub use pallet_evm::Account as EVMAccount;
use sp_version::RuntimeVersion;
use substrate_test_runtime_client::runtime::Block as TestBlock;

/// This runtime version.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("node-frontier-template"),
	impl_name: create_runtime_str!("node-frontier-template"),
	authoring_version: 1,
	spec_version: 1,
	impl_version: 1,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 1,
};

pub type AllModules = u64;
pub type Executive =
	frame_executive::Executive<Test, TestBlock, frame_system::ChainContext<Test>, Test, AllModules>;
// pub type BlockNumber = u32;
// pub type GenericHeader = sp_runtime::generic::Header<BlockNumber, BlakeTwo256>;
// pub type Block = sp_runtime::generic::Block<GenericHeader, sp_runtime::OpaqueExtrinsic>;

impl_runtime_apis! { 
	impl sp_api::Core<TestBlock> for Test {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: TestBlock) {
			//Executive::execute_block(block)
		}

		fn initialize_block(header: &<TestBlock as BlockT>::Header) {
			//Executive::initialize_block(header)
		}
	}

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<TestBlock> for Test {
        fn validate_transaction(
            _source: TransactionSource,
            utx: <TestBlock as BlockT>::Extrinsic,
        ) -> TransactionValidity {
            return Ok(ValidTransaction{
                priority: 0 as u64,
                requires: vec![],
                provides: vec![],
                longevity: 1,
                propagate: false,
            });
            // if let sp_runtime::OpaqueExtrinsic::IncludeData(data) = utx {
            //     return Ok(ValidTransaction{
            //         priority: data.len() as u64,
            //         requires: vec![],
            //         provides: vec![data],
            //         longevity: 1,
            //         propagate: false,
            //     });
            // }

            // frame_system::validate_transaction(utx)
        }
    }

	impl frontier_rpc_primitives::EthereumRuntimeApi<TestBlock> for Test {
		fn chain_id() -> u64 {
			0 as u64
		}

		fn account_basic(address: H160) -> EVMAccount {
			pallet_evm::Module::<Test>::accounts(address)
		}

		fn transaction_status(hash: H256) -> Option<frontier_rpc_primitives::TransactionStatus> {
			Ethereum::transaction_status(hash)
		}

		fn gas_price() -> U256 {
			U256::zero()
		}

		fn account_code_at(address: H160) -> Vec<u8> {
			pallet_evm::Module::<Test>::account_codes(address)
		}

		fn author() -> H160 {
			H160::default()
		}

		fn storage_at(address: H160, index: U256) -> H256 {
			H256::default()
		}

		fn block_by_number(number: u32) -> Option<EthereumBlock> {
			Ethereum::block_by_number(number.into())
		}

		fn block_transaction_count_by_number(number: u32) -> Option<U256> {
			None
		}

		fn block_transaction_count_by_hash(hash: H256) -> Option<U256> {
			None
		}

		fn block_by_hash(hash: H256) -> Option<EthereumBlock> {
			Ethereum::block_by_hash(hash)
		}

		fn transaction_by_hash(hash: H256) -> Option<(
			EthereumTransaction, 
			EthereumBlock, 
			TransactionStatus)> {
			Ethereum::transaction_by_hash(hash)
		}

		fn transaction_by_block_hash_and_index(hash: H256, index: u32) -> Option<(
			EthereumTransaction, 
			EthereumBlock, 
			TransactionStatus)> {
			Ethereum::transaction_by_block_hash_and_index(hash, index)
		}

		fn transaction_by_block_number_and_index(number: u32, index: u32) -> Option<(
			EthereumTransaction, 
			EthereumBlock, 
			TransactionStatus)> {
			Ethereum::transaction_by_block_number_and_index(
				number.into(), 
				index
			)
		}
	}
}

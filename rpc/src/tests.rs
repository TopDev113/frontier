use super::*;
use std::{sync::Arc};
use sp_core::{
    traits::BareCryptoStorePtr, testing::{KeyStore}
};
use sp_runtime::traits::{HashFor};
use sc_transaction_pool::{BasicPool, FullChainApi};
use frontier_rpc_primitives::{ConvertTransaction};
use substrate_test_runtime_client::{
    self, Backend, Client, LongestChain, runtime::{Block, Extrinsic},
    DefaultTestClientBuilderExt, TestClientBuilderExt, TestClientBuilder, Executor, LocalExecutor
};

use sc_service::client::{Client as ServiceClient, LocalCallExecutor};

type FullTransactionPool = BasicPool<
	FullChainApi<CustomClient<Backend>, Block>,
	Block,
>;

struct TestSetup {
	pub client: Arc<CustomClient<Backend>>,
	pub keystore: BareCryptoStorePtr,
	pub pool: Arc<FullTransactionPool>,
	pub is_authority: bool,
    pub convert_transaction: TransactionConverter,
    pub select_chain: LongestChain<Backend, Block>,
}

#[derive(Clone)]
struct TransactionConverter;
impl ConvertTransaction<Extrinsic> for TransactionConverter {
    fn convert_transaction(&self, _transaction: ethereum::Transaction) -> Extrinsic {
		Extrinsic::IncludeData(vec![])
	}
}

trait CustomDefaultTestClientBuilderExt: Sized {
	fn new_custom() -> Self;
}

impl CustomDefaultTestClientBuilderExt for TestClientBuilder<Executor, Backend> {
	fn new_custom() -> Self {
		TestClientBuilder::with_default_backend()
	}
}

pub type CustomClient<B> = ServiceClient<
	B,
	LocalCallExecutor<B, sc_executor::NativeExecutor<LocalExecutor>>,
	Block,
	pallet_ethereum::mock::RuntimeApi,
>;

pub trait CustomTestClientBuilderExt<B>: Sized {
	fn build_custom(self) -> (CustomClient<B>, sc_consensus::LongestChain<B, Block>);
}

impl<B> CustomTestClientBuilderExt<B> for TestClientBuilder<
	LocalCallExecutor<B, sc_executor::NativeExecutor<LocalExecutor>>,
	B
> where
	B: sc_client_api::backend::Backend<Block> + 'static,
	<B as sc_client_api::backend::Backend<Block>>::State:
		sp_api::StateBackend<HashFor<Block>>,
{
	fn build_custom(self) -> (CustomClient<B>, sc_consensus::LongestChain<B, Block>) {
		self.build_with_native_executor(None)
	}
}

impl Default for TestSetup {
	fn default() -> Self {
        let is_authority: bool = true;
        let convert_transaction = TransactionConverter;
		let keystore = KeyStore::new();
		// let client_builder = substrate_test_runtime_client::TestClientBuilder::new();
		// let (client, select_chain) = client_builder.set_keystore(keystore.clone()).build_with_longest_chain();
		let client_builder = substrate_test_runtime_client::TestClientBuilder::new_custom();
		let (client, select_chain) = client_builder.set_keystore(keystore.clone()).build_custom();

		let client_arc = Arc::new(client);

		let pool = Arc::new(BasicPool::new(
			Default::default(),
			Arc::new(FullChainApi::new(client_arc.clone())),
			None,
		).0);
		TestSetup {
            pool,
			client: client_arc,
			keystore,
            is_authority,
            convert_transaction,
            select_chain,
		}
	}
}

impl TestSetup {
	fn eth(&self) -> EthApi<
        Block, 
        CustomClient<Backend>, 
        LongestChain<Backend,Block>, 
        FullTransactionPool, 
        TransactionConverter, 
        Backend
    > {
		EthApi {
			pool: self.pool.clone(),
			client: self.client.clone(),
            select_chain: self.select_chain.clone(),
            convert_transaction: self.convert_transaction.clone(),
			is_authority: self.is_authority,
			_marker: PhantomData,
		}
	}
}
#[test]
fn hashrate_foo() {
	let env = TestSetup::default().eth();
	let call = env.hashrate().expect("Hashrate");
	println!("{:#?}", call);
	assert_eq!(1,1);
}

#[test]
fn header_foo() {
	let header = TestSetup::default().eth().select_chain.best_chain();
	assert_eq!(1,1);
}
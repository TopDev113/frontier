use jsonrpc_core::IoHandler;

extern crate sc_ethereum_rpc;
use sc_ethereum_rpc::{EthApiServer, EthRpcImpl};

macro_rules! rpc_test {
	// With parameters
	(
	  $test_name: ident,
	  $request: expr,
	  $response: expr,
    ) => {
		#[test]
		fn $test_name() {
			// given
			let mut handler = IoHandler::new();
			handler.extend_with(EthRpcImpl.to_delegate());

			assert_eq!(
				handler.handle_request_sync($request).unwrap(),
				$response.to_string()
			);
		}
	};
}

mod eth_rpc_server {
	use super::*;

	rpc_test!(
		protocol_version_0x54,
		r#"{"jsonrpc": "2.0","method":"eth_protocolVersion","id": 1}"#,
		r#"{"jsonrpc":"2.0","result":"0x54","id":1}"#,
	);
}

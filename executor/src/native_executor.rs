use std::panic::catch_unwind;
use primitives::contract::CallData;
use state_machine::{Externalities, CodeExecutor};
use error::{Error, ErrorKind, Result};
use wasm_executor::WasmExecutor;
use native_runtime as runtime;
use runtime_std;

pub struct NativeExecutor;

fn safe_call<F: ::std::panic::UnwindSafe + FnOnce() -> Vec<u8>>(f: F) -> Result<Vec<u8>> {
	catch_unwind(f).map_err(|_| ErrorKind::Runtime.into())
}

impl CodeExecutor for NativeExecutor {
	type Error = Error;

	fn call<E: Externalities>(
		&self,
		ext: &mut E,
		code: &[u8],
		method: &str,
		data: &CallData,
	) -> Result<Vec<u8>> {
		// WARNING!!! This assumes that the runtime was built *before* the main project. Until we
		// get a proper build script, this must be strictly adhered to or things will go wrong.
		let native_equivalent = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_polkadot.compact.wasm");
		if code == &native_equivalent[..] {
			runtime_std::with_externalities(ext, || match method {
				"execute_block" => safe_call(|| runtime::execute_block(&data.0)),
				"execute_transaction" => safe_call(|| runtime::execute_transaction(&data.0)),
				_ => Err(ErrorKind::MethodNotFound(method.to_owned()).into()),
			})
		} else {
			// call into wasm.
			WasmExecutor.call(ext, code, method, data)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use native_runtime::codec::KeyedVec;
	use native_runtime::support::{TestExternalities, one, two, StaticHexInto};
	use native_runtime::runtime::staking::balance;
	use primitives::twox_128;

	const BLOATY_CODE: &[u8] = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_polkadot.wasm");
	const COMPACT_CODE: &[u8] = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_polkadot.compact.wasm");
	fn tx() -> Vec<u8> { "679fcf0a846b4224c84ecad7d91a26241c46d00cb53d6480a363274e8965ee34b0b80b4b2e3836d3d8f8f12c0c1aef7350af587d9aee3883561d11726068ac0a2f8c6129d816cf51c374bc7f08c3e63ed156cf78aefb4a6550d97b87997977ee00000000000000000228000000d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a4500000000000000".convert() }

	#[test]
	fn panic_execution_with_foreign_code_gives_error() {
		let one = one();
		let mut t = TestExternalities { storage: map![
			twox_128(&one.to_keyed_vec(b"sta:bal:")).to_vec() => vec![68u8, 0, 0, 0, 0, 0, 0, 0]
		], };

		let r = NativeExecutor.call(&mut t, BLOATY_CODE, "execute_transaction", &CallData(tx()));
		assert!(r.is_err());
	}

	#[test]
	fn panic_execution_with_native_equivalent_code_gives_error() {
		let one = one();
		let mut t = TestExternalities { storage: map![
			twox_128(&one.to_keyed_vec(b"sta:bal:")).to_vec() => vec![68u8, 0, 0, 0, 0, 0, 0, 0]
		], };

		let r = NativeExecutor.call(&mut t, COMPACT_CODE, "execute_transaction", &CallData(tx()));
		assert!(r.is_err());
	}

	#[test]
	fn successful_execution_with_native_equivalent_code_gives_ok() {
		let one = one();
		let two = two();

		let mut t = TestExternalities { storage: map![
			twox_128(&one.to_keyed_vec(b"sta:bal:")).to_vec() => vec![111u8, 0, 0, 0, 0, 0, 0, 0]
		], };

		let r = NativeExecutor.call(&mut t, COMPACT_CODE, "execute_transaction", &CallData(tx()));
		assert!(r.is_ok());

		runtime_std::with_externalities(&mut t, || {
			assert_eq!(balance(&one), 42);
			assert_eq!(balance(&two), 69);
		});
	}

	#[test]
	fn successful_execution_with_foreign_code_gives_ok() {
		let one = one();
		let two = two();

		let mut t = TestExternalities { storage: map![
			twox_128(&one.to_keyed_vec(b"sta:bal:")).to_vec() => vec![111u8, 0, 0, 0, 0, 0, 0, 0]
		], };

		let r = NativeExecutor.call(&mut t, BLOATY_CODE, "execute_transaction", &CallData(tx()));
		assert!(r.is_ok());

		runtime_std::with_externalities(&mut t, || {
			assert_eq!(balance(&one), 42);
			assert_eq!(balance(&two), 69);
		});
	}
}

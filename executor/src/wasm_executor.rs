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

//! Rust implementation of Polkadot contracts.

use std::sync::Arc;
use std::collections::HashMap;
use parity_wasm::{deserialize_buffer, ModuleInstanceInterface, ProgramInstance};
use parity_wasm::interpreter::{ItemIndex};
use parity_wasm::RuntimeValue::{I32, I64};
use primitives::contract::CallData;
use state_machine::{Externalities, CodeExecutor};
use error::{Error, ErrorKind, Result};
use wasm_utils::{MemoryInstance, UserDefinedElements,
	AddModuleWithoutFullDependentInstance};
use primitives::{ed25519, blake2_256, twox_128, twox_256};

struct Heap {
	end: u32,
}

impl Heap {
	fn new() -> Self {
		Heap {
			end: 1024,
		}
	}
	fn allocate(&mut self, size: u32) -> u32 {
		let r = self.end;
		self.end += size;
		r
	}
	fn deallocate(&mut self, _offset: u32) {
	}
}

struct FunctionExecutor<'e, E: Externalities + 'e> {
	heap: Heap,
	memory: Arc<MemoryInstance>,
	ext: &'e mut E,
}

impl<'e, E: Externalities> FunctionExecutor<'e, E> {
	fn new(m: &Arc<MemoryInstance>, e: &'e mut E) -> Self {
		FunctionExecutor {
			heap: Heap::new(),
			memory: Arc::clone(m),
			ext: e,
		}
	}
}

trait WritePrimitive<T: Sized> {
	fn write_primitive(&self, offset: u32, t: T);
}

impl WritePrimitive<u32> for MemoryInstance {
	fn write_primitive(&self, offset: u32, t: u32) {
		use byteorder::{LittleEndian, ByteOrder};
		let mut r = [0u8; 4];
		LittleEndian::write_u32(&mut r, t);
		let _ = self.set(offset, &r);
	}
}

impl_function_executor!(this: FunctionExecutor<'e, E>,
	ext_print(utf8_data: *const u8, utf8_len: u32) => {
		if let Ok(utf8) = this.memory.get(utf8_data, utf8_len as usize) {
			if let Ok(message) = String::from_utf8(utf8) {
				println!("Runtime: {}", message);
			}
		}
	},
	ext_print_num(number: u64) => {
		println!("Runtime: {}", number);
	},
	ext_memcpy(dest: *mut u8, src: *const u8, count: usize) -> *mut u8 => {
		let _ = this.memory.copy_nonoverlapping(src as usize, dest as usize, count as usize);
		println!("memcpy {} from {}, {} bytes", dest, src, count);
		dest
	},
	ext_memmove(dest: *mut u8, src: *const u8, count: usize) -> *mut u8 => {
		let _ = this.memory.copy(src as usize, dest as usize, count as usize);
		println!("memmove {} from {}, {} bytes", dest, src, count);
		dest
	},
	ext_memset(dest: *mut u8, val: u32, count: usize) -> *mut u8 => {
		let _ = this.memory.clear(dest as usize, val as u8, count as usize);
		println!("memset {} with {}, {} bytes", dest, val, count);
		dest
	},
	ext_malloc(size: usize) -> *mut u8 => {
		let r = this.heap.allocate(size);
		println!("malloc {} bytes at {}", size, r);
		r
	},
	ext_free(addr: *mut u8) => {
		this.heap.deallocate(addr);
		println!("free {}", addr)
	},
	ext_set_storage(key_data: *const u8, key_len: u32, value_data: *const u8, value_len: u32) => {
		if let (Ok(key), Ok(value)) = (this.memory.get(key_data, key_len as usize), this.memory.get(value_data, value_len as usize)) {
			this.ext.set_storage(key, value);
		}
	},
	ext_get_allocated_storage(key_data: *const u8, key_len: u32, written_out: *mut u32) -> *mut u8 => {
		let (offset, written) = if let Ok(key) = this.memory.get(key_data, key_len as usize) {
			if let Ok(value) = this.ext.storage(&key) {
				let offset = this.heap.allocate(value.len() as u32) as u32;
				let _ = this.memory.set(offset, &value);
				(offset, value.len() as u32)
			} else { (0, 0) }
		} else { (0, 0) };

		this.memory.write_primitive(written_out, written);
		offset as u32
	},
	ext_get_storage_into(key_data: *const u8, key_len: u32, value_data: *mut u8, value_len: u32, value_offset: u32) -> u32 => {
		if let Ok(key) = this.memory.get(key_data, key_len as usize) {
			if let Ok(value) = this.ext.storage(&key) {
				let value = &value[value_offset as usize..];
				let written = ::std::cmp::min(value_len as usize, value.len());
				let _ = this.memory.set(value_data, &value[..written]);
				written as u32
			} else { 0 }
		} else { 0 }
	},
	ext_chain_id() -> u64 => {
		this.ext.chain_id()
	},
	ext_twox_128(data: *const u8, len: u32, out: *mut u8) => {
		let result =
			if let Ok(value) = this.memory.get(data, len as usize) {
				twox_128(&value)
			} else {
				[0; 16]
			};
		let _ = this.memory.set(out, &result);
	},
	ext_twox_256(data: *const u8, len: u32, out: *mut u8) => {
		let result =
			if let Ok(value) = this.memory.get(data, len as usize) {
				twox_256(&value)
			} else {
				[0; 32]
			};
		let _ = this.memory.set(out, &result);
	},
	ext_blake2_256(data: *const u8, len: u32, out: *mut u8) => {
		let result =
			if let Ok(value) = this.memory.get(data, len as usize) {
				blake2_256(&value)
			} else {
				[0; 32]
			};
		let _ = this.memory.set(out, &result);
	},
	ext_ed25519_verify(msg_data: *const u8, msg_len: u32, sig_data: *const u8, pubkey_data: *const u8) -> u32 => {
		(||{
			let mut sig = [0u8; 64];
			if let Err(_) = this.memory.get_into(sig_data, &mut sig[..]) {
				return 0;
			};
			let mut pubkey = [0u8; 32];
			if let Err(_) = this.memory.get_into(pubkey_data, &mut pubkey[..]) {
				return 0;
			};

			if let Ok(msg) = this.memory.get(msg_data, msg_len as usize) {
				if ed25519::Signature::from(sig).verify(&msg, &ed25519::Public::from(pubkey)) { 1 } else { 0 }
			} else {
				0
			}
		})()
	}
	=> <'e, E: Externalities + 'e>
);

/// Wasm rust executor for contracts.
///
/// Executes the provided code in a sandboxed wasm runtime.
#[derive(Debug, Default)]
pub struct WasmExecutor;

impl CodeExecutor for WasmExecutor {
	type Error = Error;

	fn call<E: Externalities>(
		&self,
		ext: &mut E,
		code: &[u8],
		method: &str,
		data: &CallData,
	) -> Result<Vec<u8>> {
		// TODO: handle all expects as errors to be returned.

		let program = ProgramInstance::new().expect("this really shouldn't be able to fail; qed");

		let module = deserialize_buffer(code.to_vec()).expect("all modules compiled with rustc are valid wasm code; qed");
		let module = program.add_module_by_sigs("test", module, map!["env" => FunctionExecutor::<E>::SIGNATURES]).expect("runtime signatures always provided; qed");

		let memory = module.memory(ItemIndex::Internal(0)).expect("all modules compiled with rustc include memory segments; qed");
		let mut fec = FunctionExecutor::new(&memory, ext);

		let size = data.0.len() as u32;
		let offset = fec.heap.allocate(size);
		memory.set(offset, &data.0).expect("heap always gives a sensible offset to write");

		let returned = program
				.params_with_external("env", &mut fec)
				.map(|p| p
					.add_argument(I32(offset as i32))
					.add_argument(I32(size as i32)))
			.and_then(|p| module.execute_export(method, p))
			.map_err(|_| -> Error { ErrorKind::Runtime.into() })?;

		if let Some(I64(r)) = returned {
			memory.get(r as u32, (r >> 32) as u32 as usize)
				.map_err(|_| ErrorKind::Runtime.into())
		} else {
			Err(ErrorKind::InvalidReturn.into())
		}
	}
}

#[cfg(test)]
mod tests {

	use super::*;
	use rustc_hex::FromHex;

	#[derive(Debug, Default)]
	struct TestExternalities {
		storage: HashMap<Vec<u8>, Vec<u8>>,
	}
	impl Externalities for TestExternalities {
		type Error = Error;

		fn storage(&self, key: &[u8]) -> Result<&[u8]> {
			Ok(self.storage.get(&key.to_vec()).map_or(&[] as &[u8], Vec::as_slice))
		}

		fn set_storage(&mut self, key: Vec<u8>, value: Vec<u8>) {
			self.storage.insert(key, value);
		}

		fn chain_id(&self) -> u64 { 42 }
	}

	#[test]
	fn storage_should_work() {
		let mut ext = TestExternalities::default();
		ext.set_storage(b"foo".to_vec(), b"bar".to_vec());
		let test_code = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_test.compact.wasm");

		let output = WasmExecutor.call(&mut ext, &test_code[..], "test_data_in", &CallData(b"Hello world".to_vec())).unwrap();

		assert_eq!(output, b"all ok!".to_vec());

		let expected: HashMap<_, _> = map![
			b"input".to_vec() => b"Hello world".to_vec(),
			b"foo".to_vec() => b"bar".to_vec(),
			b"baz".to_vec() => b"bar".to_vec()
		];
		assert_eq!(expected, ext.storage);
	}

	#[test]
	fn blake2_256_should_work() {
		let mut ext = TestExternalities::default();
		let test_code = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_test.compact.wasm");
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_blake2_256", &CallData(b"".to_vec())).unwrap(),
			FromHex::from_hex("0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8").unwrap()
		);
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_blake2_256", &CallData(b"Hello world!".to_vec())).unwrap(),
			FromHex::from_hex("3fbc092db9350757e2ab4f7ee9792bfcd2f5220ada5a4bc684487f60c6034369").unwrap()
		);
	}

	#[test]
	fn twox_256_should_work() {
		let mut ext = TestExternalities::default();
		let test_code = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_test.compact.wasm");
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_twox_256", &CallData(b"".to_vec())).unwrap(),
			FromHex::from_hex("99e9d85137db46ef4bbea33613baafd56f963c64b1f3685a4eb4abd67ff6203a").unwrap()
		);
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_twox_256", &CallData(b"Hello world!".to_vec())).unwrap(),
			FromHex::from_hex("b27dfd7f223f177f2a13647b533599af0c07f68bda23d96d059da2b451a35a74").unwrap()
		);
	}

	#[test]
	fn twox_128_should_work() {
		let mut ext = TestExternalities::default();
		let test_code = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_test.compact.wasm");
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_twox_128", &CallData(b"".to_vec())).unwrap(),
			FromHex::from_hex("99e9d85137db46ef4bbea33613baafd5").unwrap()
		);
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_twox_128", &CallData(b"Hello world!".to_vec())).unwrap(),
			FromHex::from_hex("b27dfd7f223f177f2a13647b533599af").unwrap()
		);
	}

	#[test]
	fn ed25519_verify_should_work() {
		let mut ext = TestExternalities::default();
		let test_code = include_bytes!("../../wasm-runtime/target/wasm32-unknown-unknown/release/runtime_test.compact.wasm");
		let key = ed25519::Pair::from_seed(&blake2_256(b"test"));
		let sig = key.sign(b"all ok!");
		let mut calldata = vec![];
		calldata.extend_from_slice(key.public().as_ref());
		calldata.extend_from_slice(sig.as_ref());
		assert_eq!(
			WasmExecutor.call(&mut ext, &test_code[..], "test_ed25519_verify", &CallData(calldata)).unwrap(),
			vec![1]
		);
	}
}

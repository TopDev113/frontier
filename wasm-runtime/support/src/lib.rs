#![no_std]
#![feature(lang_items)]
#![feature(alloc)]
#![cfg_attr(feature = "strict", deny(warnings))]

#![feature(alloc)]
extern crate alloc;

pub use alloc::vec;
pub use alloc::boxed;
pub use alloc::rc;
pub use core::mem;
pub use core::slice;
pub use core::cell;

/// Common re-exports that are useful to have in scope.
pub mod prelude {
	pub use alloc::vec::Vec;
	pub use alloc::boxed::Box;
}

use alloc::vec::Vec;

extern crate pwasm_libc;
extern crate pwasm_alloc;

#[lang = "panic_fmt"]
#[no_mangle]
pub fn panic_fmt() -> ! {
	  loop {}
}

extern "C" {
	fn ext_print(utf8_data: *const u8, utf8_len: u32);
	fn ext_print_num(value: u64);
	fn ext_set_storage(key_data: *const u8, key_len: u32, value_data: *const u8, value_len: u32);
	fn ext_get_allocated_storage(key_data: *const u8, key_len: u32, written_out: *mut u32) -> *mut u8;
	fn ext_get_storage_into(key_data: *const u8, key_len: u32, value_data: *mut u8, value_len: u32, value_offset: u32) -> u32;
	fn ext_chain_id() -> u64;
	fn ext_blake2_256(data: *const u8, len: u32, out: *mut u8);
	fn ext_twox_128(data: *const u8, len: u32, out: *mut u8);
	fn ext_twox_256(data: *const u8, len: u32, out: *mut u8);
	fn ext_ed25519_verify(msg_data: *const u8, msg_len: u32, sig_data: *const u8, pubkey_data: *const u8) -> u32;
}

pub fn storage(key: &[u8]) -> Vec<u8> {
	let mut length: u32 = 0;
	unsafe {
		let ptr = ext_get_allocated_storage(&key[0], key.len() as u32, &mut length);
		Vec::from_raw_parts(ptr, length as usize, length as usize)
	}
}

pub fn set_storage(key: &[u8], value: &[u8]) {
	unsafe {
		ext_set_storage(
			&key[0] as *const u8, key.len() as u32,
			&value[0] as *const u8, value.len() as u32
		);
	}
}

pub fn read_storage(key: &[u8], value_out: &mut [u8], value_offset: usize) -> usize {
	unsafe {
		ext_get_storage_into(&key[0], key.len() as u32, &mut value_out[0], value_out.len() as u32, value_offset as u32) as usize
	}
}

/// The current relay chain identifier.
pub fn chain_id() -> u64 {
	unsafe {
		ext_chain_id()
	}
}

/// Conduct a 256-bit Blake2 hash.
pub fn blake2_256(data: &[u8]) -> [u8; 32] {
	unsafe {
		let mut result: [u8; 32] = Default::default();
		// guaranteed to write into result.
		ext_blake2_256(&data[0], data.len() as u32, &mut result[0]);
		result
	}
}

/// Conduct four XX hashes to give a 256-bit result.
pub fn twox_256(data: &[u8]) -> [u8; 32] {
	unsafe {
		let mut result: [u8; 32] = Default::default();
		// guaranteed to write into result.
		ext_twox_256(&data[0], data.len() as u32, &mut result[0]);
		result
	}
}

/// Conduct two XX hashes to give a 256-bit result.
pub fn twox_128(data: &[u8]) -> [u8; 16] {
	unsafe {
		let mut result: [u8; 16] = Default::default();
		// guaranteed to write into result.
		ext_twox_128(&data[0], data.len() as u32, &mut result[0]);
		result
	}
}

/// Verify a ed25519 signature.
pub fn ed25519_verify(sig: &[u8], msg: &[u8], pubkey: &[u8]) -> bool {
	sig.len() != 64 || pubkey.len() != 32 || unsafe {
		ext_ed25519_verify(&msg[0], msg.len() as u32, &sig[0], &pubkey[0])
	} == 0
}

pub trait Printable {
	fn print(self);
}

impl<'a> Printable for &'a [u8] {
	fn print(self) {
		unsafe {
			ext_print(self.as_ptr(), self.len() as u32);
		}
	}
}

impl Printable for u64 {
	fn print(self) {
		unsafe { ext_print_num(self); }
	}
}

pub fn print<T: Printable + Sized>(value: T) {
	value.print();
}

#[macro_export]
macro_rules! impl_stubs {
	( $( $name:ident ),* ) => {
		pub mod _internal {
			$(
				#[no_mangle]
				pub fn $name(input_data: *mut u8, input_len: usize) -> u64 {
					let input = unsafe {
						$crate::vec::Vec::from_raw_parts(input_data, input_len, input_len)
					};

					let output = super::$name(input);
					&output[0] as *const u8 as u64 + ((output.len() as u64) << 32)
				}
			)*
		}
	}
}

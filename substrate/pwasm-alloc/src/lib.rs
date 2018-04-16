#![warn(missing_docs)]
#![cfg_attr(feature = "strict", deny(warnings))]
#![no_std]
#![crate_type = "rlib"]
#![cfg_attr(feature = "nightly", feature(global_allocator))]
#![cfg_attr(feature = "nightly", feature(alloc))]
#![cfg_attr(feature = "nightly", feature(allocator_api))]

//! Custom allocator crate for wasm

/// Wasm allocator
pub struct WasmAllocator;

#[cfg(feature = "nightly")]
#[global_allocator]
static ALLOCATOR: WasmAllocator = WasmAllocator;

#[cfg(feature = "nightly")]
mod __impl {
	extern crate alloc;
	extern crate pwasm_libc;

	use self::alloc::heap::{GlobalAlloc, Layout, Opaque};

	use super::WasmAllocator;

	unsafe impl GlobalAlloc for WasmAllocator {
		unsafe fn alloc(&self, layout: Layout) -> *mut Opaque {
			pwasm_libc::malloc(layout.size()) as *mut Opaque
		}

		unsafe fn dealloc(&self, ptr: *mut Opaque, _layout: Layout) {
			pwasm_libc::free(ptr as *mut u8)
		}
	}
}

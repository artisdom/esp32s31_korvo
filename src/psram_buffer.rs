//! Large application buffers explicitly use PSRAM, preserving internal RAM for DMA/radio.
extern crate alloc;
use alloc::{alloc::Layout, boxed::Box};
/// Prepared filter phases live outside the scarce internal DMA heap.
pub fn floats(len: usize) -> Box<[f32]> {
    if len == 0 {
        return Box::default();
    }
    let layout = Layout::array::<f32>(len).expect("float buffer layout");
    #[cfg(target_arch = "riscv32")]
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    #[cfg(not(target_arch = "riscv32"))]
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    unsafe {
        let ptr = ptr.cast::<f32>();
        ptr.write_bytes(0, len);
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len))
    }
}
pub fn zeroed(len: usize) -> Box<[u8]> {
    if len == 0 {
        return Box::default();
    }
    let layout = Layout::array::<u8>(len).expect("buffer layout");
    #[cfg(target_arch = "riscv32")]
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    #[cfg(not(target_arch = "riscv32"))]
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    // Box uses the same global heap to free the allocation, with its original layout.
    unsafe {
        ptr.write_bytes(0, len);
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len))
    }
}

/// Typed microphone/decoder allocations preserve their exact layout on free.
#[cfg(target_arch = "riscv32")]
pub fn samples(len: usize) -> Box<[i16]> {
    let layout = Layout::array::<i16>(len).expect("sample buffer layout");
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) }
            as *mut i16;
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    unsafe {
        ptr.write_bytes(0, len);
        Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len))
    }
}
#[cfg(target_arch = "riscv32")]
pub fn boxed<T>(value: T) -> Box<T> {
    let layout = Layout::new::<T>();
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) }
            as *mut T;
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    unsafe {
        ptr.write(value);
        Box::from_raw(ptr)
    }
}
#[cfg(not(target_arch = "riscv32"))]
pub fn samples(len: usize) -> Box<[i16]> {
    alloc::vec![0; len].into_boxed_slice()
}

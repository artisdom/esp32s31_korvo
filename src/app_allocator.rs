//! JPEG's private Vec allocations on core 1 must leave radio RAM available.
use core::alloc::{GlobalAlloc, Layout};
struct AppAllocator;
#[global_allocator]
static ALLOCATOR: AppAllocator = AppAllocator;
unsafe impl GlobalAlloc for AppAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if esp_hal::system::Cpu::current() == esp_hal::system::Cpu::AppCpu {
            // PSRAM and the writable PMA policy are established before core 1
            // starts. USB uses static DMA buffers; radio tasks run on core 0.
            unsafe {
                esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout)
            }
        } else {
            unsafe { GlobalAlloc::alloc(&esp_alloc::HEAP, layout) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Heap regions identify allocations by address, even if a decoded
        // image is freed on the core that did not allocate it.
        unsafe { GlobalAlloc::dealloc(&esp_alloc::HEAP, ptr, layout) }
    }
}

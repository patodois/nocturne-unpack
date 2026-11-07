//! Test-only allocation request counts for checking limits before reserving output.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static WATCH: Cell<usize> = const { Cell::new(usize::MAX) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

struct Probe;
fn note(size: usize) {
    let _ = WATCH.try_with(|watch| {
        if watch.get() == size {
            let _ = COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: each request is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Probe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        note(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Probe = Probe;

pub fn requests<R>(size: usize, f: impl FnOnce() -> R) -> (R, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            WATCH.with(|watch| watch.set(usize::MAX));
        }
    }
    WATCH.with(|watch| watch.set(size));
    COUNT.with(|count| count.set(0));
    let _reset = Reset;
    let result = f();
    (result, COUNT.with(Cell::get))
}

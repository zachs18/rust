//! Regression test fot #159923
#![feature(alloc_error_hook, thread_spawn_hook)]

use std::alloc::{GlobalAlloc, System};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static FAIL: AtomicBool = AtomicBool::new(false);
struct FailingGlobalAlloc;
unsafe impl GlobalAlloc for FailingGlobalAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if FAIL.swap(false, Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOC: FailingGlobalAlloc = FailingGlobalAlloc;

#[test]
fn check() {
    std::alloc::set_alloc_error_hook(|_| panic!());

    static COUNT: AtomicU32 = AtomicU32::new(0);

    // add any number of spawn hooks
    std::thread::add_spawn_hook(|_| || _ = COUNT.fetch_add(1, Ordering::Relaxed));
    std::thread::add_spawn_hook(|_| || _ = COUNT.fetch_add(1, Ordering::Relaxed));
    std::thread::add_spawn_hook(|_| || _ = COUNT.fetch_add(1, Ordering::Relaxed));

    std::thread::scope(|scope| {
        scope.spawn(|| {});
        scope.spawn(|| {});
    });
    std::thread::scope(|scope| {
        scope.spawn(|| {});
    });

    assert_eq!(COUNT.load(Ordering::Relaxed), 9);

    // remove hooks by making the allocation fail
    FAIL.store(true, Ordering::Relaxed);
    std::panic::catch_unwind(|| {
        std::thread::add_spawn_hook(|_| || {});
    })
    .unwrap_err();

    // gone :c
    std::thread::scope(|scope| {
        scope.spawn(|| {});
    });
    assert_eq!(COUNT.load(Ordering::Relaxed), 12); // 9 != 12
}

#![feature(ptr_metadata)]
use std::ptr::DynMetadata;

trait Trait {}
impl Trait for u32 {}

fn main() {
    let meta: DynMetadata<dyn Trait> = unsafe { std::mem::transmute(&[0_usize, 5, 6]) };
    //~^ ERROR: expected a vtable pointer
    dbg!(meta.size_of());
    dbg!(meta.align_of());
}

#![feature(ptr_metadata)]
use std::ptr::DynMetadata;

trait Trait {}
impl Trait for u32 {}

fn main() {
    let valid: DynMetadata<dyn Trait> = std::ptr::metadata(&42_u32 as &dyn Trait).vtable;
    let _: DynMetadata<()> = unsafe { std::mem::transmute(valid) };
    // this is allowed for `try_as_dyn` support
}

//@ check-pass
#![feature(ptr_metadata)]

use std::ptr::Thin;

trait Trait1 {}
trait Trait2 {}

fn cast(x: *const dyn Trait1) -> *const dyn Trait2 where for<'a> dyn Trait2: Thin {
    x as *const dyn Trait2
}

fn main() {}

#![feature(ptr_metadata)]
// Address issue #112737 -- ICE with dyn Pointee.
// Pointee doesn't exist anymore, so use Thin.
extern crate core;
use core::ptr::Thin;

fn raw_pointer_in(_: &dyn Thin) {}
//~^ ERROR `Thin` is not dyn compatible

fn main() {
    raw_pointer_in(&42)
    //~^ ERROR `Thin` is not dyn compatible
}

#![feature(ptr_metadata)]
// Address issue #112737 -- ICE with dyn Pointee.
// Pointee doesn't exist anymore, so use Thin.
extern crate core;
use core::ptr::Thin;

fn unknown_sized_object_ptr_in(_: &(impl Thin + ?Sized)) {}

fn raw_pointer_in(x: &dyn Thin) {
    //~^ ERROR the trait `Thin` is not dyn compatible
    unknown_sized_object_ptr_in(x)
    //~^ ERROR the trait bound `dyn Thin: Thin` is not satisfied
    //~| ERROR the trait `Thin` is not dyn compatible
}

fn main() {
    raw_pointer_in(&42)
    //~^ ERROR the trait `Thin` is not dyn compatible
}

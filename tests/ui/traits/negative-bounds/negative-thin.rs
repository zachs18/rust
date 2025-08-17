//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
#![feature(negative_bounds)]
#![feature(ptr_metadata)]

use std::ptr::Thin;

fn foo<T: ?Sized + !Thin>() {}

fn bar<T: !Sized + Thin>() {
    foo::<T>();
    //~^ ERROR the trait bound `T: !Thin` is not satisfied
}

fn main() {
    foo::<()>();
    //~^ ERROR the trait bound `(): !Thin` is not satisfied
    foo::<str>();
    //~^ ERROR the trait bound `str: !Thin` is not satisfied
}

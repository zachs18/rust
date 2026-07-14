//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
//@ check-fail
// TODO: make this compile

#![feature(rustc_attrs)]

// These are currently dyn-compatible.

#[rustc_dyn_compatible_trait]
trait Empty {}

#[rustc_dyn_compatible_trait]
trait Dispatchable {
    type Assoc;

    fn method(&self);

    fn explicitly_non_dispatchable_method(&self, b: &Self) where Self: Sized;

    fn explicitly_non_dispatchable_non_method(this: &Self, b: &Self) where Self: Sized;
}

// This is currently not dyn-compatible

#[rustc_dyn_compatible_trait]
//~^ ERROR is not dyn compatible
trait NewlyDispatchable {
    // TODO: make this compile
    fn dispatchable_non_method(this: &Self);
}

fn main() {
    let _: Option<&dyn Empty> = None;
    let _: Option<&dyn Dispatchable<Assoc = u32>> = None;

    let _: Option<&dyn NewlyDispatchable> = None;
    //~^ ERROR the trait `NewlyDispatchable` is not dyn compatible
}

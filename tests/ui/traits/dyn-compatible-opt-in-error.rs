//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
//@ check-fail

#![feature(rustc_attrs)]

#[rustc_dyn_compatible_trait]
//~^ ERROR is not dyn compatible
trait SizedSupertrait: Sized {}

#[rustc_dyn_compatible_trait]
//~^ ERROR is not dyn compatible
trait DynIncompatible1 {
    fn dyn_incompatible<T>(&self);
}

#[rustc_dyn_compatible_trait]
//~^ ERROR is not dyn compatible
trait DynIncompatible2 {
    fn dyn_incompatible(&self) where Self: std::fmt::Debug;
}

fn main() {}

//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
//@ check-pass
// TODO: add errors

#![feature(rustc_attrs)]


#[rustc_dyn_compatible_trait]
trait SizedSupertrait: Sized {}

#[rustc_dyn_compatible_trait]
trait DynIncompatible1 {
    fn dyn_incompatible<T>(&self);
}

#[rustc_dyn_compatible_trait]
trait DynIncompatible2 {
    fn dyn_incompatible(&self) where Self: std::fmt::Debug;
}

fn main() {}

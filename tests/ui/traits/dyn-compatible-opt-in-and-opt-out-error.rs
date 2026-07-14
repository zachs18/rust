//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
//@ check-fail

#![feature(rustc_attrs)]

#[rustc_dyn_compatible_trait]
//~^ NOTE trait was declared dyn-compatible here
#[rustc_dyn_incompatible_trait]
//~^ ERROR trait cannot be both
trait OptInAndOut {}

fn main() {}

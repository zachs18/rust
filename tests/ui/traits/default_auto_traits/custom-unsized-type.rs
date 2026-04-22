//@  compile-flags:  -Zexperimental-default-bounds
//@ revisions: current next
//@ [next] compile-flags: -Znext-solver

#![feature(auto_traits, lang_items, negative_impls, no_core, rustc_attrs, unsized_type)]
#![allow(incomplete_features)]
#![no_std]
#![no_core]

#[lang = "pointee_sized"]
trait PointeeSized {}

#[lang = "meta_sized"]
trait MetaSized: PointeeSized {}

#[lang = "sized"]
trait Sized: MetaSized {}

#[lang = "copy"]
pub trait Copy {}

#[lang = "default_trait1"]
auto trait Leak {}

// implicit T: Leak here
fn foo<T: PointeeSized>(_: &T) {}

mod unsized_type_leak {
    use crate::*;

    unsized type Opaque {}

    fn forward_extern_ty(x: &Opaque) {
        // ok, extern type leak by default
        crate::foo(x);
    }
}

mod unsized_type_non_leak {
    use crate::*;

    unsized type Opaque {}

    impl !Leak for Opaque {}
    fn forward_extern_ty(x: &Opaque) {
        foo(x);
        //~^ ERROR: the trait bound `unsized_type_non_leak::Opaque: Leak` is not satisfied
    }
}

fn main() {}

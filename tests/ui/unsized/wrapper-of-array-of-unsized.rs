//@ check-fail
//@ revisions: withfeature nofeature
// Regression test for #133966: previously the compiler ICEd due to assumptions that arrays are
// sized, and that struct with sized field is sized.
#![crate_type = "lib"]
#![cfg_attr(withfeature, feature(more_unsized))]

pub struct Data([[&'static str]; 5]);
//[nofeature]~^ ERROR size for values of type `[&'static str]` cannot be known at compilation time
const _: &'static Data = unsafe { &*(&[] as *const Data) };
//~^ ERROR casting `&[_; 0]` as `*const Data` is invalid

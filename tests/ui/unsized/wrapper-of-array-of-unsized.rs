//@ check-fail
// Regression test for #133966: previously the compiler ICEd due to assumptions that arrays are
// sized, and that struct with sized field is sized.
#![crate_type = "lib"]

pub struct Data([[&'static str]; 5]);
//~^ ERROR size for values of type `[&'static str]` cannot be known at compilation time
const _: &'static Data = unsafe { &*(&[] as *const Data) };
//~^ ERROR the type `{ptr metadata for [[&str]; 5]}` has an unknown layout

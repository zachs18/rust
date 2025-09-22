//! Regression test for <https://github.com/rust-lang/rust/issues/133966>
//@ revisions: withfeature nofeature
#![cfg_attr(withfeature, feature(more_unsized))]

pub struct Data([[&'static str]; 5_i32]);
//~^ ERROR mismatched types
//~| NOTE expected `usize`, found `i32`
//~| NOTE array length can only be `usize`
const _: &'static Data = unsafe { &*(&[] as *const Data) };
//~^ ERROR casting `&[_; 0]` as `*const Data` is invalid
fn main() {}

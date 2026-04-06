//@ known-bug: 999999
//@compile-flags: -Copt-level=3 --crate-type lib
#![feature(more_unsized)]
#![feature(ptr_metadata)]

use std::ptr::{self, Metadata};

pub fn foo() -> &'static [[u8]] {
    let x: &[[u8; 2]] = &[[1, 2]];
    let x: &[[u8]] = x;
    dbg!(ptr::metadata(x));
    x
}

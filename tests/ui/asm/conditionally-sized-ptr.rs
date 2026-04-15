//@ check-pass
//@ needs-asm-support
#![feature(ptr_metadata)] // for Thin

use std::arch::asm;
use std::ptr::Thin;

fn _f<T>(p: *mut T) {
    unsafe {
        asm!("/* {} */", in(reg) p);
    }
}

fn _g<T: ?Sized + Thin>(p: *mut T) {
    unsafe {
        asm!("/* {} */", in(reg) p);
    }
}

fn main() {}

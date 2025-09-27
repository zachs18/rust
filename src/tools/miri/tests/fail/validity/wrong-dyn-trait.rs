use std::{fmt, mem};

fn main() {
    let x: &dyn Send = &0;
    let _y: *const dyn fmt::Debug = unsafe { mem::transmute(x) }; //~ERROR: at .vtable, using vtable for `std::marker::Send` but `std::fmt::Debug` was expected
}

//@ normalize-stderr: "pref: Align\([1-8] bytes\)" -> "pref: $$PREF_ALIGN"
//@ normalize-stderr: "u[0-9]+ is" -> "u?? is"
//@ normalize-stderr: "pointer is 0..=[0-9]+" -> "pointer is 0..=$$PTR_MAX"
//@ normalize-stderr: "pointer is 1..=[0-9]+" -> "pointer is 1..=$$PTR_MAX"
//@ normalize-stderr: "b_offset: Size\([0-9]+ bytes\)" -> "b_offset: Size(? bytes)"

//! Enum layout tests related to scalar pairs with an int/ptr common primitive.

#![feature(rustc_attrs)]
#![crate_type = "lib"]

// Using the attribute twice to match on two parts of the output that are separated by a
// target-specific output.
// In `ScalarPair { a: u64 is 0..=1, b: pointer is whatever, b_offset: Size(8 bytes) }`,
// the `u64` and `8 bytes` depend on target pointer size.

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairOptPointerUnion {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: union pointer
    A,
    B(Option<Box<()>>),
}

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairPointerWithInt {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: pointer is 0..=
    A(usize),
    B(Box<()>),
}

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairPointerWithNonZeroInt {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: pointer is 0..=
    A(std::num::NonZeroUsize),
    B(Box<()>),
}

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairOptPointerWithNonZeroInt {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: pointer is 0..=
    A(std::num::NonZeroUsize),
    B(Option<Box<()>>),
}

#[repr(usize)]
enum OneUsize {
    One = 1,
}

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairPointerWithOneInt {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: pointer is 0..=
    A(OneUsize),
    B(Box<()>),
}

#[repr(usize)]
enum ZeroUsize {
    Zero = 0,
}

#[rustc_dump_layout(backend_repr)]
#[rustc_dump_layout(backend_repr)]
enum ScalarPairPointerWithZeroInt {
    //~^ ERROR: backend_repr: ScalarPair
    //~| ERROR: pointer is 0..
    A(ZeroUsize),
    B(Box<()>),
}

// Negative test--ensure that pointers are not commoned with integers
// of a different size. (Assumes that no target has 8 bit pointers, which
// feels pretty safe.)
#[rustc_dump_layout(backend_repr)]
enum NotScalarPairPointerWithSmallerInt { //~ERROR: backend_repr: Memory
    A(u8),
    B(Box<()>),
}

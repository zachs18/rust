// FIXME(ptr_metadata_v2): maybe move this back to GVN. Needed to move it
// because it now requires inlining and other opts to "see" that the comparisons can be folded.
// EMIT_MIR_FOR_EACH_PANIC_STRATEGY
//@ only-64bit

#![feature(rustc_attrs)]
#![feature(custom_mir)]
#![feature(core_intrinsics)]
#![feature(freeze)]
#![allow(ambiguous_wide_pointer_comparisons)]
#![allow(unconditional_panic)]
#![allow(unused)]

use std::mem::transmute;

/// Check that we do simplify when there is no provenance, and do not ICE.
fn wide_ptr_integer() {
    // CHECK-LABEL: fn wide_ptr_integer(

    let a: *const [u8] = unsafe { transmute((1usize, 1usize)) };
    let b: *const [u8] = unsafe { transmute((1usize, 2usize)) };

    // CHECK: opaque::<bool>(const false)
    opaque(a == b);
    // CHECK: opaque::<bool>(const true)
    opaque(a != b);
    // CHECK: opaque::<bool>(const true)
    opaque(a < b);
    // CHECK: opaque::<bool>(const true)
    opaque(a <= b);
    // CHECK: opaque::<bool>(const false)
    opaque(a > b);
    // CHECK: opaque::<bool>(const false)
    opaque(a >= b);
}

// CHECK-LABEL: fn main(
fn main() {
    wide_ptr_integer();
}

#[rustc_no_mir_inline]
fn opaque(_: impl Sized) {}

// EMIT_MIR wide_ptr_integer_comparisons.wide_ptr_integer.runtime-optimized.after.mir

//@ run-pass
#![feature(
    unsized_type,
    sized_hierarchy,
    min_specialization,
    arbitrary_self_types,
    ptr_alignment_type,
    ptr_metadata,
)]

use std::mem::Alignment;
use std::ptr::Metadata;

unsized type Foo {}

unsafe impl std::ptr::Thin for Foo {}

const ALIGN_TWO: Alignment = Alignment::new(2).unwrap();

unsafe impl std::marker::MetaSized for Foo {
    unsafe fn unchecked_size_for_meta(self: Metadata<Self>) -> usize { 0 }

    fn checked_size_for_meta(self: Metadata<Self>) -> Option<usize> { Some(0) }

    unsafe fn unchecked_align_for_meta(self: Metadata<Self>) -> Alignment { ALIGN_TWO }

    fn checked_align_for_meta(self: Metadata<Self>) -> Option<Alignment> { Some(ALIGN_TWO) }

    unsafe fn unchecked_layout_for_meta(self: Metadata<Self>) -> (usize, Alignment) {
        (0, ALIGN_TWO)
    }

    fn checked_layout_for_meta(self: Metadata<Self>) -> Option<(usize, Alignment)> {
        Some((0, ALIGN_TWO))
    }
}


#[repr(C)]
struct Struct {
    a: u8,
    b: Foo,
}

fn main() {
    let mut a: u32 = 42;
    let a: &mut Struct = unsafe {
        &mut *(&raw mut a as *mut Struct)
    };
    assert_eq!(a as *mut Struct as *mut u8, &raw mut a.a);
    assert_eq!((a as *mut Struct).wrapping_byte_add(2) as *mut Foo, &raw mut a.b);
    println!("{a:p}");
    println!("{:p}", &a.a);
    println!("{:p}", &a.b);
}

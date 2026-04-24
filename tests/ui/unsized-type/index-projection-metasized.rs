//@ run-pass
#![feature(
    unsized_type,
    sized_hierarchy,
    min_specialization,
    arbitrary_self_types,
    ptr_alignment_type,
    ptr_metadata,
    more_unsized,
)]

use std::mem::Alignment;
use std::ptr::Metadata;

unsized type Foo {}

unsafe impl std::ptr::Thin for Foo {}

const ALIGN_TWO: Alignment = Alignment::new(2).unwrap();

unsafe impl std::marker::MetaSized for Foo {
    unsafe fn unchecked_size_for_meta(self: Metadata<Self>) -> usize { 2 }

    fn checked_size_for_meta(self: Metadata<Self>) -> Option<usize> { Some(2) }

    unsafe fn unchecked_align_for_meta(self: Metadata<Self>) -> Alignment { ALIGN_TWO }

    fn checked_align_for_meta(self: Metadata<Self>) -> Option<Alignment> { Some(ALIGN_TWO) }

    unsafe fn unchecked_layout_for_meta(self: Metadata<Self>) -> (usize, Alignment) {
        (2, ALIGN_TWO)
    }

    fn checked_layout_for_meta(self: Metadata<Self>) -> Option<(usize, Alignment)> {
        Some((2, ALIGN_TWO))
    }
}

fn main() {
    let mut a: u32 = 42;
    let a: &mut [Foo; 2] = unsafe {
        &mut *(&raw mut a as *mut [Foo; 2])
    };
    assert_eq!(a as *mut [Foo; 2] as *mut Foo, &raw mut a[0]);
    assert_eq!((a as *mut [Foo; 2]).wrapping_byte_add(2) as *mut Foo, &raw mut a[1]);

    let a: &mut [Foo] = a;
    assert_eq!(a as *mut [Foo] as *mut Foo, &raw mut a[0]);
    assert_eq!((a as *mut [Foo]).wrapping_byte_add(2) as *mut Foo, &raw mut a[1]);
}

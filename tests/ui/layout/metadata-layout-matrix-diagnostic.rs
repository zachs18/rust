//@ run-pass

#![allow(dead_code, incomplete_features, internal_features)]
#![feature(arbitrary_self_types)]
#![feature(checked_layout_for_meta)]
#![feature(layout_for_meta)]
#![feature(min_specialization)]
#![feature(more_unsized)]
#![feature(offset_of_slice)]
#![feature(ptr_alignment_type)]
#![feature(ptr_metadata)]
#![feature(ptr_metadata_v2)]
#![feature(sized_hierarchy)]
#![feature(unsized_type)]

use std::alloc::Layout;
use std::marker::{MetaAligned, MetaSized, PointeeSized};
use std::mem::{self, Alignment};
use std::ptr::{self, Metadata, Thin, build_metadata};

/// Checks one valid matrix row against all six MetaSized queries and the
/// public checked wrappers. A factory is used so the test does not need to
/// assume that every generated Metadata<T> type implements Copy.
fn assert_valid<T, F>(make_meta: F, expected_size: usize, expected_align: usize)
where
    T: ?Sized + MetaSized,
    F: Fn() -> Metadata<T>,
{
    assert_eq!(mem::checked_size_for_meta::<T>(make_meta()), Some(expected_size));
    assert_eq!(mem::checked_align_for_meta::<T>(make_meta()), Some(expected_align));

    let public_layout = Layout::for_meta::<T>(make_meta()).expect("valid metadata was rejected");
    assert_eq!(public_layout.size(), expected_size);
    assert_eq!(public_layout.align(), expected_align);

    let checked_size = <T as MetaSized>::checked_size_for_meta(make_meta());
    let checked_align = <T as MetaAligned>::checked_align_for_meta(make_meta());
    let checked_layout = <T as MetaSized>::checked_layout_for_meta(make_meta());
    assert_eq!(checked_size, Some(expected_size));
    assert_eq!(checked_align.map(Alignment::as_usize), Some(expected_align));
    assert_eq!(
        checked_layout.map(|(size, align)| (size, align.as_usize())),
        Some((expected_size, expected_align)),
    );

    // Safe metadata establishes the precondition for all unchecked calls.
    let unchecked_size = unsafe { <T as MetaSized>::unchecked_size_for_meta(make_meta()) };
    let unchecked_align = unsafe { <T as MetaAligned>::unchecked_align_for_meta(make_meta()) };
    let unchecked_layout = unsafe { <T as MetaSized>::unchecked_layout_for_meta(make_meta()) };
    assert_eq!(unchecked_size, expected_size);
    assert_eq!(unchecked_align.as_usize(), expected_align);
    assert_eq!(
        (unchecked_layout.0, unchecked_layout.1.as_usize()),
        (expected_size, expected_align),
    );
}

/// Checks failure coherence. It intentionally never invokes an unchecked
/// query, because rejected metadata does not satisfy the unchecked precondition.
fn report_rejected<T, F>(label: &str, make_meta: F)
where
    T: ?Sized + MetaSized,
    F: Fn() -> Metadata<T>,
{
    let mem_size = mem::checked_size_for_meta::<T>(make_meta());
    let mem_align = mem::checked_align_for_meta::<T>(make_meta());
    let public_layout = Layout::for_meta::<T>(make_meta()).map(|l| (l.size(), l.align()));
    let trait_size = <T as MetaSized>::checked_size_for_meta(make_meta());
    let trait_align = <T as MetaAligned>::checked_align_for_meta(make_meta())
        .map(Alignment::as_usize);
    let trait_layout = <T as MetaSized>::checked_layout_for_meta(make_meta())
        .map(|(size, align)| (size, align.as_usize()));
    println!(
        "{label}: mem_size={mem_size:?}, mem_align={mem_align:?}, \
         public_layout={public_layout:?}, trait_size={trait_size:?}, \
         trait_align={trait_align:?}, trait_layout={trait_layout:?}"
    );
}

// A small custom metadata-sized type used to test implementor coherence.
unsized type FixedTwo {}

unsafe impl Thin for FixedTwo {}

const ALIGN_TWO: Alignment = Alignment::new(2).unwrap();

unsafe impl MetaAligned for FixedTwo {
    unsafe fn unchecked_align_for_meta(self: Metadata<Self>) -> Alignment { ALIGN_TWO }
    fn checked_align_for_meta(self: Metadata<Self>) -> Option<Alignment> { Some(ALIGN_TWO) }
}

unsafe impl MetaSized for FixedTwo {
    unsafe fn unchecked_size_for_meta(self: Metadata<Self>) -> usize { 2 }
    fn checked_size_for_meta(self: Metadata<Self>) -> Option<usize> { Some(2) }

    unsafe fn unchecked_layout_for_meta(self: Metadata<Self>) -> (usize, Alignment) {
        (2, ALIGN_TWO)
    }

    fn checked_layout_for_meta(self: Metadata<Self>) -> Option<(usize, Alignment)> {
        Some((2, ALIGN_TWO))
    }
}

struct PairOfStrs {
    a: str,
    b: str,
}

struct OffsetStruct<T: PointeeSized> {
    tag: u8,
    tail: T,
}

fn sized_baseline() {
    assert_valid::<u32, _>(|| build_metadata!(..), 4, 4);
}

fn strings_and_slice_boundaries() {
    assert_valid::<str, _>(|| build_metadata!(len: 0, ..), 0, 1);
    assert_valid::<str, _>(|| build_metadata!(len: 5, ..), 5, 1);

    assert_valid::<[u32], _>(|| build_metadata!(len: 0, ..), 0, 4);
    assert_valid::<[u32], _>(|| build_metadata!(len: 10, ..), 40, 4);

    let largest_representable_len = (isize::MAX as usize) / mem::size_of::<u32>();
    assert_valid::<[u32], _>(
        || build_metadata!(len: largest_representable_len, ..),
        largest_representable_len * mem::size_of::<u32>(),
        mem::align_of::<u32>(),
    );
}

fn overflow_is_rejected_without_calling_unchecked() {
    report_rejected::<str, _>("M5-str-usize-max", || build_metadata!(len: usize::MAX, ..));
    report_rejected::<[u32], _>("M5-slice-u32-usize-max", || {
        build_metadata!(len: usize::MAX, ..)
    });
}

fn recursively_nested_metadata() {
    let value: [[u8; 3]; 2] = [[0, 1, 2], [3, 4, 5]];

    let as_array_of_slices: &[[u8]; 2] = &value;
    assert_valid::<[[u8]; 2], _>(|| ptr::metadata(as_array_of_slices), 6, 1);

    let as_slice_of_slices: &[[u8]] = &value;
    assert_valid::<[[u8]], _>(|| ptr::metadata(as_slice_of_slices), 6, 1);

    let as_slice_of_arrays: &[[u8; 3]] = &value;
    assert_valid::<[[u8; 3]], _>(|| ptr::metadata(as_slice_of_arrays), 6, 1);
}

fn aggregate_metadata_and_padding() {
    assert_valid::<PairOfStrs, _>(
        || build_metadata!(
            a: build_metadata!(len: 42, ..),
            b: build_metadata!(len: 37, ..),
            ..
        ),
        79,
        1,
    );

    // The nested slice has statically known alignment 4, so the `tail` field
    // is placed after three bytes of padding.
    assert_eq!(mem::offset_of!(OffsetStruct<[[u32]]>, tail), 4);
}

fn trait_object_coercion_preserves_layout() {
    let concrete = 42_u32;
    let object: &dyn std::fmt::Debug = &concrete;
    assert_valid::<dyn std::fmt::Debug, _>(|| ptr::metadata(object), 4, 4);
}

fn custom_type_and_recursive_stride() {
    assert_valid::<FixedTwo, _>(|| build_metadata!(..), 2, 2);
    assert_valid::<[FixedTwo; 2], _>(|| build_metadata!(..), 4, 2);
    assert_valid::<[FixedTwo], _>(|| build_metadata!(len: 2, ..), 4, 2);
}

fn main() {
    sized_baseline();
    println!("M1 PASS");
    strings_and_slice_boundaries();
    println!("M2--M4 PASS");
    recursively_nested_metadata();
    println!("M6--M8 PASS");
    aggregate_metadata_and_padding();
    println!("M9--M10 PASS");
    trait_object_coercion_preserves_layout();
    println!("M11 PASS");
    custom_type_and_recursive_stride();
    println!("M12--M13 PASS");
    overflow_is_rejected_without_calling_unchecked();
}

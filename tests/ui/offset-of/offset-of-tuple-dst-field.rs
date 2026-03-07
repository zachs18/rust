//@ revisions: offset_of_slice no_offset_of_slice
#![feature(extern_types, sized_hierarchy, more_unsized)]
#![cfg_attr(offset_of_slice, feature(offset_of_slice))]

use std::marker::{MetaSized, MetaAligned, Aligned, PointeeSized};
use std::mem::offset_of;

trait Trait {}

extern "C" {
    type Extern;
}

fn main() {
    // FIXME: special case: the last element of tuples is always laid out last,
    // so all other fields must be `Sized`
    offset_of!((u8, u8), 1);
    offset_of!(([u8], u8), 1); //~ ERROR the size for values of type

    // Sized tuple elements are always at a static offset
    offset_of!((u16, u8, ()), 1);
    offset_of!(([u16], u8, ()), 1);
    offset_of!((dyn Trait, u8, ()), 1);
    offset_of!((Extern, u8, ()), 1);

    // The field itself must be `Sized` (or `Aligned` under `feature(offset_of_slice)`).
    offset_of!((u16, u8, ()), 0);
    offset_of!(([u16], u8, ()), 0);
    //[no_offset_of_slice]~^ ERROR the size for values of type
    offset_of!((dyn Trait, u8, ()), 0);
    //[no_offset_of_slice]~^ ERROR the size for values of type
    //[offset_of_slice]~^^ ERROR the alignment for values of type
    offset_of!((Extern, u8, ()), 0);
    //[no_offset_of_slice]~^ ERROR the size for values of type
    //[offset_of_slice]~^^ ERROR the alignment for values of type
}

fn generic_sized<T>() {
    offset_of!((u8, T, ()), 1);
    offset_of!(([u8], T, ()), 1);
    offset_of!((dyn Trait, T, ()), 1);
    offset_of!((Extern, T, ()), 1);
}

fn generic_aligned_1<T: PointeeSized + Aligned>() {
    offset_of!((u8, T, ()), 1);
    //[no_offset_of_slice]~^ ERROR the size for values of type `T`
}
fn generic_aligned_2<T: PointeeSized + Aligned>() {
    offset_of!(([u8], T, ()), 1);
    //[offset_of_slice]~^ ERROR the size for values of type `[u8]`
    //[no_offset_of_slice]~^^ ERROR the size for values of type `T`
}
fn generic_aligned_3<T: PointeeSized + Aligned>() {
    offset_of!((dyn Trait, T, ()), 1);
    //[offset_of_slice]~^ ERROR the size for values of type `dyn Trait`
    //[no_offset_of_slice]~^^ ERROR the size for values of type `T`
}
fn generic_aligned_4<T: PointeeSized + Aligned>() {
    offset_of!((Extern, T, ()), 1);
    //[offset_of_slice]~^ ERROR the size for values of type `Extern`
    //[no_offset_of_slice]~^^ ERROR the size for values of type `T`
}


fn generic_metasized_1<T: MetaSized>() {
    offset_of!((u8, T, ()), 1);
    //[no_offset_of_slice]~^ ERROR the size for values of type `T`
    //[offset_of_slice]~^^ ERROR the alignment for values of type `T`
}
fn generic_metasized_2<T: MetaSized>() {
    offset_of!(([u8], T, ()), 1);
    //[no_offset_of_slice]~^ ERROR the size for values of type `T`
    //[offset_of_slice]~^^ ERROR the alignment for values of type `T`
    //[offset_of_slice]~| ERROR the size for values of type `[u8]`
}
fn generic_metasized_3<T: MetaSized>() {
    offset_of!((dyn Trait, T, ()), 1);
    //[no_offset_of_slice]~^ ERROR the size for values of type `T`
    //[offset_of_slice]~^^ ERROR the alignment for values of type `T`
    //[offset_of_slice]~| ERROR the size for values of type `dyn Trait`
}
fn generic_metasized_4<T: MetaSized>() {
    offset_of!((Extern, T, ()), 1);
    //[no_offset_of_slice]~^ ERROR the size for values of type `T`
    //[offset_of_slice]~^^ ERROR the alignment for values of type `T`
    //[offset_of_slice]~| ERROR the size for values of type `Extern`
}

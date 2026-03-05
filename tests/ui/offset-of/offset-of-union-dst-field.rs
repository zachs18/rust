//@ revisions: offset_of_slice no_offset_of_slice
#![feature(extern_types, sized_hierarchy, more_unsized)]
#![cfg_attr(offset_of_slice, feature(offset_of_slice))]

use std::marker::{MetaSized, Aligned, PointeeSized};
use std::mem::{ManuallyDrop, offset_of};

trait Trait {}

extern "C" {
    type Extern;
}

#[repr(C)]
union ReprCUnion<T: PointeeSized> {
    _other1: u32,
    _other2: ManuallyDrop<[u32]>,
    _other3: ManuallyDrop<dyn Trait>,
    _other4: ManuallyDrop<Extern>,
    val: ManuallyDrop<T>,
}

union ReprRustUnion<T: PointeeSized> {
    _other1: u32,
    _other2: ManuallyDrop<[u32]>,
    _other3: ManuallyDrop<dyn Trait>,
    _other4: ManuallyDrop<Extern>,
    val: ManuallyDrop<T>,
}

fn main() {
    // repr(C) union: no restrictions
    offset_of!(ReprCUnion<u32>, val);
    offset_of!(ReprCUnion<[u32]>, val);
    offset_of!(ReprCUnion<dyn Trait>, val);
    offset_of!(ReprCUnion<Extern>, val);

    // repr(Rust) union: field itself must be `Sized` (or `Aligned` under offset_of_slice)
    offset_of!(ReprRustUnion<u32>, val);
    offset_of!(ReprRustUnion<[u32]>, val);
    //[no_offset_of_slice]~^ ERROR the size
    offset_of!(ReprRustUnion<dyn Trait>, val);
    //[no_offset_of_slice]~^ ERROR the size
    //[offset_of_slice]~^^ ERROR the alignment
    offset_of!(ReprRustUnion<Extern>, val);
    //[no_offset_of_slice]~^ ERROR the size
    //[offset_of_slice]~^^ ERROR the alignment
}

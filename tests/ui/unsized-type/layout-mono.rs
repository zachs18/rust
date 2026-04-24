//@ build-pass
//! Check that monomorphizing `size_of_val::<&T>` doesn't try to monomorphize `<T as MetaSized>::*`.
#![feature(unsized_type)]

unsized type Foo {}

fn main() {
    let _ = size_of_val::<&'static Foo> as fn(_) -> usize;
}

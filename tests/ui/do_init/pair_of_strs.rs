//@ run-pass
#![feature(in_place_init)]
#![feature(more_unsized)]
#![feature(layout_for_meta)]
#![feature(ptr_metadata)]
#![feature(ptr_metadata_v2)]

use std::ptr::{Metadata, build_metadata};

struct PairOfStrs {
    a: str,
    b: str,
}

fn main() {
    // Constructing a pointer metadata value for an aggregate type.
    let m: Metadata<PairOfStrs> = build_metadata!(
        a: build_metadata!(len: 42, ..),
        b: build_metadata!(len: 37, ..),
        ..
    );

    // Fallible layout computation given a pointer metadata value.
    assert_eq!(
        std::mem::checked_size_for_meta(m),
        Some(42 + 37),
    );

    // Constructing an aggregate type using initializers.
    let pair: Box<PairOfStrs> = Box::build(do init struct PairOfStrs {
        // `&str` implements `Init<str>` by copying the string data,
        // so a string literal can be used as an initializer for `str`.
        a: "hello",
        b: "world",
    });

    assert_eq!(&pair.a, "hello");
    assert_eq!(&pair.b, "world");
}

//@ run-pass
#![feature(unsized_unions)]
use std::mem::{ManuallyDrop, size_of_val, align_of_val};

union W<T: ?Sized> {
    a: [u8; 9],
    #[allow(unused)]
    b: ManuallyDrop<T>,
}

union X<T: ?Sized> {
    a: [u64; 1],
    #[allow(unused)]
    b: ManuallyDrop<T>,
}

const _: () = assert!(
    align_of::<u8>() < align_of::<u64>(),
    "this test assumes alignof(u64) > 1, if that is not true on your platform, disable this test",
);

fn main() {
    // unsizable field is smaller but more-aligned than sized field
    let w: &W<[u64; 1]> = &W { a: [42; 9] };
    assert_eq!(size_of_val(w), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(w), align_of::<u64>());
    assert_eq!(unsafe { w.a }, [42; 9]);

    let w_slice: &W<[u64]> = w;
    assert_eq!(size_of_val(w_slice), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(w_slice), align_of::<u64>());
    assert_eq!(unsafe { w_slice.a }, [42; 9]);

    let w_dyn: &W<dyn std::fmt::Debug> = w;
    assert_eq!(size_of_val(w_dyn), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(w_dyn), align_of::<u64>());
    assert_eq!(unsafe { w_dyn.a }, [42; 9]);

    // unsizable field is larger and more-aligned than sized field
    let w: &W<[u64; 3]> = &W { a: [42; 9] };
    assert_eq!(size_of_val(w), 24);
    assert_eq!(align_of_val(w), align_of::<u64>());
    assert_eq!(unsafe { w.a }, [42; 9]);

    let w_slice: &W<[u64]> = w;
    assert_eq!(size_of_val(w_slice), 24);
    assert_eq!(align_of_val(w_slice), align_of::<u64>());
    assert_eq!(unsafe { w_slice.a }, [42; 9]);

    let w_dyn: &W<dyn std::fmt::Debug> = w;
    assert_eq!(size_of_val(w_dyn), 24);
    assert_eq!(align_of_val(w_dyn), align_of::<u64>());
    assert_eq!(unsafe { w_dyn.a }, [42; 9]);

    // unsizable field is smaller and less-aligned than sized field
    let x: &X<[u8; 1]> = &X { a: [42] };
    assert_eq!(size_of_val(x), 1_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x), align_of::<u64>());
    assert_eq!(unsafe { x.a }, [42]);

    let x_slice: &X<[u8]> = x;
    assert_eq!(size_of_val(x_slice), 1_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x_slice), align_of::<u64>());
    assert_eq!(unsafe { x_slice.a }, [42]);

    let x_dyn: &X<dyn std::fmt::Debug> = x;
    assert_eq!(size_of_val(x_dyn), 1_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x_dyn), align_of::<u64>());
    assert_eq!(unsafe { x_dyn.a }, [42]);

    // unsizable field is larger but less-aligned than sized field
    let x: &X<[u8; 9]> = &X { a: [42] };
    assert_eq!(size_of_val(x), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x), align_of::<u64>());
    assert_eq!(unsafe { x.a }, [42]);

    let x_slice: &X<[u8]> = x;
    assert_eq!(size_of_val(x_slice), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x_slice), align_of::<u64>());
    assert_eq!(unsafe { x_slice.a }, [42]);

    let x_dyn: &X<dyn std::fmt::Debug> = x;
    assert_eq!(size_of_val(x_dyn), 9_usize.next_multiple_of(align_of::<u64>()));
    assert_eq!(align_of_val(x_dyn), align_of::<u64>());
    assert_eq!(unsafe { x_dyn.a }, [42]);

}

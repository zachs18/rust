//@ run-pass
#![feature(in_place_init)]
#![feature(in_place_init_syntax)]
#![feature(default_field_values)]
#![feature(more_unsized)]
#![feature(rustc_attrs)]

#[derive(Debug, PartialEq)]
struct Basic {
    a: u8,
    b: u16 = 2,
    c: u32 = 3,
    d: u8,
}

fn basic_basic() {
    let bx: Box<Basic> = Box::build(core::init::do_init!(struct Basic {
        a: 1,
        b: 2,
        c: 3,
        d: 4,
    }));
    assert_eq!(*bx, Basic { a: 1, b: 2, c: 3, d: 4 });
}

fn basic_out_of_order() {
    let bx: Box<Basic> = Box::build(core::init::do_init!(struct Basic {
        c: 3,
        b: 2,
        a: 1,
        d: 4,
    }));
    assert_eq!(*bx, Basic { a: 1, b: 2, c: 3, d: 4 });
}

fn basic_with_defaults() {
    let bx: Box<Basic> = Box::build(core::init::do_init!(struct Basic {
        a: 1,
        d: 4,
        ..
    }));
    assert_eq!(*bx, Basic { a: 1, b: 2, c: 3, d: 4 });
}

fn basic_with_fru() {
    let bx: Box<Basic> = Box::build(core::init::do_init!(struct Basic {
        a: 1,
        d: 4,
        ..Basic { a: 11, b: 12, c: 13, d: 14 }
    }));
    assert_eq!(*bx, Basic { a: 1, b: 12, c: 13, d: 4 });
}


#[derive(Debug, PartialEq)]
struct BasicUnsized<T: ?Sized> {
    a: u8,
    b: i32 = 2,
    c: u16 = 3,
    d: T,
}

fn basic_unsized_basic() {
    let bx: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        a: 1,
        b: 2,
        c: 3,
        d: "hello, world",
    }));
    assert_eq!(bx.a, 1);
    assert_eq!(bx.b, 2);
    assert_eq!(bx.c, 3);
    assert_eq!(&bx.d, "hello, world");
}

fn basic_unsized_out_of_order() {
    let bx: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        d: "waow",
        c: 3,
        b: 2,
        a: 1,
    }));
    assert_eq!(bx.a, 1);
    assert_eq!(bx.b, 2);
    assert_eq!(bx.c, 3);
    assert_eq!(&bx.d, "waow");
}

fn basic_unsized_with_defaults() {
    let bx: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        a: 1,
        d: "foobar",
        ..
    }));
    assert_eq!(bx.a, 1);
    assert_eq!(bx.b, 2);
    assert_eq!(bx.c, 3);
    assert_eq!(&bx.d, "foobar");
}

fn basic_unsized_with_fru() {
    let src: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        a: 11,
        b: 12,
        c: 13,
        d: "src",
    }));
    let bx: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        b: 42,
        d: "bx",
        // This is allowed because only `Sized` fields are used
        ..*src
    }));
    assert_eq!(bx.a, 11);
    assert_eq!(bx.b, 42);
    assert_eq!(bx.c, 13);
    assert_eq!(&bx.d, "bx");

    // `src` was not moved out of because all the FRU fields were `Copy`
    assert_eq!(src.a, 11);
    assert_eq!(src.b, 12);
    assert_eq!(src.c, 13);
    assert_eq!(&src.d, "src");
}


#[derive(Debug, PartialEq)]
struct Unsized2<T: ?Sized> {
    a: u8,
    b: i32 = 2,
    c: String,
    d: T,
}
fn unsized_with_unsized_fru_move() {
    let src: Box<Unsized2<str>> = Box::build(core::init::do_init!(struct Unsized2 {
        a: 11,
        b: 12,
        c: String::from("hello"),
        d: "src",
    }));
    let bx: Box<Unsized2<str>> = Box::build(core::init::do_init!(struct Unsized2 {
        b: 42,
        // This is allowed because only `Sized` fields are used
        d: "bx",
        ..*src
    }));
    assert_eq!(bx.a, 11);
    assert_eq!(bx.b, 42);
    assert_eq!(bx.c, "hello");
    assert_eq!(&bx.d, "bx");

    // `src` was moved out of because `String` is not `Copy
}

#[repr(C)]
struct Complicated {
    array: [[usize]],
    name: str,
    callback: dyn Fn(usize, &mut Vec<usize>),
    this: std::rc::Weak<Self>,
}

fn complicated() {
    let rc = std::rc::Rc::build_cyclic(core::init::do_init!(struct Complicated {
        array: std::init::repeat_slice(
            std::init::repeat_slice(
                std::init::from_fn_with_arg(|(row, col): (usize, usize)| row * 10 + col),
                10,
            ),
            10,
        ),
        name: "complicated",
        _ (with ptr name): std::init::from_fn_with_arg(|p: *mut str| {
            unsafe {&mut *p}.make_ascii_uppercase();
        }),
        callback(with arg): std::init::unsize(
            std::init::from_fn_with_arg(|weak: &std::rc::Weak<Complicated>| {
                let this = weak.clone();
                move |n: usize, v: &mut Vec<usize>| {
                    v.push(n);
                    if n > 0 {
                        (this.upgrade().unwrap().callback)(n-1, v)
                    }
                }
            })
        ),
        this(with arg): std::init::from_fn_with_arg(Clone::clone)
    }));
    assert_eq!(&rc.name, "COMPLICATED");
    assert!(rc.array.iter().flatten().copied().eq(0..100));
    let mut v = vec![];
    (rc.callback)(3, &mut v);
    assert_eq!(v, [3, 2, 1, 0]);
}

fn main() {
    basic_basic();
    basic_out_of_order();
    basic_with_defaults();
    basic_with_fru();

    basic_unsized_basic();
    basic_unsized_out_of_order();
    basic_unsized_with_defaults();
    basic_unsized_with_fru();

    unsized_with_unsized_fru_move();

    complicated();
}

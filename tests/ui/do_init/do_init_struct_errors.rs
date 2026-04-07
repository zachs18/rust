#![feature(in_place_init)]
#![feature(in_place_init_syntax)]
#![feature(default_field_values)]

#[derive(Debug, PartialEq)]
struct BasicUnsized<T: ?Sized> {
    a: u8,
    b: i32 = 2,
    c: String,
    d: T,
}

fn basic_unsized_with_unsized_fru() {
    let src: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
        a: 11,
        b: 12,
        c: String::from("hello"),
        d: "src",
    }));
    let bx: Box<BasicUnsized<str>> = Box::build(core::init::do_init!(struct BasicUnsized {
                                 //~^ ERROR: the size for values of type `str`
                                 //~| NOTE: doesn't have a size
                                 //~| NOTE: initializer elements must
        b: 42,
        ..*src
        //~^ ERROR the size for values of type `str` cannot be known at compilation time
        //~| NOTE: doesn't have a size
        //~| NOTE: in `do init struct Struct { ..place }`, values moved from `place` must have a statically known size
    }));
    assert_eq!(bx.a, 11);
    assert_eq!(bx.b, 42);
    assert_eq!(bx.c, "hello");
    assert_eq!(&bx.d, "src");
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
    let _ = src;
}


fn main() {
    basic_unsized_with_unsized_fru();
    unsized_with_unsized_fru_move();
}

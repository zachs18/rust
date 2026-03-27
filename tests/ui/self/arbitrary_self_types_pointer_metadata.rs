//@ run-pass
#![feature(arbitrary_self_types_pointers)]
#![feature(ptr_metadata)]

use std::rc::Rc;
use std::ptr::Metadata;

#[repr(transparent)]
struct Foo(str);

impl Foo {
    fn foo(self: Metadata<Self>) -> Metadata<str> {
        self.0
    }

    fn complicated_2(self: Rc<Metadata<Self>>) -> Metadata<str> {
        (*self).0
    }
}

fn main() {
    let foo: &Foo = unsafe { std::mem::transmute("abc123") };
    let meta: Metadata<Foo> = std::ptr::metadata(foo);
    assert_eq!("abc123".len(), meta.foo().len );
    let rc = Rc::new(meta);
    assert_eq!("abc123".len(), rc.complicated_2().len);
}

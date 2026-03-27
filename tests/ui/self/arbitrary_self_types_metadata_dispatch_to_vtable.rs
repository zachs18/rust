//@ check-pass

#![feature(derive_coerce_pointee)]
#![feature(ptr_metadata)]
#![feature(arbitrary_self_types)]

use std::marker::CoercePointee;
use std::ops::Receiver;
use std::ptr::Metadata;

// `CoercePointee` isn't needed here, it's just a simpler
// (and more conceptual) way of deriving `DispatchFromDyn`.
// You could think of `MyDispatcher` as a smart pointer
// that just doesn't deref to its target type.
#[derive(CoercePointee)]
#[repr(transparent)]
struct MyDispatcher<T: ?Sized>(Metadata<T>);

impl<T: ?Sized> Receiver for MyDispatcher<T> {
    type Target = T;
}
struct Test;

trait Trait {
    fn test(self: MyDispatcher<Self>);
}

impl Trait for Test {
    fn test(self: MyDispatcher<Self>) {
        todo!()
    }
}
fn main() {
    MyDispatcher::<dyn Trait>(Metadata::<Test>::default()).test();
}

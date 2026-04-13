#![allow(unused)]
use std::mem::ManuallyDrop;

union Foo<T: ?Sized> {
    x: (),
    y: ManuallyDrop<T>,
    //~^ ERROR: the size for values of type `T` cannot
}

fn main() {
    let foo: Foo<[u32; 3]> = Foo { y: ManuallyDrop::new([1, 2, 3]) };
    let p: &Foo<[u32]> = &foo;
}

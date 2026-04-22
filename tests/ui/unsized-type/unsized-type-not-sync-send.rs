// Make sure extern types are !Sync and !Send.

#![feature(unsized_type, sized_hierarchy)]

use std::marker::{PhantomData, PointeeSized};

unsized type A {}

unsized type B<T> {
    _phantom: PhantomData<fn() -> T>,
}

unsafe impl<T: Send> Send for B<T> {}
unsafe impl<T: Sync> Sync for B<T> {}

fn assert_sync<T: PointeeSized + Sync>() {}
fn assert_send<T: PointeeSized + Send>() {}

fn main() {
    assert_sync::<A>();
    //~^ ERROR `A` cannot be shared between threads safely [E0277]

    assert_send::<A>();
    //~^ ERROR `A` cannot be sent between threads safely [E0277]

    assert_sync::<B<u32>>();
    assert_send::<B<u32>>();

    assert_sync::<B<*const u32>>();
    //~^ ERROR `*const u32` cannot be shared between threads safely [E0277]

    assert_send::<B<*const u32>>();
    //~^ ERROR `*const u32` cannot be sent between threads safely [E0277]
}

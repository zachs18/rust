//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
//@ run-pass
// run it to make sure vtable dispatch works for non-methods

#![feature(rustc_attrs)]
#![feature(unsized_fn_params)]

// These are currently dyn-compatible on stable.

#[allow(unused)]
#[rustc_dyn_compatible_trait]
trait Empty {}

#[allow(unused)]
#[rustc_dyn_compatible_trait]
trait Dispatchable {
    type Assoc;

    fn method(&self);

    fn explicitly_non_dispatchable_method(&self, b: &Self) where Self: Sized;

    fn explicitly_non_dispatchable_non_method(this: &Self, b: &Self) where Self: Sized;
}

// This is newly dyn-compatible

#[rustc_dyn_compatible_trait]
trait NewlyDispatchable {
    fn dispatchable_non_method(this: &Self) -> usize;

    fn dispatchable_on_raw_ptr(this: *const Self) -> usize;

    fn dispatchable_on_value(this: Self) -> usize; // with unsized_fn_params
}

impl<T> NewlyDispatchable for T {
    fn dispatchable_non_method(_this: &Self) -> usize {
        size_of::<T>()
    }

    fn dispatchable_on_raw_ptr(_this: *const Self) -> usize {
        size_of::<T>()
    }

    fn dispatchable_on_value(_this: Self) -> usize {
        size_of::<T>()
    }
}


fn main() {
    let _: Option<&dyn Empty> = None;
    let _: Option<&dyn Dispatchable<Assoc = u32>> = None;

    let _: Option<&dyn NewlyDispatchable> = None;

    assert_eq!(<dyn NewlyDispatchable>::dispatchable_non_method(&42u64), size_of::<u64>());
    assert_eq!(<dyn NewlyDispatchable>::dispatchable_on_raw_ptr(&42u32), size_of::<u32>());
    assert_eq!(
        <dyn NewlyDispatchable>::dispatchable_on_value(
            *(Box::new(42u16) as Box<dyn NewlyDispatchable>)
        ),
        size_of::<u16>(),
    );
}

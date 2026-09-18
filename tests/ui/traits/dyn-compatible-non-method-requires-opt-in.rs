//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver
// testing that `#[rustc_dyn_compatible_trait]` is required for non-methods to be dispatchable.

trait NonDispatchable {
    fn non_method(this: &Self) -> usize;
}

impl<T> NonDispatchable for T {
    fn non_method(_this: &Self) -> usize {
        size_of::<T>()
    }

}


fn main() {
    let _: Option<&dyn NonDispatchable> = None;
    //~^ ERROR is not dyn compatible
}

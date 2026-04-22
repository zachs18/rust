#![feature(unsized_type, sized_hierarchy)]

unsized type B<T> {}
//~^ ERROR: type parameter `T` is never used

fn main() {}

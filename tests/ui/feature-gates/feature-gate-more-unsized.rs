fn foo_1(_: &[[u8]]) {}
//~^ ERROR cannot be known at compilation time
//~| NOTE doesn't have a size known at compile-time
//~| HELP the trait `Sized` is not implemented for `[u8]`
//~| NOTE slice

fn foo_2(_: &[[u8]; 3]) {}
//~^ ERROR cannot be known at compilation time
//~| NOTE doesn't have a size known at compile-time
//~| HELP the trait `Sized` is not implemented for `[u8]`
//~| NOTE array

fn foo_3(_: &([u8], u32)) {}
//~^ ERROR cannot be known at compilation time
//~| NOTE doesn't have a size known at compile-time
//~| HELP the trait `Sized` is not implemented for `[u8]`
//~| NOTE tuple

fn main() {}

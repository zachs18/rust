// Checks that the compiler complains about the missing closure body and does not
// crash.
// This is a regression test for <https://github.com/rust-lang/rust/issues/143128>.

fn foo() { |b: [str; _]| {}; }
//~^ ERROR the size for values of type `str` cannot be known at compilation time

fn bar() { |b: [u8; _]| {}; }
//~^ ERROR type annotations needed

fn main() {}

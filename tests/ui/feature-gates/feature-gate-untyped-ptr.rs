#![feature(builtin_syntax)]

type T = builtin # untyped_ptr(nullable); //~ ERROR: the untyped pointer type is unstable
type U = builtin # untyped_ptr(nonnull); //~ ERROR: the untyped pointer type is unstable

fn main() {}

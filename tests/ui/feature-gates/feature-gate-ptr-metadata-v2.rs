#![feature(builtin_syntax)]

type T = builtin # ptr_metadata([u32]); //~ ERROR: the pointer metadata type is unstable

fn main() {}

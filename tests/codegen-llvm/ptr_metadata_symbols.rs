//! Check that symbol names with pattern types in them are
//! different from the same symbol with the base type

//@ compile-flags: -Csymbol-mangling-version=v0 -Copt-level=0 --crate-type=lib

#![feature(ptr_metadata_v2)]
#![feature(builtin_syntax)]

macro_rules! builtin { // tidy doesn't recognize builtin # ptr_metadata(T) syntax
    ($($rest:tt)*) => { builtin # $($rest)* };
}

type MetadataU32 = builtin!(ptr_metadata(u32));

fn foo<T>() {}

pub fn bar() {
    // CHECK: call ptr_metadata_symbols::foo::<*mut u32>
    // CHECK: call void @_RINvC[[CRATE_IDENT:[a-zA-Z0-9]{12}]]_20ptr_metadata_symbols3fooOmEB2_
    foo::<*mut u32>();
    // CHECK: call ptr_metadata_symbols::foo::<{ptr metadata for u32}>
    // CHECK: call void @_RINvC[[CRATE_IDENT]]_20ptr_metadata_symbols3fooHmEB2_
    foo::<MetadataU32>();
}

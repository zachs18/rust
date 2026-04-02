#![feature(builtin_syntax, ptr_metadata, ptr_metadata_v2)]

use std::ptr::DynMetadata;

fn thin_missing_dotdot<T>() -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata()
    //~^ ERROR: pointer metadata of `T` does not have known fields
    //~| NOTE: `T` is `Thin`, so you can use `..` default field syntax
}

fn thin_with_dotdot<T>() -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata(..)
}

fn thin_with_base<T>(base: builtin # ptr_metadata(T)) -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata(..base)
}

fn too_generic_no_dotdot<T: ?Sized>() -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata()
    //~^ ERROR: pointer metadata of `T` does not have known fields
}

fn too_generic_with_dotdot<T: ?Sized>() -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata(..)
    //~^ ERROR: pointer metadata of `T` does not have known fields
}

fn generic_with_base<T: ?Sized>(base: builtin # ptr_metadata(T)) -> builtin # ptr_metadata(T) {
    builtin # ptr_metadata(..base)
}

fn unknown_pointee_no_type_no_dotdot() {
    let _ = builtin # ptr_metadata();
    //~^ ERROR: type annotations needed
    //~| NOTE: cannot infer type
}

fn unknown_pointee_no_type_with_dotdot() {
    let _ = builtin # ptr_metadata(..);
    //~^ ERROR: type annotations needed
    //~| NOTE: cannot infer type
}

fn unknown_pointee_inferred_type_no_dotdot() {
    let _ = builtin # ptr_metadata(for _);
    //~^ ERROR: type annotations needed
    //~| NOTE: cannot infer type
}

fn unknown_pointee_inferred_type_with_dotdot() {
    let _ = builtin # ptr_metadata(for _; ..);
    //~^ ERROR: type annotations needed
    //~| NOTE: cannot infer type
}

fn slice_metadata<T>(len: usize) -> builtin # ptr_metadata([T]) {
    builtin # ptr_metadata(len, ..)
}

fn slice_metadata_no_shorthand<T>() -> builtin # ptr_metadata([T]) {
    builtin # ptr_metadata(len: 42, ..)
}

fn slice_metadata_annotated_type() -> builtin # ptr_metadata([impl Sized]) {
    builtin # ptr_metadata(for [u32]; len: 42, ..)
}

fn str_metadata_without_dotdot<T>(len: usize) -> builtin # ptr_metadata(str) {
    builtin # ptr_metadata(len)
    //~^ ERROR: may have more fields added
    //~| NOTE: you can use `..`
}

fn str_metadata<T>(len: usize) -> builtin # ptr_metadata(str) {
    builtin # ptr_metadata(len, ..)
}

trait Trait {}

fn trait_object_metadata_without_dotdot(
    vtable: DynMetadata<dyn Trait + 'static>,
) -> builtin # ptr_metadata(dyn Trait) {
    builtin # ptr_metadata(vtable)
    //~^ ERROR: may have more fields added
    //~| NOTE: you can use `..`
}

fn trait_object_metadata(
    vtable: DynMetadata<dyn Trait + 'static>,
) -> builtin # ptr_metadata(dyn Trait) {
    builtin # ptr_metadata(vtable, ..)
}

struct Foo {
    x: u32,
    y: dyn Trait,
}

fn struct_metadata(vtable: DynMetadata<dyn Trait + 'static>) -> builtin # ptr_metadata(Foo) {
    builtin # ptr_metadata(y: builtin # ptr_metadata(vtable, ..), ..)
}

struct Bar {}

fn not_nonexhaustive_struct_metadata() -> builtin # ptr_metadata(Bar) {
    builtin # ptr_metadata()
}

#[non_exhaustive]
struct Qux {}

fn nonexhaustive_struct_metadata() -> builtin # ptr_metadata(Qux) {
    builtin # ptr_metadata()
    //~^ ERROR: may have more fields added
    //~| NOTE: you can use `..`
}

fn main() {}

//@ check-pass
//@ edition:2018

#![feature(ptr_metadata)]
#![feature(type_alias_impl_trait)]
#![feature(sized_hierarchy)]
#![feature(extern_types)]

pub type Opaque = impl std::future::Future;

#[define_opaque(Opaque)]
fn opaque() -> Opaque {
    async {}
}

pub type OpaqueExtern = impl std::marker::PointeeSized + std::ptr::Thin;

#[define_opaque(OpaqueExtern)]
fn opaque_extern() -> Option<&'static OpaqueExtern> {
    extern "Rust" {
        type ExternType;
    }
    None::<&ExternType>
}

fn a<T>() {
    // type parameter T is known to be sized
    is_thin::<T>();
    // tail of ADT (which is a type param) is known to be sized
    is_thin::<std::cell::Cell<T>>();
    // opaque type bounded by Sized is known to be thin
    is_thin::<Opaque>();
    // opaque type bounded by Thin is known to be thin
    is_thin::<OpaqueExtern>();
}

fn a2<T: Iterator>() {
    // associated type is known to be sized
    is_thin::<T::Item>();
}

fn is_thin<T: std::ptr::Thin + std::marker::PointeeSized>() {}

fn main() {}

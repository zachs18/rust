//@ edition:2018

#![feature(ptr_metadata)]
#![feature(type_alias_impl_trait)]
#![feature(sized_hierarchy)]
#![feature(extern_types)]

type Opaque = impl std::fmt::Debug + ?Sized;

#[define_opaque(Opaque)]
fn opaque() -> &'static Opaque {
    &[1] as &[i32]
}

type OpaqueExtern = impl std::marker::PointeeSized;

#[define_opaque(OpaqueExtern)]
fn opaque_extern() -> Option<&'static OpaqueExtern> {
    extern "Rust" {
        type ExternType;
    }
    None::<&ExternType>
}

fn a<T: ?Sized>() {
    is_thin::<T>();
    //~^ ERROR trait bound `T: Thin` is not satisfied

    is_thin::<Opaque>();
    //~^ ERROR trait bound `Opaque: Thin` is not satisfied

    is_thin::<OpaqueExtern>();
    //~^ ERROR trait bound `OpaqueExtern: Thin` is not satisfied
}

fn is_thin<T: std::ptr::Thin + std::marker::PointeeSized>() {}

fn main() {}

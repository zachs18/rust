//@ run-crash
//@ error-pattern: attempted to compute the size or alignment of unsized type `Foo` that does not implement `MetaSized`
//@ needs-subprocess
#![feature(unsized_type, sized_hierarchy, min_specialization, ptr_metadata)]

unsized type Foo {}

unsafe impl std::ptr::Thin for Foo {}

#[repr(C)]
struct Struct {
    a: u8,
    b: Foo,
}

fn main() {
    let mut a: u8 = 42;
    let a: &mut Struct = unsafe {
        &mut *(&raw mut a as *mut Struct)
    };
    println!("{a:p}");
    println!("{:p}", &a.a);
    // Panic here
    println!("{:p}", &a.b);
}

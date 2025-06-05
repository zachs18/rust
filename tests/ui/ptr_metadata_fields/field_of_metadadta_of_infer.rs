#![feature(ptr_metadata)]

fn main() {
    let x: &[u32] = &[1, 2, 3];
    let len = std::ptr::metadata(x as *const _).len;
    //~^ ERROR no field `len` on type `{ptr metadata for _}`
    let len = std::ptr::metadata(x).len;

    let x: &dyn std::fmt::Debug = &42;;
    let vtable = std::ptr::metadata(x as *const _).vtable;
    //~^ ERROR no field `vtable` on type `{ptr metadata for _}`
    let vtable = std::ptr::metadata(x).vtable;
}

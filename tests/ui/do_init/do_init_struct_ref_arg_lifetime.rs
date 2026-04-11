#![feature(in_place_init)]
#![feature(never_type)]
#![feature(ptr_metadata)]

use std::init::{PinInitOnce, InitOnce};
use std::ptr::Metadata;
use std::mem::MaybeUninit;

struct Foo;

unsafe impl PinInitOnce<u32, !, &'static mut u32> for Foo {
    fn metadata(_: &Self) -> Metadata<u32> {
        Default::default()
    }

    unsafe fn init_once(
        this: Foo,
        dst: &mut MaybeUninit<u32>,
        arg: &'static mut u32,
        pre_zeroed: bool,
    ) -> Result<(), !> {
        dst.write(*arg);
        Ok(())
    }
}
unsafe impl InitOnce<u32, !, &'static mut u32> for Foo {}

fn main() {
    // control test, shouldn't compile for unrelated reasons
    {
        let mut x: u32 = 42;
        let bx: Box<u32> = Box::build(std::init::with_arg(Foo, &mut x));
        //~^ ERROR: `x` does not live long enough
        dbg!(*bx);
    }

    {
        #[derive(Debug)]
        struct Bar {
            x: u32,
            y: u32,
        }
        let bx: Box<Bar> = Box::<Bar>::build(do init struct Bar {
            //~^ ERROR: implementation of `InitOnce` is not general enough
            x: 42,
            y (with ref x): Foo,
        });
        dbg!(*bx);
    }
}

use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize a `[u8]` as a `str`.
#[derive(Debug, Clone, Copy)]
pub struct AsBytes<I> {
    init: I,
}

impl<I> AsBytes<I> {
    pub(crate) const fn new(init: I) -> Self {
        Self { init }
    }
}

unsafe impl<Error, Arg, I: PinInitOnce<str, Error, Arg>> PinInitOnce<[u8], Error, Arg>
    for AsBytes<I>
{
    fn metadata(this: &Self) -> Metadata<[u8]> {
        I::metadata(&this.init) as Metadata<[u8]>
    }

    fn should_zero(this: &Self) -> bool {
        I::should_zero(&this.init)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[u8]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `MaybeUninit<str>` has no validity invariant
        let dst = unsafe { &mut *(dst as *mut MaybeUninit<[u8]> as *mut MaybeUninit<str>) };
        // SAFETY: `dst` has the correct length for `this.init`
        unsafe { I::init_once(this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<Error, Arg, I: PinInitMut<str, Error, Arg>> PinInitMut<[u8], Error, Arg>
    for AsBytes<I>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[u8]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `MaybeUninit<str>` has no validity invariant
        let dst = unsafe { &mut *(dst as *mut MaybeUninit<[u8]> as *mut MaybeUninit<str>) };
        // SAFETY: `dst` has the correct length for `this.init`
        unsafe { I::init_mut(&mut this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<Error, Arg, I: PinInit<str, Error, Arg>> PinInit<[u8], Error, Arg> for AsBytes<I> {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[u8]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `MaybeUninit<str>` has no validity invariant
        let dst = unsafe { &mut *(dst as *mut MaybeUninit<[u8]> as *mut MaybeUninit<str>) };
        // SAFETY: `dst` has the correct length for `this.init`
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<Error, Arg, I: InitOnce<str, Error, Arg>> InitOnce<[u8], Error, Arg> for AsBytes<I> {}
unsafe impl<Error, Arg, I: InitMut<str, Error, Arg>> InitMut<[u8], Error, Arg> for AsBytes<I> {}
unsafe impl<Error, Arg, I: Init<str, Error, Arg>> Init<[u8], Error, Arg> for AsBytes<I> {}

use crate::init::{Init, PinInit};
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

unsafe impl<Error, Arg, I: PinInit<str, Error, Arg>> PinInit<[u8], Error, Arg> for AsBytes<I> {
    fn metadata(this: &Self) -> Metadata<[u8]> {
        I::metadata(&this.init) as Metadata<[u8]>
    }

    fn should_zero(this: &Self) -> bool {
        I::should_zero(&this.init)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[u8]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `MaybeUninit<str>` has no validity invariant
        let dst = unsafe { &mut *(dst as *mut MaybeUninit<[u8]> as *mut MaybeUninit<str>) };
        // SAFETY: `dst` has the correct length for `this.init`
        unsafe { I::init(this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<Error, Arg, I: Init<str, Error, Arg>> Init<[u8], Error, Arg> for AsBytes<I> {}

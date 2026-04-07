use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData, PointeeSized};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize a place using initializer that can be reused.
// This needs to be a separate type so it can mention `T`,
// as otherwise `impl Init<T> for &U where U: Init<T>` overlaps with
// `impl Init<T> for T` where `U: Init<&U>`, which a user could write.
#[repr(transparent)]
pub struct ByRef<T: MetaSized, I: PointeeSized> {
    result: PhantomData<fn() -> T>,
    /// The wrapped initializer.
    pub init: I,
}

impl<T: MetaSized, I: PointeeSized + fmt::Debug> fmt::Debug for ByRef<T, I> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ByRef").field("init", &&self.init).finish_non_exhaustive()
    }
}

impl<T: MetaSized, I: PointeeSized> ByRef<T, I> {
    pub(crate) const fn new(init: &I) -> &Self {
        // SAFETY: self is `#[repr(transparent)]` over `I` with no additional invariants.
        unsafe { core::mem::transmute::<&I, &Self>(init) }
    }
}

unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + PinInit<T, Error, Arg>>
    PinInitOnce<T, Error, Arg> for &ByRef<T, I>
{
    fn metadata(this: &Self) -> Metadata<T> {
        I::metadata(&this.init)
    }

    fn should_zero(this: &Self) -> bool {
        I::should_zero(&this.init)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + Init<T, Error, Arg>> InitOnce<T, Error, Arg>
    for &ByRef<T, I>
{
}
unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + PinInit<T, Error, Arg>>
    PinInitMut<T, Error, Arg> for &ByRef<T, I>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + Init<T, Error, Arg>> InitMut<T, Error, Arg>
    for &ByRef<T, I>
{
}
unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + PinInit<T, Error, Arg>>
    PinInit<T, Error, Arg> for &ByRef<T, I>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg, I: PointeeSized + Init<T, Error, Arg>> Init<T, Error, Arg>
    for &ByRef<T, I>
{
}

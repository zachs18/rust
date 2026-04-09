use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, Thin};

/// Initialize a place using the initializer argument as an initializer.
pub struct FromArg<T: MetaSized + Thin> {
    result: PhantomData<fn() -> T>,
}

impl<T: MetaSized + Thin> Clone for FromArg<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: MetaSized + Thin> Copy for FromArg<T> {}

impl<T: MetaSized + Thin> fmt::Debug for FromArg<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FromArg").finish_non_exhaustive()
    }
}

impl<T: MetaSized + Thin> FromArg<T> {
    pub(crate) const fn new() -> Self {
        Self { result: PhantomData }
    }
}

unsafe impl<T: MetaSized + Thin, Error, Arg: PinInitOnce<T, Error>> PinInitOnce<T, Error, Arg>
    for FromArg<T>
{
    fn metadata(_this: &Self) -> Metadata<T> {
        Metadata::default()
    }

    unsafe fn init_once(
        _this: Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { Arg::init_once(arg, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: MetaSized + Thin, Error, Arg: InitOnce<T, Error>> InitOnce<T, Error, Arg>
    for FromArg<T>
{
}

unsafe impl<T: MetaSized + Thin, Error, Arg: PinInitOnce<T, Error>> PinInitMut<T, Error, Arg>
    for FromArg<T>
{
    unsafe fn init_mut(
        _this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { Arg::init_once(arg, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: MetaSized + Thin, Error, Arg: InitOnce<T, Error>> InitMut<T, Error, Arg>
    for FromArg<T>
{
}

unsafe impl<T: MetaSized + Thin, Error, Arg: PinInitOnce<T, Error>> PinInit<T, Error, Arg>
    for FromArg<T>
{
    unsafe fn init_ref(
        _this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { Arg::init_once(arg, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: MetaSized + Thin, Error, Arg: InitOnce<T, Error>> Init<T, Error, Arg>
    for FromArg<T>
{
}

use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize a place with an initializer that takes a computed argument.
pub struct MapArg<T: MetaSized, I, F> {
    result: PhantomData<fn() -> T>,
    init: I,
    f: F,
}

impl<T: MetaSized, I: Clone, F: Clone> Clone for MapArg<T, I, F> {
    fn clone(&self) -> Self {
        Self { result: PhantomData, init: self.init.clone(), f: self.f.clone() }
    }
}
impl<T: MetaSized, I: Copy, F: Copy> Copy for MapArg<T, I, F> {}

impl<T: MetaSized, I: fmt::Debug, F: fmt::Debug> fmt::Debug for MapArg<T, I, F> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapArg")
            .field("init", &self.init)
            .field("f", &self.f)
            .finish_non_exhaustive()
    }
}

impl<T: MetaSized, I, F> MapArg<T, I, F> {
    pub(crate) const fn new(init: I, f: F) -> Self {
        Self { result: PhantomData, init, f }
    }
}

unsafe impl<
    T: MetaSized,
    Error,
    Arg1,
    Arg2,
    F: FnOnce(Arg1) -> Arg2,
    I: PinInitOnce<T, Error, Arg2>,
> PinInitOnce<T, Error, Arg1> for MapArg<T, I, F>
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
        arg: Arg1,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let arg = (this.f)(arg);
        // SAFETY: delegated to caller
        unsafe { I::init_once(this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg1, Arg2, F: FnOnce(Arg1) -> Arg2, I: InitOnce<T, Error, Arg2>>
    InitOnce<T, Error, Arg1> for MapArg<T, I, F>
{
}

unsafe impl<T: MetaSized, Error, Arg1, Arg2, F: FnMut(Arg1) -> Arg2, I: PinInitMut<T, Error, Arg2>>
    PinInitMut<T, Error, Arg1> for MapArg<T, I, F>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg1,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let arg = (&mut this.f)(arg);
        // SAFETY: delegated to caller
        unsafe { I::init_mut(&mut this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg1, Arg2, F: FnMut(Arg1) -> Arg2, I: InitMut<T, Error, Arg2>>
    InitMut<T, Error, Arg1> for MapArg<T, I, F>
{
}

unsafe impl<T: MetaSized, Error, Arg1, Arg2, F: Fn(Arg1) -> Arg2, I: PinInit<T, Error, Arg2>>
    PinInit<T, Error, Arg1> for MapArg<T, I, F>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg1,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let arg = (&this.f)(arg);
        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg1, Arg2, F: Fn(Arg1) -> Arg2, I: Init<T, Error, Arg2>>
    Init<T, Error, Arg1> for MapArg<T, I, F>
{
}

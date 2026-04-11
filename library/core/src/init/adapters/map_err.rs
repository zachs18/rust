use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize a place with an initializer that produces a different error type.
pub struct MapErr<T: MetaSized, I, F, Error1> {
    result: PhantomData<fn() -> (Error1, T)>,
    init: I,
    f: F,
}

impl<T: MetaSized, I: Clone, F: Clone, Error1> Clone for MapErr<T, I, F, Error1> {
    fn clone(&self) -> Self {
        Self { result: PhantomData, init: self.init.clone(), f: self.f.clone() }
    }
}
impl<T: MetaSized, I: Copy, F: Copy, Error1> Copy for MapErr<T, I, F, Error1> {}

impl<T: MetaSized, I: fmt::Debug, F: fmt::Debug, Error1> fmt::Debug for MapErr<T, I, F, Error1> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapErr")
            .field("init", &self.init)
            .field("f", &self.f)
            .finish_non_exhaustive()
    }
}

impl<T: MetaSized, I, F, Error1> MapErr<T, I, F, Error1> {
    pub(crate) const fn new(init: I, f: F) -> Self {
        Self { result: PhantomData, init, f }
    }
}

unsafe impl<
    T: MetaSized,
    Error1,
    Error2,
    Arg,
    F: FnOnce(Error1) -> Error2,
    I: PinInitOnce<T, Error1, Arg>,
> PinInitOnce<T, Error2, Arg> for MapErr<T, I, F, Error1>
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
    ) -> Result<(), Error2> {
        // SAFETY: delegated to caller
        unsafe { I::init_once(this.init, dst, arg, pre_zeroed) }.map_err(this.f)
    }
}
unsafe impl<
    T: MetaSized,
    Error1,
    Error2,
    Arg,
    F: FnOnce(Error1) -> Error2,
    I: InitOnce<T, Error1, Arg>,
> InitOnce<T, Error2, Arg> for MapErr<T, I, F, Error1>
{
}

unsafe impl<
    T: MetaSized,
    Error1,
    Error2,
    Arg,
    F: FnMut(Error1) -> Error2,
    I: PinInitMut<T, Error1, Arg>,
> PinInitMut<T, Error2, Arg> for MapErr<T, I, F, Error1>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error2> {
        // SAFETY: delegated to caller
        unsafe { I::init_mut(&mut this.init, dst, arg, pre_zeroed) }.map_err(&mut this.f)
    }
}
unsafe impl<
    T: MetaSized,
    Error1,
    Error2,
    Arg,
    F: FnMut(Error1) -> Error2,
    I: InitMut<T, Error1, Arg>,
> InitMut<T, Error2, Arg> for MapErr<T, I, F, Error1>
{
}

unsafe impl<T: MetaSized, Error1, Error2, Arg, F: Fn(Error1) -> Error2, I: PinInit<T, Error1, Arg>>
    PinInit<T, Error2, Arg> for MapErr<T, I, F, Error1>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error2> {
        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }.map_err(&this.f)
    }
}
unsafe impl<T: MetaSized, Error1, Error2, Arg, F: Fn(Error1) -> Error2, I: Init<T, Error1, Arg>>
    Init<T, Error2, Arg> for MapErr<T, I, F, Error1>
{
}

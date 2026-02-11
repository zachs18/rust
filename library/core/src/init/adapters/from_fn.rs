use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, Thin};

/// Initialize a place by creating an initializer just-in-time.
///
/// ```rust
/// #![feature(in_place_init)]
/// let mut i = 1;
/// let bx: Box<[usize; 3]> = Box::build(
///     std::init::repeat_array(std::init::from_fn(|| {
///         let val = i;
///         i += 2;
///         val
///     }))
/// );
/// assert_eq!(*bx, [1, 3, 5]);
///
/// let mut i = 1;
/// let bx: Box<[usize]> = Box::build(
///     std::init::repeat_array::<3, _>(std::init::from_fn(|| {
///         let val = i;
///         i += 2;
///         val
///     }))
/// );
/// assert_eq!(*bx, [1, 3, 5]);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct FromFn<T: Thin + MetaSized, F, HasArg = FnNoArg> {
    _has_arg: HasArg,
    // Necessary, otherwise the relevant impl overlaps when `F: Fn() -> F`
    _type: PhantomData<fn() -> T>,
    func: F,
}

/// Marker type for [`FromFn`] whose function does not require an argument.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct FnNoArg;

/// Marker type for [`FromFn`] whose function requires an argument.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct FnWithArg;

impl<T: Thin + MetaSized, F> FromFn<T, F> {
    pub(crate) const fn new(func: F) -> Self {
        Self { func, _has_arg: FnNoArg, _type: PhantomData }
    }
}

impl<T: Thin + MetaSized, F> FromFn<T, F, FnWithArg> {
    pub(crate) const fn new_with_arg(func: F) -> Self {
        Self { func, _has_arg: FnWithArg, _type: PhantomData }
    }
}

unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error, Arg>, F: FnOnce() -> I>
    PinInitOnce<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
    fn metadata(_this: &Self) -> Metadata<T> {
        Default::default()
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)();
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error, Arg>>::init_once(init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error, Arg>, F: FnMut() -> I>
    PinInitMut<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)();
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error, Arg>>::init_once(init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error, Arg>, F: Fn() -> I>
    PinInit<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)();
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error, Arg>>::init_once(init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error, Arg>, F: FnOnce() -> I>
    InitOnce<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error, Arg>, F: FnMut() -> I>
    InitMut<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error, Arg>, F: Fn() -> I>
    Init<T, Error, Arg> for FromFn<T, F, FnNoArg>
{
}

unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error>, F: FnOnce(Arg) -> I>
    PinInitOnce<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
    fn metadata(_this: &Self) -> Metadata<T> {
        Default::default()
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)(arg);
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error>>::init_once(init, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error>, F: FnMut(Arg) -> I>
    PinInitMut<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)(arg);
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error>>::init_once(init, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: PinInitOnce<T, Error>, F: Fn(Arg) -> I>
    PinInit<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let init = (this.func)(arg);
        // SAFETY: delegated to caller
        unsafe { <I as PinInitOnce<T, Error>>::init_once(init, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error>, F: FnOnce(Arg) -> I>
    InitOnce<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error>, F: FnMut(Arg) -> I>
    InitMut<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
}
unsafe impl<T: Thin + MetaSized, Error, Arg, I: InitOnce<T, Error>, F: Fn(Arg) -> I>
    Init<T, Error, Arg> for FromFn<T, F, FnWithArg>
{
}

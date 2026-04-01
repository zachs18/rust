use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, PhantomData, Unsize as UnsizeTrait};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize an unsized place with an initializer for a value that can be unsized-coerced
/// into the type of that place.
pub struct Unsize<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I> {
    result: PhantomData<fn() -> U>,
    inner_result: PhantomData<fn() -> T>,
    init: I,
}

impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I: Clone> Clone for Unsize<U, T, I> {
    fn clone(&self) -> Self {
        Self { result: PhantomData, inner_result: PhantomData, init: self.init.clone() }
    }
}
impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I: Copy> Copy for Unsize<U, T, I> {}

impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I: fmt::Debug> fmt::Debug for Unsize<U, T, I> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unsize").field("init", &self.init).finish_non_exhaustive()
    }
}

impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I> Unsize<U, T, I> {
    pub(crate) const fn new(init: I) -> Self {
        Self { result: PhantomData, inner_result: PhantomData, init }
    }

    /// # Safety
    ///
    /// `dst` must have come from unsizing a value with `I::metadata(&self.init)` metadata.
    unsafe fn un_unsize<'dst, Error, Arg>(
        &self,
        dst: &'dst mut MaybeUninit<U>,
    ) -> &'dst mut MaybeUninit<T>
    where
        I: PinInitOnce<T, Error, Arg>,
    {
        // "un-unsize" `dst`
        let meta: Metadata<T> = I::metadata(&self.init);
        let dst = core::ptr::from_raw_parts_mut(
            dst as *mut _ as *mut (),
            meta as Metadata<MaybeUninit<T>>,
        );
        // SAFETY: delegated to caller; if the original `dst` was unsized from a `&mut MaybeUninit<T>`
        // with `meta`, then it's safe to "reborrow" it with that metadata again.
        unsafe { &mut *dst }
    }
}

unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: PinInitOnce<T, Error, Arg>>
    PinInitOnce<U, Error, Arg> for Unsize<U, T, I>
{
    fn metadata(this: &Self) -> Metadata<U> {
        I::metadata(&this.init)
    }

    fn should_zero(this: &Self) -> bool {
        I::should_zero(&this.init)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<U>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        let dst = unsafe { this.un_unsize(dst) };

        // SAFETY: delegated to caller
        unsafe { I::init_once(this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: InitOnce<T, Error, Arg>>
    InitOnce<U, Error, Arg> for Unsize<U, T, I>
{
}

unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: PinInitMut<T, Error, Arg>>
    PinInitMut<U, Error, Arg> for Unsize<U, T, I>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<U>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        let dst = unsafe { this.un_unsize(dst) };

        // SAFETY: delegated to caller
        unsafe { I::init_mut(&mut this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: InitMut<T, Error, Arg>>
    InitMut<U, Error, Arg> for Unsize<U, T, I>
{
}

unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: PinInit<T, Error, Arg>>
    PinInit<U, Error, Arg> for Unsize<U, T, I>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<U>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        let dst = unsafe { this.un_unsize(dst) };

        // SAFETY: delegated to caller
        unsafe { I::init_ref(&this.init, dst, arg, pre_zeroed) }
    }
}
unsafe impl<U: MetaSized, T: MetaSized + UnsizeTrait<U>, Error, Arg, I: Init<T, Error, Arg>>
    Init<U, Error, Arg> for Unsize<U, T, I>
{
}

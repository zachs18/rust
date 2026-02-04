use crate::fmt;
use crate::init::{InitOnce, PinInitOnce};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// Initialize a place with an initializer that takes a provided argument.
pub struct WithArg<T: MetaSized, I, Arg> {
    result: PhantomData<fn() -> T>,
    init: I,
    arg: Arg,
}

impl<T: MetaSized, I: Clone, Arg: Clone> Clone for WithArg<T, I, Arg> {
    fn clone(&self) -> Self {
        Self { result: PhantomData, init: self.init.clone(), arg: self.arg.clone() }
    }
}
impl<T: MetaSized, I: Copy, Arg: Copy> Copy for WithArg<T, I, Arg> {}

impl<T: MetaSized, I: fmt::Debug, Arg: fmt::Debug> fmt::Debug for WithArg<T, I, Arg> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithArg")
            .field("init", &self.init)
            .field("arg", &self.arg)
            .finish_non_exhaustive()
    }
}

impl<T: MetaSized, I, Arg> WithArg<T, I, Arg> {
    pub(crate) const fn new(init: I, arg: Arg) -> Self {
        Self { result: PhantomData, init, arg }
    }
}

unsafe impl<T: MetaSized, Error, Arg, I: PinInitOnce<T, Error, Arg>> PinInitOnce<T, Error>
    for WithArg<T, I, Arg>
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
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { I::init_once(this.init, dst, this.arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, Error, Arg, I: InitOnce<T, Error, Arg>> InitOnce<T, Error>
    for WithArg<T, I, Arg>
{
}

use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

/// Initialize a slice in two pieces.
#[derive(Debug, Clone, Copy)]
pub struct Chain<I1, I2> {
    init1: I1,
    init2: I2,
}

impl<I1, I2> Chain<I1, I2> {
    pub(crate) const fn new(init1: I1, init2: I2) -> Self {
        Self { init1, init2 }
    }
}

struct DropGuard<'a, T>(&'a mut [T]);
impl<'a, T> Drop for DropGuard<'a, T> {
    fn drop(&mut self) {
        if core::mem::needs_drop::<T>() {
            // SAFETY:
            // items were marked initialized in the code below
            unsafe {
                core::ptr::drop_in_place(self.0);
            }
        }
    }
}

unsafe impl<
    T,
    Error,
    Arg: Clone,
    I1: PinInitOnce<[T], Error, Arg>,
    I2: PinInitOnce<[T], Error, Arg>,
> PinInitOnce<[T], Error, Arg> for Chain<I1, I2>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        let len = usize::checked_add(I1::metadata(&this.init1).len, I2::metadata(&this.init2).len)
            .expect("slice length overflow");
        build_metadata!(len, ..)
    }

    fn should_zero(this: &Self) -> bool {
        I1::should_zero(&this.init1) || I2::should_zero(&this.init2)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let len1 = I1::metadata(&this.init1).len;
        let len2 = I2::metadata(&this.init2).len;
        debug_assert_eq!(dst.len(), len1 + len2);

        let (dst1, dst2) = dst.transpose_mut().split_at_mut(len1);
        let dst1 = dst1.transpose_mut();
        let dst2 = dst2.transpose_mut();

        // SAFETY: `dst1` has the correct length for `init1`
        unsafe {
            I1::init_once(this.init1, dst1, arg.clone(), pre_zeroed)?;
        }
        // SAFETY: `dst1` was just initialized
        let guard = DropGuard(unsafe { dst1.assume_init_mut() });

        // SAFETY: `dst2` has the correct length for `init2`
        unsafe {
            I2::init_once(this.init2, dst2, arg, pre_zeroed)?;
        }
        core::mem::forget(guard);
        Ok(())
    }
}
unsafe impl<T, Error, Arg: Clone, I1: PinInitMut<[T], Error, Arg>, I2: PinInitMut<[T], Error, Arg>>
    PinInitMut<[T], Error, Arg> for Chain<I1, I2>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let len1 = I1::metadata(&this.init1).len;
        let len2 = I2::metadata(&this.init2).len;
        debug_assert_eq!(dst.len(), len1 + len2);

        let (dst1, dst2) = dst.transpose_mut().split_at_mut(len1);
        let dst1 = dst1.transpose_mut();
        let dst2 = dst2.transpose_mut();

        // SAFETY: `dst1` has the correct length for `init1`
        unsafe {
            I1::init_mut(&mut this.init1, dst1, arg.clone(), pre_zeroed)?;
        }
        // SAFETY: `dst1` was just initialized
        let guard = DropGuard(unsafe { dst1.assume_init_mut() });

        // SAFETY: `dst2` has the correct length for `init2`
        unsafe {
            I2::init_mut(&mut this.init2, dst2, arg, pre_zeroed)?;
        }
        core::mem::forget(guard);
        Ok(())
    }
}
unsafe impl<T, Error, Arg: Clone, I1: PinInit<[T], Error, Arg>, I2: PinInit<[T], Error, Arg>>
    PinInit<[T], Error, Arg> for Chain<I1, I2>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let len1 = I1::metadata(&this.init1).len;
        let len2 = I2::metadata(&this.init2).len;
        debug_assert_eq!(dst.len(), len1 + len2);

        let (dst1, dst2) = dst.transpose_mut().split_at_mut(len1);
        let dst1 = dst1.transpose_mut();
        let dst2 = dst2.transpose_mut();

        // SAFETY: `dst1` has the correct length for `init1`
        unsafe {
            I1::init_ref(&this.init1, dst1, arg.clone(), pre_zeroed)?;
        }
        // SAFETY: `dst1` was just initialized
        let guard = DropGuard(unsafe { dst1.assume_init_mut() });

        // SAFETY: `dst2` has the correct length for `init2`
        unsafe {
            I2::init_ref(&this.init2, dst2, arg, pre_zeroed)?;
        }
        core::mem::forget(guard);
        Ok(())
    }
}
unsafe impl<T, Error, Arg: Clone, I1: InitOnce<[T], Error, Arg>, I2: InitOnce<[T], Error, Arg>>
    InitOnce<[T], Error, Arg> for Chain<I1, I2>
{
}
unsafe impl<T, Error, Arg: Clone, I1: InitMut<[T], Error, Arg>, I2: InitMut<[T], Error, Arg>>
    InitMut<[T], Error, Arg> for Chain<I1, I2>
{
}
unsafe impl<T, Error, Arg: Clone, I1: Init<[T], Error, Arg>, I2: Init<[T], Error, Arg>>
    Init<[T], Error, Arg> for Chain<I1, I2>
{
}

unsafe impl<Error, Arg: Clone, I1: PinInitOnce<str, Error, Arg>, I2: PinInitOnce<str, Error, Arg>>
    PinInitOnce<str, Error, Arg> for Chain<I1, I2>
{
    fn metadata(this: &Self) -> Metadata<str> {
        let len = usize::checked_add(I1::metadata(&this.init1).len, I2::metadata(&this.init2).len)
            .expect("slice length overflow");
        build_metadata!(len, ..)
    }

    fn should_zero(this: &Self) -> bool {
        I1::should_zero(&this.init1) || I2::should_zero(&this.init2)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<str>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.as_bytes_mut();
        let len1 = I1::metadata(&this.init1).len;
        let len2 = I2::metadata(&this.init2).len;
        debug_assert_eq!(dst.len(), len1 + len2);

        let (dst1, dst2) = dst.split_at_mut(len1);
        let dst1 = dst1.transpose_mut();
        let dst2 = dst2.transpose_mut();

        // SAFETY: `dst1` has the correct length for `init1`
        unsafe {
            PinInitOnce::init_once(
                crate::init::as_bytes(this.init1),
                dst1,
                arg.clone(),
                pre_zeroed,
            )?;
        }
        // Don't need a drop guard because `str` doesn't need dropping

        // SAFETY: `dst2` has the correct length for `init2`
        unsafe {
            PinInitOnce::init_once(crate::init::as_bytes(this.init2), dst2, arg, pre_zeroed)?;
        }
        Ok(())
    }
}
unsafe impl<Error, Arg: Clone, I1: InitOnce<str, Error, Arg>, I2: InitOnce<str, Error, Arg>>
    InitOnce<str, Error, Arg> for Chain<I1, I2>
{
}

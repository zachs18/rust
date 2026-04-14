use crate::init::{Init, PinInit};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

/// Initialize a slice by repeating an element
#[derive(Debug, Clone, Copy)]
pub struct Repeat<I> {
    elem: I,
    len: usize,
}

impl<I> Repeat<I> {
    pub(crate) const fn new(elem: I, len: usize) -> Self {
        Self { elem, len }
    }
}

unsafe impl<T: MetaSized, Error, Arg: Clone, I: Clone + PinInit<T, Error, Arg>>
    PinInit<[T], Error, Arg> for Repeat<I>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        build_metadata!(len: this.len, elem: I::metadata(&this.elem), ..)
    }

    fn should_zero(this: &Self) -> bool {
        this.len > 0 && I::should_zero(&this.elem)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        debug_assert_eq!(dst.len(), this.len);

        struct DropGuard<'a, T: MetaSized> {
            dst: &'a mut [MaybeUninit<T>],
            init_len: usize,
        }
        impl<'a, T: MetaSized> Drop for DropGuard<'a, T> {
            fn drop(&mut self) {
                if core::mem::needs_drop::<T>() {
                    // SAFETY:
                    // items were marked initialized in the code below
                    unsafe {
                        self.dst[..self.init_len].assume_init_drop();
                    }
                }
            }
        }

        let dst = dst.transpose_mut();
        let mut guard = DropGuard { dst, init_len: 0 };

        let inits = core::iter::repeat_n(this.elem, this.len);
        let args = core::iter::repeat_n(arg, this.len);

        for (idx, (elem, (init, arg))) in
            core::iter::zip(&mut *guard.dst, core::iter::zip(inits, args)).enumerate()
        {
            // SAFETY: we are initializing an element with the correct
            // parameters given to us by the caller
            unsafe {
                I::init(init, elem, arg, pre_zeroed)?;
            }
            guard.init_len = idx + 1;
        }

        core::mem::forget(guard);
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error, Arg: Clone, I: Clone + Init<T, Error, Arg>> Init<[T], Error, Arg>
    for Repeat<I>
{
}

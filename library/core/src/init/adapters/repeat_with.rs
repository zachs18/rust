use crate::init::util::InitializingSlice;
use crate::init::{ConstLength, Init, Length, PinInit, RuntimeLength};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, Thin, build_metadata};

/// Initialize an array or slice by creating an initializer for each element.
///
/// ```rust
/// #![feature(in_place_init)]
/// let bx: Box<[usize; 3]> = Box::build(
///     std::init::repeat_with_array(|idx| idx * 2 + 1)
/// );
/// assert_eq!(*bx, [1, 3, 5]);
///
/// let bx: Box<[usize]> = Box::build(
///     std::init::repeat_with_array::<3, _>(|idx| idx * 2 + 1)
/// );
/// assert_eq!(*bx, [1, 3, 5]);
/// ```
#[derive(Debug, Clone)]
pub struct RepeatWith<F, L: Length> {
    length: L,
    func: F,
}

impl<F> RepeatWith<F, RuntimeLength> {
    pub(crate) const fn new_slice(length: usize, func: F) -> Self {
        Self { length: RuntimeLength { length }, func }
    }

    pub(crate) const fn new_array<const N: usize>(func: F) -> RepeatWith<F, ConstLength<N>> {
        RepeatWith { length: ConstLength, func }
    }
}

unsafe impl<
    T: Thin + MetaSized,
    L: Length,
    Error,
    Arg: Clone,
    I: PinInit<T, Error, Arg>,
    F: FnMut(usize) -> I,
> PinInit<[T], Error, Arg> for RepeatWith<F, L>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        build_metadata!(len: this.length.length(), ..)
    }

    unsafe fn init(
        mut this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.transpose_mut();
        let count = this.length.length();
        debug_assert_eq!(dst.len(), count);
        // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
        let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
        for (idx, arg) in core::iter::repeat_n(arg, count).enumerate() {
            let init = (this.func)(idx);
            // SAFETY: delegated to caller
            unsafe {
                I::init(init, &mut buf.data[idx], arg, pre_zeroed)?;
            }
            // SAFETY: we just initialized buf.data[idx]
            buf.initialized_len += 1;
        }
        core::mem::forget(buf);
        Ok(())
    }
}
unsafe impl<
    T: Thin + MetaSized,
    L: Length,
    Error,
    Arg: Clone,
    I: Init<T, Error, Arg>,
    F: FnMut(usize) -> I,
> Init<[T], Error, Arg> for RepeatWith<F, L>
{
}

unsafe impl<
    T: Thin + MetaSized,
    const N: usize,
    Error,
    Arg: Clone,
    I: Init<T, Error, Arg>,
    F: FnMut(usize) -> I,
> PinInit<[T; N], Error, Arg> for RepeatWith<F, ConstLength<N>>
{
    fn metadata(_this: &Self) -> Metadata<[T; N]> {
        build_metadata!(..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInit<[T], Error, Arg>>::init(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<
    T: Thin + MetaSized,
    const N: usize,
    Error,
    Arg: Clone,
    I: Init<T, Error, Arg>,
    F: FnMut(usize) -> I,
> Init<[T; N], Error, Arg> for RepeatWith<F, ConstLength<N>>
{
}

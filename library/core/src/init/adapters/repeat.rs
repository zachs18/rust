use crate::init::util::InitializingSlice;
use crate::init::{ConstLength, Init, Length, PinInit, RuntimeLength};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

/// Initialize an array or slice by cloning an initializer for each element.
///
/// ```rust
/// #![feature(in_place_init)]
/// let bx: Box<[usize; 3]> = Box::build(
///     std::init::repeat_array(42)
/// );
/// assert_eq!(*bx, [42, 42, 42]);
///
/// let bx: Box<[usize]> = Box::build(
///     std::init::repeat_array::<3, _>(42)
/// );
/// assert_eq!(*bx, [42, 42, 42]);
/// ```
#[derive(Debug, Clone)]
pub struct Repeat<I, L: Length> {
    length: L,
    elem: I,
}

impl<I> Repeat<I, RuntimeLength> {
    pub(crate) const fn new_slice(length: usize, elem: I) -> Self {
        Self { length: RuntimeLength { length }, elem }
    }

    pub(crate) const fn new_array<const N: usize>(elem: I) -> Repeat<I, ConstLength<N>> {
        Repeat { length: ConstLength, elem }
    }
}

unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: Clone + PinInit<T, Error, Arg>>
    PinInit<[T], Error, Arg> for Repeat<I, L>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        build_metadata!(len: this.length.length(), elem: I::metadata(&this.elem), ..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.transpose_mut();
        let count = this.length.length();
        debug_assert_eq!(dst.len(), count);
        // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
        let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
        for (idx, (elem, arg)) in core::iter::repeat_n((this.elem, arg), count).enumerate() {
            // SAFETY: delegated to caller
            unsafe {
                I::init(elem, &mut buf.data[idx], arg, pre_zeroed)?;
            }
            // SAFETY: we just initialized buf.data[idx]
            buf.initialized_len += 1;
        }
        core::mem::forget(buf);
        Ok(())
    }
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: Clone + Init<T, Error, Arg>>
    Init<[T], Error, Arg> for Repeat<I, L>
{
}

unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: Clone + Init<T, Error, Arg>>
    PinInit<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
    fn metadata(this: &Self) -> Metadata<[T; N]> {
        build_metadata!(elem: I::metadata(&this.elem), ..)
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
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: Clone + Init<T, Error, Arg>>
    Init<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
}

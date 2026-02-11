use crate::init::util::InitializingSlice;
use crate::init::{
    ConstLength, Init, InitMut, InitOnce, Length, PinInit, PinInitMut, PinInitOnce, RuntimeLength,
};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

trait RepeatArgKind<IArg> {
    type Arg;

    fn args(input_arg: IArg, count: usize) -> impl ExactSizeIterator<Item = Self::Arg>;
}

#[derive(Debug, Clone, Copy)]
struct NoIndexArg;

impl<IArg: Clone> RepeatArgKind<IArg> for NoIndexArg {
    type Arg = IArg;

    fn args(input_arg: IArg, count: usize) -> impl ExactSizeIterator<Item = Self::Arg> {
        crate::iter::repeat_n(input_arg, count)
    }
}

#[derive(Debug, Clone, Copy)]
struct OnlyIndexArg;

impl RepeatArgKind<()> for OnlyIndexArg {
    type Arg = usize;

    fn args(_input_arg: (), count: usize) -> impl ExactSizeIterator<Item = Self::Arg> {
        0..count
    }
}

#[derive(Debug, Clone, Copy)]
struct WithIndexArg;

impl<IArg: Clone> RepeatArgKind<IArg> for WithIndexArg {
    type Arg = (IArg, usize);

    fn args(input_arg: IArg, count: usize) -> impl ExactSizeIterator<Item = Self::Arg> {
        crate::iter::repeat_n(input_arg, count).zip(0..count)
    }
}

#[derive(Debug, Clone, Copy)]
struct RepeatInner<I, L: Length, K> {
    elem: I,
    length: L,
    _arg_kind: K,
}

unsafe impl<
    T: MetaSized,
    L: Length,
    Error,
    IArg,
    OArg,
    I: PinInitMut<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInitOnce<[T], Error, IArg> for RepeatInner<I, L, K>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        build_metadata!(len: this.length.length(), elem: I::metadata(&this.elem), ..)
    }

    fn should_zero(this: &Self) -> bool {
        I::should_zero(&this.elem)
    }

    unsafe fn init_once(
        mut this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.transpose_mut();
        let count = this.length.length();
        debug_assert_eq!(dst.len(), count);
        // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
        let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
        let elem = &mut this.elem;
        for (idx, arg) in K::args(arg, count).enumerate() {
            // we do this so the last element can be initialized using `init_once`,
            // which could avoid cloning.
            if idx < count - 1 {
                // SAFETY: delegated to caller
                unsafe {
                    I::init_mut(elem, &mut buf.data[idx], arg, pre_zeroed)?;
                }
                // SAFETY: we just initialized buf.data[idx]
                buf.initialized_len += 1;
            } else {
                // SAFETY: delegated to caller
                unsafe {
                    I::init_once(this.elem, &mut buf.data[idx], arg, pre_zeroed)?;
                }
                break;
            }
        }
        core::mem::forget(buf);
        Ok(())
    }
}
unsafe impl<
    T: MetaSized,
    L: Length,
    Error,
    IArg,
    OArg,
    I: PinInitMut<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInitMut<[T], Error, IArg> for RepeatInner<I, L, K>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.transpose_mut();
        let count = this.length.length();
        debug_assert_eq!(dst.len(), count);
        // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
        let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
        let elem = &mut this.elem;
        for (idx, arg) in K::args(arg, count).enumerate() {
            // SAFETY: delegated to caller
            unsafe {
                I::init_mut(elem, &mut buf.data[idx], arg, pre_zeroed)?;
            }
            // SAFETY: we just initialized buf.data[idx]
            buf.initialized_len += 1;
        }
        core::mem::forget(buf);
        Ok(())
    }
}
unsafe impl<
    T: MetaSized,
    L: Length,
    Error,
    IArg,
    OArg,
    I: PinInit<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInit<[T], Error, IArg> for RepeatInner<I, L, K>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        let dst = dst.transpose_mut();
        let count = this.length.length();
        debug_assert_eq!(dst.len(), count);
        // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
        let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
        let elem = &this.elem;
        for (idx, arg) in K::args(arg, count).enumerate() {
            // SAFETY: delegated to caller
            unsafe {
                I::init_ref(elem, &mut buf.data[idx], arg, pre_zeroed)?;
            }
            // SAFETY: we just initialized buf.data[idx]
            buf.initialized_len += 1;
        }
        core::mem::forget(buf);
        Ok(())
    }
}

unsafe impl<
    T: MetaSized,
    const N: usize,
    Error,
    IArg,
    OArg,
    I: PinInitMut<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInitOnce<[T; N], Error, IArg> for RepeatInner<I, ConstLength<N>, K>
{
    fn metadata(this: &Self) -> Metadata<[T; N]> {
        build_metadata!(elem: I::metadata(&this.elem), ..)
    }

    fn should_zero(this: &Self) -> bool {
        <I as PinInitOnce<T, Error, OArg>>::should_zero(&this.elem)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitOnce<[T], Error, IArg>>::init_once(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<
    T: MetaSized,
    const N: usize,
    Error,
    IArg,
    OArg,
    I: PinInitMut<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInitMut<[T; N], Error, IArg> for RepeatInner<I, ConstLength<N>, K>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitMut<[T], Error, IArg>>::init_mut(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<
    T: MetaSized,
    const N: usize,
    Error,
    IArg,
    OArg,
    I: PinInit<T, Error, OArg>,
    K: RepeatArgKind<IArg, Arg = OArg>,
> PinInit<[T; N], Error, IArg> for RepeatInner<I, ConstLength<N>, K>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: IArg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInit<[T], Error, IArg>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}

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
    inner: RepeatInner<I, L, NoIndexArg>,
}

impl<I> Repeat<I, RuntimeLength> {
    pub(crate) const fn new_slice(length: usize, elem: I) -> Self {
        Self {
            inner: RepeatInner { length: RuntimeLength { length }, elem, _arg_kind: NoIndexArg },
        }
    }

    pub(crate) const fn new_array<const N: usize>(elem: I) -> Repeat<I, ConstLength<N>> {
        Repeat { inner: RepeatInner { length: ConstLength, elem, _arg_kind: NoIndexArg } }
    }
}

unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: PinInitMut<T, Error, Arg>>
    PinInitOnce<[T], Error, Arg> for Repeat<I, L>
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        <_ as PinInitOnce<[T], Error, Arg>>::metadata(&this.inner)
    }

    fn should_zero(this: &Self) -> bool {
        <I as PinInitOnce<T, Error, Arg>>::should_zero(&this.inner.elem)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { RepeatInner::init_once(this.inner, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: PinInitMut<T, Error, Arg>>
    PinInitMut<[T], Error, Arg> for Repeat<I, L>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { RepeatInner::init_mut(&mut this.inner, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: PinInit<T, Error, Arg>>
    PinInit<[T], Error, Arg> for Repeat<I, L>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { RepeatInner::init_ref(&this.inner, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: InitMut<T, Error, Arg>>
    InitOnce<[T], Error, Arg> for Repeat<I, L>
{
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: InitMut<T, Error, Arg>>
    InitMut<[T], Error, Arg> for Repeat<I, L>
{
}
unsafe impl<T: MetaSized, L: Length, Error, Arg: Clone, I: Init<T, Error, Arg>>
    Init<[T], Error, Arg> for Repeat<I, L>
{
}

unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: PinInitMut<T, Error, Arg>>
    PinInitOnce<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
    fn metadata(this: &Self) -> Metadata<[T; N]> {
        RepeatInner::metadata(&this.inner)
    }

    fn should_zero(this: &Self) -> bool {
        <I as PinInitOnce<T, Error, Arg>>::should_zero(&this.inner.elem)
    }

    unsafe fn init_once(
        mut this: Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitMut<[T], Error, Arg>>::init_mut(&mut this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: PinInitMut<T, Error, Arg>>
    PinInitMut<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitMut<[T], Error, Arg>>::init_mut(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: PinInit<T, Error, Arg>>
    PinInit<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T; N]>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInit<[T], Error, Arg>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: InitMut<T, Error, Arg>>
    InitOnce<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
}
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: InitMut<T, Error, Arg>>
    InitMut<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
}
unsafe impl<T: MetaSized, const N: usize, Error, Arg: Clone, I: Init<T, Error, Arg>>
    Init<[T; N], Error, Arg> for Repeat<I, ConstLength<N>>
{
}

use crate::init::util::InitializingSlice;
use crate::init::{
    ConstLength, Init, InitMut, InitOnce, Length, PinInit, PinInitMut, PinInitOnce, RuntimeLength,
};
use crate::marker::{MetaSized, PhantomData};
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

trait ArgKind<OArg>: Sized {
    fn args(input_arg: Self, count: usize) -> impl ExactSizeIterator<Item = OArg>;
}

impl<T: Clone> ArgKind<T> for T {
    fn args(input_arg: Self, count: usize) -> impl ExactSizeIterator<Item = T> {
        core::iter::repeat_n(input_arg, count)
    }
}

impl<T: Clone> ArgKind<(T, usize)> for T {
    fn args(input_arg: Self, count: usize) -> impl ExactSizeIterator<Item = (T, usize)> {
        core::iter::repeat_n(input_arg, count).zip(0..count)
    }
}

impl ArgKind<usize> for () {
    fn args(_input_arg: (), count: usize) -> impl ExactSizeIterator<Item = usize> {
        0..count
    }
}

// FIXME(in_place_init): thread IsZero through this somehow, to get the
// same optimization as Vec's SpecFromElem
macro_rules! repeat_impls {
    ($(#[$($arg_docs:tt)*])* $Name:ident: [$($Arg:ident $(: $arg_bound:ident)?)?] IArg = $IArg:ty, OArg = $OArg:ty) => {
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: PinInitMut<T, Error, $OArg>>
            PinInitOnce<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
            fn metadata(this: &Self) -> Metadata<[T]> {
                build_metadata!(len: this.length.length(), elem: I::metadata(&this.elem), ..)
            }

            fn should_zero(this: &Self) -> bool {
                this.length.length() > 0 && <I as PinInitOnce<T, Error, $OArg>>::should_zero(&this.elem)
            }

            unsafe fn init_once(
                mut this: Self,
                dst: &mut MaybeUninit<[T]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                let dst = dst.transpose_mut();
                let count = this.length.length();
                debug_assert_eq!(dst.len(), count);
                // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
                let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
                let elem = &mut this.elem;
                for (idx, arg) in <$IArg as ArgKind<$OArg>>::args(arg, count).enumerate() {
                    // we do this so the last element can be initialized using `init_once`,
                    // which could avoid cloning in `I::init_mut`.
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
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: PinInitMut<T, Error, $OArg>>
            PinInitMut<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
            unsafe fn init_mut(
                this: &mut Self,
                dst: &mut MaybeUninit<[T]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                let dst = dst.transpose_mut();
                let count = this.length.length();
                debug_assert_eq!(dst.len(), count);
                // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
                let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
                let elem = &mut this.elem;
                for (idx, arg) in <$IArg as ArgKind<$OArg>>::args(arg, count).enumerate() {
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
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: PinInit<T, Error, $OArg>>
            PinInit<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
            unsafe fn init_ref(
                this: &Self,
                dst: &mut MaybeUninit<[T]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                let dst = dst.transpose_mut();
                let count = this.length.length();
                debug_assert_eq!(dst.len(), count);
                // SAFETY: we only modify `buf.initialized_len` after we have initialized the relevant parts of `buf.data`
                let mut buf = unsafe { InitializingSlice::from_fully_uninit(dst) };
                let elem = &this.elem;
                for (idx, arg) in <$IArg as ArgKind<$OArg>>::args(arg, count).enumerate() {
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
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: InitMut<T, Error, $OArg>>
            InitOnce<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
        }
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: InitMut<T, Error, $OArg>>
            InitMut<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
        }
        /// Initialize a slice by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, L: Length, Error, $($Arg $(: $arg_bound)? ,)? I: Init<T, Error, $OArg>>
            Init<[T], Error, $IArg> for $Name<I, L, $OArg>
        {
        }

        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: PinInitMut<T, Error, $OArg>>
            PinInitOnce<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
            fn metadata(this: &Self) -> Metadata<[T; N]> {
                build_metadata!(elem: I::metadata(&this.elem), ..)
            }

            fn should_zero(this: &Self) -> bool {
                N > 0 && <I as PinInitOnce<T, Error, $OArg>>::should_zero(&this.elem)
            }

            unsafe fn init_once(
                this: Self,
                dst: &mut MaybeUninit<[T; N]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                // SAFETY: delegated to caller
                unsafe { <Self as PinInitOnce<[T], Error, $IArg>>::init_once(this, dst, arg, pre_zeroed) }
            }
        }
        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: PinInitMut<T, Error, $OArg>>
            PinInitMut<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
            unsafe fn init_mut(
                this: &mut Self,
                dst: &mut MaybeUninit<[T; N]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                // SAFETY: delegated to caller
                unsafe { <Self as PinInitMut<[T], Error, $IArg>>::init_mut(this, dst, arg, pre_zeroed) }
            }
        }
        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: PinInit<T, Error, $OArg>>
            PinInit<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
            unsafe fn init_ref(
                this: &Self,
                dst: &mut MaybeUninit<[T; N]>,
                arg: $IArg,
                pre_zeroed: bool,
            ) -> Result<(), Error> {
                // SAFETY: delegated to caller
                unsafe { <Self as PinInit<[T], Error, $IArg>>::init_ref(this, dst, arg, pre_zeroed) }
            }
        }
        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: InitMut<T, Error, $OArg>>
            InitOnce<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
        }
        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: InitMut<T, Error, $OArg>>
            InitMut<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
        }
        /// Initialize an array by repeating an element initializer,
        $(#[$($arg_docs)*])*
        unsafe impl<T: MetaSized, const N: usize, Error, $($Arg $(: $arg_bound)? ,)? I: Init<T, Error, $OArg>>
            Init<[T; N], Error, $IArg> for $Name<I, ConstLength<N>, $OArg>
        {
        }

    };
}

/// Initialize an array or slice by re-using an initializer for each element.
///
/// Created using [`repeat_array`] and [`repeat_slice`].
///
/// ```rust
/// #![feature(in_place_init)]
/// let bx: Box<[usize; 3]> = Box::build(
///     std::init::repeat_array(42)
/// );
/// assert_eq!(*bx, [42, 42, 42]);
///
/// let bx: Box<[usize]> = Box::build(
///     std::init::repeat_slice(
///         std::init::from_fn_with_arg(|idx: usize| idx * 2 + 1),
///         3
///     )
/// );
/// assert_eq!(*bx, [1, 3, 5]);
/// ```
///
/// [`repeat_array`]: crate::init::repeat_array
/// [`repeat_slice`]: crate::init::repeat_slice
#[derive(Debug, Clone)]
pub struct Repeat<I, L: Length = RuntimeLength, ElemArg = ()> {
    elem: I,
    length: L,
    _arg: PhantomData<fn(ElemArg) -> ElemArg>,
}
impl<I, ElemArg> Repeat<I, RuntimeLength, ElemArg> {
    pub(crate) const fn new_slice(length: usize, elem: I) -> Self {
        Self { length: RuntimeLength { length }, elem, _arg: PhantomData }
    }
    pub(crate) const fn new_array<const N: usize>(elem: I) -> Repeat<I, ConstLength<N>, ElemArg> {
        Repeat { length: ConstLength, elem, _arg: PhantomData }
    }
}
repeat_impls! {
    /// cloning the initializer argument for each element, if given.
    Repeat: [Arg: Clone] IArg = Arg, OArg = Arg
}
repeat_impls! {
    /// cloning the initializer argument for each element, if given,
    /// and additionally passing the element index.
    Repeat: [Arg: Clone] IArg = Arg, OArg = (Arg, usize)
}
repeat_impls! {
    /// passing the element index as the element initializer argument.
    Repeat: [] IArg = (), OArg = usize
}

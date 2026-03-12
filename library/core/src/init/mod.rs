//! In-place initialization.

#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{AsBytes, as_bytes};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{Chain, chain};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{FnNoArg, FnWithArg, FromFn, from_fn, from_fn_with_arg};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{Repeat, repeat_array, repeat_slice};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{
    Uninit, uninit, uninit_slice, uninit_slice_unchecked, uninit_unchecked, uninit_with_metadata,
    uninit_with_metadata_unchecked,
};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{WithArg, with_arg};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{
    Zeroed, zeroed, zeroed_slice, zeroed_slice_unchecked, zeroed_str, zeroed_unchecked,
    zeroed_with_metadata, zeroed_with_metadata_unchecked,
};
#[unstable(feature = "in_place_init", issue = "none")]
pub use dyn_init::DynInit;
#[unstable(feature = "in_place_init", issue = "none")]
pub use util::{ConstLength, Length, RuntimeLength};
#[doc(hidden)]
#[unstable(feature = "std_internals", issue = "none")]
pub use util::{IsZero, NoneIsZero};

use crate::clone::CloneToUninit;
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata, metadata};

mod adapters;
mod dyn_init;
mod util;

/// A trait for pinned in-place initializers.
///
/// # Safety
///
/// See the documentation for [`metadata`][PinInitOnce::metadata] and [`init_once`][PinInitOnce::init_once].
#[lang = "pin_init_once"]
pub unsafe trait PinInitOnce<T: MetaSized, Error = !, Arg = ()> {
    /// The pointer metadata for the value that this initializer will create.
    ///
    /// # Safety
    ///
    /// ## Callers
    ///
    /// This function has no preconditions.
    ///
    /// This function may panic or otherwise diverge, unless `T: Thin`.
    ///
    /// Note that the layout of a `T` with the metadata returned by this function may not be computible,
    /// e.g. for a slice, this may return a length too long to be representable in memory. Callers
    /// should use [`core::mem::checked_size_for_meta`] and related functions to compute the required layout,
    /// and should return an allocation failure, or other appropriate error, if the layout is not computible.
    ///
    /// ## Implementors
    ///
    /// * This method must return an equal value (or diverge) each time it is called, if there are not intermediate modifications to `this`,
    ///   similar to [`DerefPure`](core::ops::DerefPure).
    ///     * Additionally, if `Self: PinInitMut<T, Error, Arg>`, then calling `Self::init_mut` may not cause `metadata`'s result to change.
    /// * If `Self` implements `PinInitOnce<T, Error, Arg>` for additional `Error` and/or `Arg`, this function must return the same value (or diverge) in all such implementations.
    ///     * Note the same is not required with respect to `T` for types which implement `PinInitOnce<T, _, _>` for multiple `T`.
    /// * If `Self` implements `Clone` (or `Copy`), then any clones (or copies) must return the same value.
    /// * If `T: Thin`, this function must not diverge or have any observable side-effects.
    /// * If `T: Thin`, callers are not required to call this function.
    fn metadata(this: &Self) -> Metadata<T>;

    /// Whether this initializer requests that the destination be pre-initialized with all zero bytes before calling [`PinInitOnce::init_once`].
    ///
    /// Note that this function returning `true` does not mean that the caller is *required* to
    /// pre-zero the destination before calling [`PinInitOnce::init_once`], only that doing so could be beneficial.
    ///
    /// # Safety
    ///
    /// ## Callers
    ///
    /// This function has no preconditions.
    ///
    /// ## Implementors
    ///
    /// This function must not diverge.
    #[inline]
    fn should_zero(_this: &Self) -> bool {
        false
    }

    /// Initialize a `T` value into the provided destination.
    ///
    /// `arg` allows for callers to pass in extra data that may only be available after knowing the metadata, e.g. in `Rc::new_cyclic`.
    ///
    /// `pre_zeroed` allows for optimization in some cases where allocators can provide pre-zeroed memory.
    ///
    /// # Safety
    ///
    /// ## Callers
    ///
    /// * The metadata of `dst` is be the value returned by `Self::metadata(this)`, with no intermediate modifications to `this`.
    ///     * Except if `Self: PinInitMut<T, Error, Arg>`, where calling `Self::init_mut` may not cause `Self::metadata`'s result to change.
    ///     * Except if `T: Thin`, where it is not required to call `Self::metadata(this)`, and thus this call has no preconditions.
    ///
    /// This function may panic or return `Err(_)`, in which case `*dst` must be treated as uninitialized.
    ///
    /// If this function returns `Ok(())`, then `*dst` should be treated as a fully initialized `T`,
    /// and must be treated as pinned (unless `Self` additionally implements [`Init<T, Error, Arg>`]).
    ///
    /// If `*dst` was pre-filled with zeroed bytes, `pre_zeroed` can be `true`, otherwise it must be `false`.
    ///
    /// ## Implementors
    ///
    /// If this function returns `Ok(())`, then `*dst` must be a fully initialized `T`.
    ///
    /// If this function panics or returns `Err(_)`, then it should drop any partially-initialized parts of the destination.
    /// This is not a safety requirement, but failing to do so may cause resource leaks.
    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error>;
}

/// A trait for non-pinned in-place initializers.
///
/// # Safety
///
/// See [`PinInitOnce`].
///
/// [`PinInitOnce::init_once`]'s caller requirements are relaxed to not necessarily treat `*dst` as pinned.
#[lang = "init_once"]
pub unsafe trait InitOnce<T: MetaSized, Error = !, Arg = ()>:
    PinInitOnce<T, Error, Arg>
{
}

/// A trait for pinned in-place initializers which can be used multiple times.
///
/// # Safety
///
/// See the documentation for [`metadata`][PinInitOnce::metadata] and [`init_once`][PinInitOnce::init_once].
///
/// Additionally, the return value of `Self::metadata` must not change after a call to `Self::init_mut`.
#[lang = "pin_init_mut"]
pub unsafe trait PinInitMut<T: MetaSized, Error = !, Arg = ()>:
    PinInitOnce<T, Error, Arg>
{
    /// Initialize a `T` value into the provided destination,
    ///
    /// # Safety
    ///
    /// See [`PinInitOnce::init_once`] for safety requirements.
    ///
    /// Additionally, for implementors: calling `Self::init_mut` may not cause `Self::metadata`'s result to change.
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error>;
}

/// A trait for non-pinned in-place initializers.
///
/// # Safety
///
/// See [`PinInitOnce`].
///
/// [`PinInitOnce::init_once`]'s caller requirements are relaxed to not necessarily treat `*dst` as pinned.
#[lang = "init_mut"]
pub unsafe trait InitMut<T: MetaSized, Error = !, Arg = ()>:
    InitOnce<T, Error, Arg> + PinInitMut<T, Error, Arg>
{
}

/// A trait for pinned in-place initializers which can be used multiple times without mutating state.
///
/// # Safety
///
/// See the documentation for [`metadata`][PinInitOnce::metadata] and [`init_once`][PinInitOnce::init_once].
#[lang = "pin_init"]
pub unsafe trait PinInit<T: MetaSized, Error = !, Arg = ()>:
    PinInitMut<T, Error, Arg>
{
    /// Initialize a `T` value into the provided destination,
    ///
    /// # Safety
    ///
    /// See [`PinInitOnce::init_once`] for safety requirements.
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: Arg,
        pre_zeroed: bool,
    ) -> Result<(), Error>;
}

/// A trait for non-pinned in-place initializers.
///
/// # Safety
///
/// See [`PinInitOnce`].
///
/// [`PinInitOnce::init_once`]'s caller requirements are relaxed to not necessarily treat `*dst` as pinned.
#[lang = "init"]
pub unsafe trait Init<T: MetaSized, Error = !, Arg = ()>:
    InitMut<T, Error, Arg> + PinInit<T, Error, Arg>
{
}

/// Initialize a place by writing an existing value
unsafe impl<T: MetaSized, Error> PinInitOnce<T, Error> for T {
    fn metadata(this: &Self) -> Metadata<T> {
        crate::ptr::metadata(this)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // Could be `dst.write(this);`, but that doesn't support unsized types.
        let size = size_of_val::<Self>(&this);
        // SAFETY: `this` will be forgotten immediately, so this is semantically a move
        unsafe {
            crate::ptr::copy_nonoverlapping(
                (&raw const this).cast::<u8>(),
                dst.as_mut_ptr().cast(),
                size,
            );
        }
        crate::mem::forget_unsized(this);
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> InitOnce<T, Error> for T {}

/// Initialize a place by cloning an existing value
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInitMut<T, Error> for T {
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe {
            T::clone_to_uninit(this, dst.as_mut_ptr().cast());
        }
        Ok(())
    }
}
/// Initialize a place by cloning an existing value
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInit<T, Error> for T {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe {
            T::clone_to_uninit(this, dst.as_mut_ptr().cast());
        }
        Ok(())
    }
}
unsafe impl<T: MetaSized + CloneToUninit, Error> InitMut<T, Error> for T {}
unsafe impl<T: MetaSized + CloneToUninit, Error> Init<T, Error> for T {}

/// Initialize a place by writing an existing value
unsafe impl<T, Error> PinInitOnce<T, Error> for Result<T, Error> {
    fn metadata(_this: &Self) -> Metadata<T> {
        build_metadata!(..)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        dst.write(this?);
        Ok(())
    }
}
unsafe impl<T, Error> InitOnce<T, Error> for Result<T, Error> {}

/// Initialize a slice with an array of a given length.
unsafe impl<T, Error, const N: usize> PinInitOnce<[T], Error> for [T; N] {
    fn metadata(_this: &Self) -> Metadata<[T]> {
        build_metadata!(len: N, ..)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        debug_assert_eq!(dst.len(), N);
        dst.transpose_mut().as_mut_array().unwrap().transpose_mut().write(this);
        Ok(())
    }
}
unsafe impl<T, Error, const N: usize> InitOnce<[T], Error> for [T; N] {}

/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> PinInitOnce<[T], Error>
    for &[T; N]
{
    fn metadata(this: &Self) -> Metadata<[T]> {
        let array_meta = metadata::<[T; N]>(*this);
        build_metadata!(len: N, elem: array_meta.elem, ..)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: discharged to caller
        unsafe { <&[T] as PinInitOnce<[T], Error>>::init_once(this, dst, (), pre_zeroed) }
    }
}
/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> PinInitMut<[T], Error>
    for &[T; N]
{
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T]>,
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: discharged to caller
        unsafe { <&[T] as PinInitOnce<[T], Error>>::init_once(*this, dst, (), pre_zeroed) }
    }
}
/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> PinInit<[T], Error> for &[T; N] {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T]>,
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: discharged to caller
        unsafe { <&[T] as PinInitOnce<[T], Error>>::init_once(*this, dst, (), pre_zeroed) }
    }
}
/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> InitOnce<[T], Error> for &[T; N] {}
/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> InitMut<[T], Error> for &[T; N] {}
/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: MetaSized + CloneToUninit, Error, const N: usize> Init<[T], Error> for &[T; N] {}

/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInitOnce<T, Error> for &T {
    fn metadata(this: &Self) -> Metadata<T> {
        core::ptr::metadata::<T>(*this)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `dst` comes from a mutable reference, so it is valid for writes and well-aligned
        unsafe {
            T::clone_to_uninit(this, dst.as_mut_ptr().cast());
        }
        Ok(())
    }
}
/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInitMut<T, Error> for &T {
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitOnce<T, Error>>::init_once(*this, dst, (), pre_zeroed) }
    }
}
/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInit<T, Error> for &T {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <Self as PinInitOnce<T, Error>>::init_once(*this, dst, (), pre_zeroed) }
    }
}
/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> InitOnce<T, Error> for &T {}
/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> InitMut<T, Error> for &T {}
/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> Init<T, Error> for &T {}

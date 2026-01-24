//! In-place initialization.

#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{AsBytes, as_bytes};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{Chain, chain};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{Repeat, repeat};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{
    Uninit, uninit, uninit_slice, uninit_slice_unchecked, uninit_unchecked, uninit_with_metadata,
    uninit_with_metadata_unchecked,
};
#[unstable(feature = "in_place_init", issue = "none")]
pub use adapters::{
    Zeroed, zeroed, zeroed_slice, zeroed_slice_unchecked, zeroed_str, zeroed_unchecked,
    zeroed_with_metadata, zeroed_with_metadata_unchecked,
};

use crate::clone::CloneToUninit;
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, build_metadata};

mod adapters;

/// A trait for pinned in-place initializers.
///
/// # Safety
///
/// See the documentation for [`metadata`][PinInit::metadata] and [`init`][PinInit::init].
#[lang = "pin_init"]
pub unsafe trait PinInit<T: MetaSized, Error = !, Arg = ()>: Sized {
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
    /// * If `Self` implements `PinInit<T, Error, Arg>` for additional `Error` and/or `Arg`, this function must return the same value (or diverge) in all such implementations.
    ///     * Note the same is not required with respect to `T` for types which implement `PinInit<T, _, _>` for multiple `T`.
    /// * If `Self` implements `Clone` (or `Copy`), then any clones (or copies) must return the same value.
    /// * If `T: Thin`, this function must not diverge or have any observable side-effects.
    /// * If `T: Thin`, callers are not required to call this function.
    fn metadata(this: &Self) -> Metadata<T>;

    /// Whether this initializer requests that the destination be pre-initialized with all zero bytes before calling [`PinInit::init`].
    ///
    /// Note that this function returning `true` does not mean that the caller is *required* to
    /// pre-zero the destination before calling [`PinInit::init`], only that doing so could be beneficial.
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
    /// `extra` allows for callers to pass in extra data that may only be available after knowing the metadata, e.g. in `Rc::new_cyclic`.
    ///
    /// `pre_zeroed` allows for optimization in some cases where allocators can provide pre-zeroed memory.
    ///
    /// # Safety
    ///
    /// ## Callers
    ///
    /// * The metadata of `dst` is be the value returned by `Self::metadata(this)`, with no intermediate modifications to `this`.
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
    unsafe fn init(
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
/// See [`PinInit`].
///
/// [`PinInit::init`]'s caller requirements are relaxed to not necessarily treat `*dst` as pinned.
#[lang = "init"]
pub unsafe trait Init<T: MetaSized, Error = !, Extra = ()>:
    PinInit<T, Error, Extra>
{
}

/// Initialize a place by writing an existing value
unsafe impl<T, Error> PinInit<T, Error> for T {
    fn metadata(_this: &Self) -> Metadata<T> {
        build_metadata!(..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        dst.write(this);
        Ok(())
    }
}
unsafe impl<T, Error> Init<T, Error> for T {}

/// Initialize a place by writing an existing value
unsafe impl<T, Error> PinInit<T, Error> for Result<T, Error> {
    fn metadata(_this: &Self) -> Metadata<T> {
        build_metadata!(..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        dst.write(this?);
        Ok(())
    }
}
unsafe impl<T, Error> Init<T, Error> for Result<T, Error> {}

/// Initialize a slice with an array of a given length.
unsafe impl<T, Error, const N: usize> PinInit<[T], Error> for [T; N] {
    fn metadata(_this: &Self) -> Metadata<[T]> {
        build_metadata!(len: N, ..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        _: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        debug_assert_eq!(dst.len(), N);
        dst.transpose_mut().as_mut_array().unwrap().transpose_mut().write(this);
        Ok(())
    }
}
unsafe impl<T, Error, const N: usize> Init<[T], Error> for [T; N] {}

/// Initialize a slice by cloning from an array of a given length.
unsafe impl<T: Clone, Error, const N: usize> PinInit<[T], Error> for &[T; N] {
    fn metadata(_this: &Self) -> Metadata<[T]> {
        build_metadata!(len: N, ..)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<[T]>,
        _: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: discharged to caller
        unsafe { <&[T] as PinInit<[T], Error>>::init(this, dst, (), pre_zeroed) }
    }
}
unsafe impl<T: Clone, Error, const N: usize> Init<[T], Error> for &[T; N] {}

/// Initialize a place by cloning an existing value.
unsafe impl<T: MetaSized + CloneToUninit, Error> PinInit<T, Error> for &T {
    fn metadata(this: &Self) -> Metadata<T> {
        core::ptr::metadata::<T>(*this)
    }

    unsafe fn init(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: `dst` comes from a mutable reference, so it is valid for writes and well-aligned
        unsafe {
            T::clone_to_uninit(this, dst.as_mut_ptr().cast());
        }
        Ok(())
    }
}
unsafe impl<T: MetaSized + CloneToUninit, Error> Init<T, Error> for &T {}

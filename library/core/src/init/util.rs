pub use is_zero::{IsZero, NoNicheMetadata, NoneIsZero};

use crate::marker::MetaSized;
use crate::mem::MaybeUninit;

mod is_zero;

/// Used by [`init`] APIs to represent the length of an array or slice.
///
/// # Safety
///
/// ## Implementors
///
/// * `length` must return the same value for `self` and clones/copies of `self`, if there are no intermediate modifications,
///   similar to [`DerefPure`](crate::ops::DerefPure).
/// * `length` may panic or diverge
///
/// [`init`]: crate::init
pub unsafe trait Length: Copy {
    /// The length that this value represents.
    fn length(&self) -> usize;
}

/// A constant length, used to initialize arrays and slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstLength<const N: usize>;

unsafe impl<const N: usize> Length for ConstLength<N> {
    fn length(&self) -> usize {
        N
    }
}

/// A runtimte-known length, used to initialize slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RuntimeLength {
    /// The length of the slice.
    pub length: usize,
}

unsafe impl Length for RuntimeLength {
    fn length(&self) -> usize {
        self.length
    }
}

pub(super) struct InitializingSlice<'a, T: MetaSized> {
    pub data: &'a mut [MaybeUninit<T>],
    pub initialized_len: usize,
    _priv: (),
}

impl<'a, T: MetaSized> InitializingSlice<'a, T> {
    /// # Safety:
    ///
    /// Fields are accessed correctly (i.e. `self.data[..initialized_len]` is always initialized).
    pub(super) unsafe fn from_fully_uninit(data: &'a mut [MaybeUninit<T>]) -> Self {
        Self { data, initialized_len: 0, _priv: () }
    }
}

impl<'a, T: MetaSized> Drop for InitializingSlice<'a, T> {
    #[cold] // will only be invoked on unwind
    fn drop(&mut self) {
        // SAFETY:
        // * the pointer is valid because it was made from a mutable reference
        // * `initialized_len` counts the initialized elements as an invariant of this type,
        //   so each of the pointed-to elements is initialized and may be dropped.
        unsafe { self.data[..self.initialized_len].assume_init_drop() };
    }
}

use super::TrivialClone;
use crate::clone::CloneToUninit;
use crate::marker::{Destruct, MetaSized};
use crate::mem::{self, MaybeUninit};
use crate::ptr;

/// Private specialization trait used by CloneToUninit, as per
/// [the dev guide](https://std-dev-guide.rust-lang.org/policy/specialization.html).
#[rustc_const_unstable(feature = "const_clone", issue = "142757")]
pub(super) const unsafe trait CopySpec: CloneToUninit {
    unsafe fn clone_one(src: &Self, dst: *mut Self)
    where
        Self: [const] Clone;
    unsafe fn clone_slice(src: &[Self], dst: *mut [Self]);
}

#[rustc_const_unstable(feature = "const_clone", issue = "142757")]
unsafe impl<T: MetaSized + [const] CloneToUninit> const CopySpec for T {
    #[inline]
    default unsafe fn clone_one(src: &Self, dst: *mut Self)
    where
        Self: [const] Clone,
    {
        // SAFETY: The safety conditions of clone_to_uninit() are a superset of those of
        // ptr::write().
        unsafe {
            // We hope the optimizer will figure out to create the cloned value in-place,
            // skipping ever storing it on the stack and the copy to the destination.
            ptr::write(dst, src.clone());
        }
    }

    #[inline]
    #[cfg_attr(debug_assertions, track_caller)]
    default unsafe fn clone_slice(src: &[Self], dst: *mut [Self]) {
        let len = src.len();
        // This is the most likely mistake to make, so check it as a debug assertion.
        debug_assert!(
            len == dst.len(),
            "clone_to_uninit() source and destination must have equal lengths",
        );

        // SAFETY: The produced `&mut` is valid because:
        // * The caller is obligated to provide a pointer which is valid for writes.
        // * All bytes pointed to are in MaybeUninit, so we don't care about the memory's
        //   initialization status.
        let uninit_ref = unsafe { &mut *(dst as *mut [MaybeUninit<T>]) };

        // Copy the elements
        let mut initializing = InitializingSlice::from_fully_uninit(uninit_ref);
        let mut i = 0;
        while i < len {
            let element_ref = &src[i];
            // If the clone_to_uninit() panics, `initializing` will take care of the cleanup.
            // SAFETY: delegated to caller
            unsafe { initializing.clone_push(element_ref) };
            i += 1;
        }
        // If we reach here, then the entire slice is initialized, and we've satisfied our
        // responsibilities to the caller. Disarm the cleanup guard by forgetting it.
        mem::forget(initializing);
    }
}

// Specialized implementation for types that are [`TrivialClone`], not just [`Clone`],
// and can therefore be copied bitwise.
#[rustc_const_unstable(feature = "const_clone", issue = "142757")]
unsafe impl<T: [const] CloneToUninit + TrivialClone> const CopySpec for T {
    #[inline]
    unsafe fn clone_one(src: &Self, dst: *mut Self) {
        // SAFETY: The safety conditions of clone_to_uninit() are a superset of those of
        // ptr::copy_nonoverlapping().
        unsafe {
            ptr::copy_nonoverlapping(src, dst, 1);
        }
    }

    #[inline]
    #[cfg_attr(debug_assertions, track_caller)]
    unsafe fn clone_slice(src: &[Self], dst: *mut [Self]) {
        let len = src.len();
        // This is the most likely mistake to make, so check it as a debug assertion.
        debug_assert!(
            len == dst.len(),
            "clone_to_uninit() source and destination must have equal lengths",
        );

        // SAFETY: The safety conditions of clone_to_uninit() are a superset of those of
        // ptr::copy_nonoverlapping().
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), len);
        }
    }
}

/// Ownership of a collection of values stored in a non-owned `[MaybeUninit<T>]`, some of which
/// are not yet initialized. This is sort of like a `Vec` that doesn't own its allocation.
/// Its responsibility is to provide cleanup on unwind by dropping the values that *are*
/// initialized, unless disarmed by forgetting.
///
/// This is a helper for `impl<T: Clone> CloneToUninit for [T]`.
struct InitializingSlice<'a, T: MetaSized> {
    data: &'a mut [MaybeUninit<T>],
    /// Number of elements of `*self.data` that are initialized.
    initialized_len: usize,
}

impl<'a, T: MetaSized> InitializingSlice<'a, T> {
    #[inline]
    const fn from_fully_uninit(data: &'a mut [MaybeUninit<T>]) -> Self {
        Self { data, initialized_len: 0 }
    }

    /// Push a value onto the end of the initialized part of the slice.
    ///
    /// # Panics
    ///
    /// Panics if the slice is already fully initialized.
    ///
    /// # Safety
    ///
    /// `value`'s metadata must be the same as the element metadata of `self.data`.
    #[inline]
    #[rustc_const_unstable(feature = "const_clone", issue = "142757")]
    const unsafe fn clone_push(&mut self, value: &T)
    where
        T: [const] CloneToUninit,
    {
        let dst = &mut self.data[self.initialized_len];
        // SAFETY:
        // delegated to caller
        unsafe { T::clone_to_uninit(value, dst.as_mut_ptr().cast()) };
        self.initialized_len += 1;
    }
}

#[rustc_const_unstable(feature = "const_clone", issue = "142757")]
impl<'a, T: MetaSized + [const] Destruct> const Drop for InitializingSlice<'a, T> {
    #[cold] // will only be invoked on unwind
    fn drop(&mut self) {
        // SAFETY:
        // * the pointer is valid because it was made from a mutable reference
        // * `initialized_len` counts the initialized elements as an invariant of this type,
        //   so each of the pointed-to elements is initialized and may be dropped.
        unsafe { self.data[..self.initialized_len].assume_init_drop() };
    }
}

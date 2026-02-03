use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// An initializer that does not initialize.
pub struct Uninit<T: MetaSized> {
    metadata: Metadata<T>,
}

impl<T: MetaSized> crate::fmt::Debug for Uninit<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_struct("Uninit").field("metadata", &self.metadata).finish()
    }
}

impl<T: MetaSized> Clone for Uninit<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: MetaSized> Copy for Uninit<T> {}

impl<T: MetaSized> Uninit<T> {
    /// Create an initializer that leaves a [`MaybeUninit<T>`] uninitialized, with a given pointer metadata.
    pub(super) const fn new(metadata: Metadata<T>) -> Uninit<MaybeUninit<T>> {
        Uninit { metadata: metadata as Metadata<MaybeUninit<T>> }
    }

    /// Create an initializer leaves a `T` uninitialized.
    ///
    /// # Safety
    ///
    /// A `T` consisting of all uninitialized bytes, with pointer metadata `meta` must be valid.
    pub(super) const unsafe fn new_unchecked(metadata: Metadata<T>) -> Self {
        Self { metadata }
    }
}

unsafe impl<T: MetaSized, Error> PinInitOnce<T, Error> for Uninit<T> {
    fn metadata(this: &Self) -> Metadata<T> {
        this.metadata
    }

    fn should_zero(_this: &Self) -> bool {
        false
    }

    unsafe fn init_once(
        _this: Self,
        _dst: &mut MaybeUninit<T>,
        _extra: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // `Self` can only be constructed if `T` with meta `this.meta`
        // is valid as all uninitialized bytes.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> PinInitMut<T, Error> for Uninit<T> {
    unsafe fn init_mut(
        _this: &mut Self,
        _dst: &mut MaybeUninit<T>,
        _extra: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // `Self` can only be constructed if `T` with meta `this.meta`
        // is valid as all uninitialized bytes.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> PinInit<T, Error> for Uninit<T> {
    unsafe fn init_ref(
        _this: &Self,
        _dst: &mut MaybeUninit<T>,
        _extra: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // `Self` can only be constructed if `T` with meta `this.meta`
        // is valid as all uninitialized bytes.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> InitOnce<T, Error> for Uninit<T> {}
unsafe impl<T: MetaSized, Error> InitMut<T, Error> for Uninit<T> {}
unsafe impl<T: MetaSized, Error> Init<T, Error> for Uninit<T> {}

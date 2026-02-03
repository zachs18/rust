use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::MetaSized;
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// An initializer that initializes all bytes to zero
pub struct Zeroed<T: MetaSized> {
    metadata: Metadata<T>,
}

impl<T: MetaSized> crate::fmt::Debug for Zeroed<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_struct("Zeroed").field("metadata", &self.metadata).finish()
    }
}

impl<T: MetaSized> Clone for Zeroed<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: MetaSized> Copy for Zeroed<T> {}

impl<T: MetaSized> Zeroed<T> {
    /// Create an initializer that initializes a [`MaybeUninit<T>`] with all bytes zero.
    pub(super) const fn new(metadata: Metadata<T>) -> Zeroed<MaybeUninit<T>> {
        Zeroed { metadata: metadata as Metadata<MaybeUninit<T>> }
    }

    /// Create an initializer that initializes a `T` with all bytes zero.
    ///
    /// # Safety
    ///
    /// A `T` consisting of all bytes zero, with pointer metadata `metadata` must be valid.
    pub(super) const unsafe fn new_unchecked(metadata: Metadata<T>) -> Self {
        Self { metadata }
    }
}

unsafe impl<T: MetaSized, Error> PinInitOnce<T, Error> for Zeroed<T> {
    fn metadata(this: &Self) -> Metadata<T> {
        this.metadata
    }

    fn should_zero(_this: &Self) -> bool {
        true
    }

    unsafe fn init_once(
        _this: Self,
        dst: &mut MaybeUninit<T>,
        _extra: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        if !pre_zeroed {
            dst.as_bytes_mut().write_filled(0);
        }
        // `Self` can only be constructed if `T` with meta `this.metadata`
        // is valid as all bytes zero.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> PinInitMut<T, Error> for Zeroed<T> {
    unsafe fn init_mut(
        _this: &mut Self,
        dst: &mut MaybeUninit<T>,
        _extra: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        if !pre_zeroed {
            dst.as_bytes_mut().write_filled(0);
        }
        // `Self` can only be constructed if `T` with meta `this.metadata`
        // is valid as all bytes zero.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> PinInit<T, Error> for Zeroed<T> {
    unsafe fn init_ref(
        _this: &Self,
        dst: &mut MaybeUninit<T>,
        _extra: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        if !pre_zeroed {
            dst.as_bytes_mut().write_filled(0);
        }
        // `Self` can only be constructed if `T` with meta `this.metadata`
        // is valid as all bytes zero.
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error> InitOnce<T, Error> for Zeroed<T> {}
unsafe impl<T: MetaSized, Error> InitMut<T, Error> for Zeroed<T> {}
unsafe impl<T: MetaSized, Error> Init<T, Error> for Zeroed<T> {}

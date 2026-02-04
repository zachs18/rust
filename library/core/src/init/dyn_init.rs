use crate::fmt;
use crate::init::{Init, InitMut, InitOnce, PinInit, PinInitMut, PinInitOnce};
use crate::marker::{MetaSized, Tuple};
use crate::mem::MaybeUninit;
use crate::ptr::Metadata;

/// A concrete type for returning dynamic initializers from trait methods.
pub struct DynInit<T: MetaSized, A: Tuple> {
    metadata: Metadata<T>,
    callback: unsafe fn(*mut (), A),
}

impl<T: MetaSized, A: Tuple> Clone for DynInit<T, A> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: MetaSized, A: Tuple> Copy for DynInit<T, A> {}

impl<T: MetaSized, A: Tuple> fmt::Debug for DynInit<T, A> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Formatter::debug_struct_field2_finish(
            f,
            "DynInit",
            "metadata",
            &self.metadata,
            "callback",
            &self.callback,
        )
    }
}

impl<T: MetaSized, A: Tuple> DynInit<T, A> {
    /// # Safety
    ///
    /// `callback` must be safe to call with a `*mut ()` that points to a uniquely owned span of
    /// memory correctly allocated for a `T` with the given `metadata`. If `callback` returns
    /// successfully, the `*mut ()` must point to an initialized `T` valid for the given metadata.
    ///
    /// `callback` may panic, in which case the pointee should be treated as uninitialized.
    pub fn new_unchecked(metadata: Metadata<T>, callback: unsafe fn(*mut (), A)) -> Self {
        Self { metadata, callback }
    }
}

unsafe impl<T: MetaSized, Error, A: Tuple> PinInitOnce<T, Error, A> for DynInit<T, A> {
    fn metadata(this: &Self) -> Metadata<T> {
        this.metadata
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        arg: A,
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller, and `DynInit` constructor
        unsafe {
            (this.callback)(dst.as_mut_ptr().cast(), arg);
        }
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error, A: Tuple> PinInitMut<T, Error, A> for DynInit<T, A> {
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: A,
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller, and `DynInit` constructor
        unsafe {
            (this.callback)(dst.as_mut_ptr().cast(), arg);
        }
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error, A: Tuple> PinInit<T, Error, A> for DynInit<T, A> {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: A,
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller, and `DynInit` constructor
        unsafe {
            (this.callback)(dst.as_mut_ptr().cast(), arg);
        }
        Ok(())
    }
}
unsafe impl<T: MetaSized, Error, A: Tuple> InitOnce<T, Error, A> for DynInit<T, A> {}
unsafe impl<T: MetaSized, Error, A: Tuple> InitMut<T, Error, A> for DynInit<T, A> {}
unsafe impl<T: MetaSized, Error, A: Tuple> Init<T, Error, A> for DynInit<T, A> {}

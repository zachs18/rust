//! In-place initialization.

use core::alloc::{Allocator, Layout};
use core::clone::CloneToUninit;
pub use core::init::*;
use core::marker::PointeeSized;
use core::mem::{self, MaybeUninit};
use core::ptr::{self, Metadata};

use crate::alloc::Global;
use crate::boxed::Box;
use crate::vec::Vec;

/// Kinds of errors that can occur when creating a value on the heap.
pub enum BuildErrorKind<T: ?Sized, E = !> {
    /// The layout for a value with this pointer metadata cannot be computed.
    /// Should never occur when `T: Sized`.
    LayoutOverflow(Metadata<T>),
    /// Allocation failed for this layout.
    AllocError(Layout),
    /// Initialization failed with this error.
    InitError(E),
}

impl<T: ?Sized, E: core::fmt::Debug> core::fmt::Debug for BuildErrorKind<T, E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::LayoutOverflow(arg0) => f.debug_tuple("LayoutOverflow").field(arg0).finish(),
            Self::AllocError(arg0) => f.debug_tuple("AllocError").field(arg0).finish(),
            Self::InitError(arg0) => f.debug_tuple("InitError").field(arg0).finish(),
        }
    }
}

impl<T: ?Sized, E> BuildErrorKind<T, E> {
    #[cfg(not(no_rc))]
    pub(crate) fn map_metadata<U: ?Sized>(
        self,
        f: impl FnOnce(Metadata<T>) -> Metadata<U>,
    ) -> BuildErrorKind<U, E> {
        match self {
            BuildErrorKind::LayoutOverflow(metadata) => BuildErrorKind::LayoutOverflow(f(metadata)),
            BuildErrorKind::AllocError(layout) => BuildErrorKind::AllocError(layout),
            BuildErrorKind::InitError(err) => BuildErrorKind::InitError(err),
        }
    }

    #[cfg(not(no_rc))]
    pub(crate) fn map_err<E2>(self, f: impl FnOnce(E) -> E2) -> BuildErrorKind<T, E2> {
        match self {
            BuildErrorKind::LayoutOverflow(metadata) => BuildErrorKind::LayoutOverflow(metadata),
            BuildErrorKind::AllocError(layout) => BuildErrorKind::AllocError(layout),
            BuildErrorKind::InitError(err) => BuildErrorKind::InitError(f(err)),
        }
    }
}

/// The error type for fallibly creating values on the heap. See [`BuildErrorKind`]
pub struct BuildError<T: ?Sized, E = !, A: Allocator = Global> {
    /// The kind of error encountered when fallibly creating the value.
    pub kind: BuildErrorKind<T, E>,
    /// The allocator in which the value was to be created.
    pub alloc: A,
}

impl<T: ?Sized, E: core::fmt::Debug, A: Allocator + core::fmt::Debug> core::fmt::Debug
    for BuildError<T, E, A>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BuildError").field("kind", &self.kind).field("alloc", &self.alloc).finish()
    }
}

impl<T: ?Sized, E, A: Allocator> BuildError<T, E, A> {
    /// If `self.kind` is [`AllocError(layout)`](BuildErrorKind::AllocError), calls
    /// [`crate::alloc::handle_alloc_error`] with `layout`.
    ///
    /// If `self.kind` is [`LayoutOverflow`](BuildErrorKind::LayoutOverflow),
    /// panics.
    ///
    /// If `self.kind` is [`InitError(err)`](BuildErrorKind::InitError), returns `(err, self.alloc)`.
    #[cold]
    #[cfg(not(no_global_oom_handling))]
    pub fn handle_alloc_error_with_allocator(self) -> (E, A) {
        match self.kind {
            BuildErrorKind::LayoutOverflow(metadata) => {
                panic!("layout for pointee with metadata {metadata:?} cannot be computed")
            }
            BuildErrorKind::AllocError(layout) => crate::alloc::handle_alloc_error(layout),
            BuildErrorKind::InitError(err) => (err, self.alloc),
        }
    }
    /// If `self.kind` is [`AllocError(layout)`](BuildErrorKind::AllocError), calls
    /// [`crate::alloc::handle_alloc_error`] with `layout`.
    ///
    /// If `self.kind` is [`LayoutOverflow`](BuildErrorKind::LayoutOverflow),
    /// panics.
    ///
    /// If `self.kind` is [`InitError(err)`](BuildErrorKind::InitError), returns `err`.
    #[cold]
    #[cfg(not(no_global_oom_handling))]
    pub fn handle_alloc_error(self) -> E {
        match self.kind {
            BuildErrorKind::LayoutOverflow(metadata) => {
                panic!("layout for pointee with metadata {metadata:?} cannot be computed")
            }
            BuildErrorKind::AllocError(layout) => crate::alloc::handle_alloc_error(layout),
            BuildErrorKind::InitError(err) => err,
        }
    }

    #[cfg(not(no_rc))]
    pub(crate) fn map_err<E2>(self, f: impl FnOnce(E) -> E2) -> BuildError<T, E2, A> {
        BuildError { kind: self.kind.map_err(f), alloc: self.alloc }
    }

    #[inline]
    pub(crate) fn layout_overflow(metadata: Metadata<T>, alloc: A) -> Self {
        Self { kind: BuildErrorKind::LayoutOverflow(metadata), alloc }
    }

    #[inline]
    pub(crate) fn alloc_error(layout: Layout, alloc: A) -> Self {
        Self { kind: BuildErrorKind::AllocError(layout), alloc }
    }

    #[inline]
    pub(crate) fn init_error(err: E, alloc: A) -> Self {
        Self { kind: BuildErrorKind::InitError(err), alloc }
    }
}

/// Initialize a place by moving an existing value from a `Box`
unsafe impl<T: ?Sized, A: Allocator, Error> PinInitOnce<T, Error> for Box<T, A> {
    fn metadata(this: &Self) -> Metadata<T> {
        ptr::metadata::<T>(&**this)
    }

    unsafe fn init_once(
        this: Self,
        dst: &mut MaybeUninit<T>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        let size = mem::size_of_val::<T>(&*this);
        let (ptr, alloc) = Box::into_raw_with_allocator(this);
        // Don't drop `T`, but still deallocate the `Box` when we've moved from it
        let this = unsafe { Box::from_raw_in(ptr as *mut MaybeUninit<T>, alloc) };
        unsafe {
            ptr::copy_nonoverlapping(
                Box::as_ptr(&this).cast::<u8>(),
                dst.as_mut_ptr().cast::<u8>(),
                size,
            );
        }
        Ok(())
    }
}
unsafe impl<T: ?Sized, A: Allocator, Error> InitOnce<T, Error> for Box<T, A> {}

/// Initialize a place by cloning an existing value from a `Box`
unsafe impl<T: ?Sized + CloneToUninit, A: Allocator, Error> PinInitMut<T, Error> for Box<T, A> {
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<T>,
        arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <T as PinInit<T, Error>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}
/// Initialize a place by cloning an existing value from a `Box`
unsafe impl<T: ?Sized + CloneToUninit, A: Allocator, Error> PinInit<T, Error> for Box<T, A> {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<T>,
        arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <T as PinInit<T, Error>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: ?Sized + CloneToUninit, A: Allocator, Error> InitMut<T, Error> for Box<T, A> {}
unsafe impl<T: ?Sized + CloneToUninit, A: Allocator, Error> Init<T, Error> for Box<T, A> {}

/// Initialize a slice by moving existing values from a `Vec`
unsafe impl<T, A: Allocator, Error> PinInitOnce<[T], Error> for Vec<T, A> {
    fn metadata(this: &Self) -> Metadata<[T]> {
        ptr::metadata::<[T]>(&**this)
    }

    unsafe fn init_once(
        mut this: Self,
        dst: &mut MaybeUninit<[T]>,
        _arg: (),
        _pre_zeroed: bool,
    ) -> Result<(), Error> {
        let len = this.len();
        unsafe {
            ptr::copy_nonoverlapping(this.as_ptr(), dst.as_mut_ptr().cast::<T>(), len);
            this.set_len(0);
        }
        Ok(())
    }
}
unsafe impl<T, A: Allocator, Error> InitOnce<[T], Error> for Vec<T, A> {}

/// Initialize a slice by cloning existing values from a `Vec`
unsafe impl<T: Clone, A: Allocator, Error> PinInitMut<[T], Error> for Vec<T, A> {
    unsafe fn init_mut(
        this: &mut Self,
        dst: &mut MaybeUninit<[T]>,
        arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <[T] as PinInit<[T], Error>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}

/// Initialize a slice by cloning existing values from a `Vec`
unsafe impl<T: Clone, A: Allocator, Error> PinInit<[T], Error> for Vec<T, A> {
    unsafe fn init_ref(
        this: &Self,
        dst: &mut MaybeUninit<[T]>,
        arg: (),
        pre_zeroed: bool,
    ) -> Result<(), Error> {
        // SAFETY: delegated to caller
        unsafe { <[T] as PinInit<[T], Error>>::init_ref(this, dst, arg, pre_zeroed) }
    }
}
unsafe impl<T: Clone, A: Allocator, Error> InitMut<[T], Error> for Vec<T, A> {}
unsafe impl<T: Clone, A: Allocator, Error> Init<[T], Error> for Vec<T, A> {}

// `Option<&T>` and `Option<Box<T>>` are guaranteed to represent `None` as null
// when `T: Thin`.
// Additionally (though not guaranteed), they represent `None` as null when `T`
// is not `Thin` but the metadata has no niches.
unsafe impl<T: PointeeSized + NoNicheMetadata> NoneIsZero for Box<T> {}

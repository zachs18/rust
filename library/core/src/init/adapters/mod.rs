pub use as_bytes::AsBytes;
pub use by_mut::ByMut;
pub use by_ref::ByRef;
pub use chain::Chain;
pub use from_arg::FromArg;
pub use from_fn::{FnNoArg, FnWithArg, FromFn};
pub use map_arg::MapArg;
pub use repeat::Repeat;
pub use uninit::Uninit;
pub use unsize::Unsize;
pub use with_arg::WithArg;
pub use zeroed::Zeroed;

use crate::init::{ConstLength, RuntimeLength};
use crate::marker::{MetaSized, PointeeSized, Unsize as UnsizeTrait};
use crate::mem::MaybeUninit;
use crate::ptr::{Metadata, Thin, build_metadata};

mod as_bytes;
mod by_mut;
mod by_ref;
mod chain;
mod from_arg;
mod from_fn;
mod map_arg;
mod repeat;
mod uninit;
mod unsize;
mod with_arg;
mod zeroed;

// `Zeroed`

/// Create an initializer that initializes a [`MaybeUninit<T>`] with all bytes zero, for `T: Thin`.
pub const fn zeroed<T: MetaSized + Thin>() -> Zeroed<MaybeUninit<T>> {
    Zeroed::new(build_metadata!(..))
}

/// Create an initializer that initializes a `T` with all bytes zero, for `T: Thin`.
///
/// # Safety
///
/// A `T` consisting of all bytes zero must be valid.
pub const unsafe fn zeroed_unchecked<T: MetaSized + Thin>() -> Zeroed<T> {
    // SAFETY: discharged to caller
    unsafe { Zeroed::new_unchecked(build_metadata!(..)) }
}

/// Create an initializer that initializes a [`MaybeUninit<T>`] with all bytes zero, for `T: Thin`.
pub const fn zeroed_slice<T: MetaSized + Thin>(len: usize) -> Zeroed<[MaybeUninit<T>]> {
    // SAFETY: `[MaybeUninit<T>]` (of any length) is valid with all bytes zero.
    unsafe { Zeroed::new_unchecked(build_metadata!(len, ..)) }
}

/// Create an initializer that initializes a [`[T]`](prim@slice) with all bytes zero, for `T: Thin`.
///
/// # Safety
///
/// A `T` consisting of all bytes zero must be valid.
pub const unsafe fn zeroed_slice_unchecked<T: MetaSized + Thin>(len: usize) -> Zeroed<[T]> {
    // SAFETY: discharged to caller
    unsafe { Zeroed::new_unchecked(build_metadata!(len, ..)) }
}

/// Create an initializer that initializes a [`str`] with all bytes zero.
pub const fn zeroed_str(len: usize) -> Zeroed<str> {
    // SAFETY: `str` (of any length) is valid with all bytes zero.
    unsafe { Zeroed::new_unchecked(build_metadata!(len, ..)) }
}

/// Create an initializer that initializes a [`MaybeUninit<T>`] with all bytes zero, with a given pointer metadata.
pub const fn zeroed_with_metadata<T: MetaSized>(metadata: Metadata<T>) -> Zeroed<MaybeUninit<T>> {
    Zeroed::new(metadata)
}

/// Create an initializer that initializes a `T` with all bytes zero, with a given pointer metadata.
///
/// # Safety
///
/// A `T` consisting of all uninitialized bytes, with pointer metadata `meta` must be valid.
pub const unsafe fn zeroed_with_metadata_unchecked<T: MetaSized>(
    metadata: Metadata<T>,
) -> Zeroed<T> {
    // SAFETY: discharged to caller
    unsafe { Zeroed::new_unchecked(metadata) }
}

// `Uninit`

/// Create an initializer that leaves a [`MaybeUninit<T>`] uninitialized, for `T: Thin`.
pub const fn uninit<T: MetaSized + Thin>() -> Uninit<MaybeUninit<T>> {
    Uninit::new(build_metadata!(..))
}

/// Create an initializer leaves a `T` uninitialized, for `T: Thin`.
///
/// # Safety
///
/// A `T` consisting of all uninitialized bytes must be valid.
pub const unsafe fn uninit_unchecked<T: MetaSized + Thin>() -> Uninit<T> {
    // SAFETY: discharged to caller
    unsafe { Uninit::new_unchecked(build_metadata!(..)) }
}

/// Create an initializer that leaves a slice of [`MaybeUninit<T>`] uninitialized, for `T: Thin`.
pub const fn uninit_slice<T: MetaSized + Thin>(len: usize) -> Uninit<[MaybeUninit<T>]> {
    // SAFETY: `[MaybeUninit<T>]` (of any length) is valid to leave uninitialized.
    unsafe { Uninit::new_unchecked(build_metadata!(len, ..)) }
}

/// Create an initializer that leaves a [`[T]`](prim@slice) uninitialized, for `T: Thin`.
///
/// # Safety
///
/// A `T` consisting of all uninitialized bytes must be valid.
pub const unsafe fn uninit_slice_unchecked<T: MetaSized + Thin>(len: usize) -> Uninit<[T]> {
    // SAFETY: discharged to caller
    unsafe { Uninit::new_unchecked(build_metadata!(len, ..)) }
}

/// Create an initializer that leaves a [`MaybeUninit<T>`] uninitialized, with a given pointer metadata.
pub const fn uninit_with_metadata<T: MetaSized>(metadata: Metadata<T>) -> Uninit<MaybeUninit<T>> {
    Uninit::new(metadata)
}
/// Create an initializer leaves a `T` uninitialized.
///
/// # Safety
///
/// A `T` consisting of all uninitialized bytes, with pointer metadata `meta` must be valid.
pub const unsafe fn uninit_with_metadata_unchecked<T: MetaSized>(
    metadata: Metadata<T>,
) -> Uninit<T> {
    // SAFETY: discharged to caller
    unsafe { Uninit::new_unchecked(metadata) }
}

// `Chain`

/// Create an initializer that initializes a slice in two parts.
pub const fn chain<I1, I2>(init1: I1, init2: I2) -> Chain<I1, I2> {
    Chain::new(init1, init2)
}

// `AsBytes`

/// Create an initializer that initializes a `[u8]` with a `str`.
pub const fn as_bytes<I>(init: I) -> AsBytes<I> {
    AsBytes::new(init)
}

// `Repeat`

/// Create an initializer that initializes a `[T]` by repeating an element initializer.
pub const fn repeat_slice<I, ElemArg>(elem: I, len: usize) -> Repeat<I, RuntimeLength, ElemArg> {
    Repeat::new_slice(len, elem)
}

/// Create an initializer that initializes a `[T; N]` by repeating an element initializer.
pub const fn repeat_array<const N: usize, I, ElemArg>(
    elem: I,
) -> Repeat<I, ConstLength<N>, ElemArg> {
    Repeat::new_array::<N>(elem)
}

// `FromFn`

/// Create an initializer that initializes a place by calling a function to produce an initializer just-in-time.
pub const fn from_fn<T, F>(func: F) -> FromFn<T, F, FnNoArg> {
    FromFn::new(func)
}

/// Create an initializer that initializes a place by calling a function to produce an initializer just-in-time.
pub const fn from_fn_with_arg<T, F>(func: F) -> FromFn<T, F, FnWithArg> {
    FromFn::new_with_arg(func)
}

// `FromArg`

/// Create an initializer that initializes a place by using the initializer argument as an initializer.
pub const fn from_arg<T: MetaSized + Thin>() -> FromArg<T> {
    FromArg::new()
}

// `WithArg`

/// Create an initializer that initializes a place with an initializer and a pre-provided argument.
pub const fn with_arg<T: MetaSized, I, Arg>(init: I, arg: Arg) -> WithArg<T, I, Arg> {
    WithArg::new(init, arg)
}

// `MapArg`

/// Create an initializer that initializes a place with an initializer by passing it a computed argument.
pub const fn map_arg<T: MetaSized, I, F>(init: I, f: F) -> MapArg<T, I, F> {
    MapArg::new(init, f)
}

// `Unsize`

/// Create an initializer that initializes an unsized place with an initializer for a value that
/// can be unsized-coerced into the type of that place.
pub const fn unsize<U: MetaSized, T: MetaSized + UnsizeTrait<U>, I>(init: I) -> Unsize<U, T, I> {
    Unsize::new(init)
}

// `ByRef`/`ByMut`

/// Create an initializer that wraps a reference to an initializer that can be reused.
pub const fn by_ref<T: MetaSized, I: PointeeSized>(init: &I) -> &ByRef<T, I> {
    ByRef::new(init)
}

/// Create an initializer that wraps a mutable reference to an initializer that can be reused.
pub const fn by_mut<T: MetaSized, I: PointeeSized>(init: &mut I) -> &mut ByMut<T, I> {
    ByMut::new(init)
}

#![unstable(feature = "ptr_metadata", issue = "81513")]

use crate::clone::TrivialClone;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::intrinsics::{aggregate_raw_ptr, ptr_metadata};
use crate::marker::{MetaSized, PointeeSized};
use crate::ptr::NonNull;

/// The type for pointer metadata in pointers and references to `T`.
///
/// `Metadata<T>` implements [`Copy`], [`Ord`], [`Hash`], [`Debug`](core::fmt::Debug), [`Send`],
/// [`Sync`], [`Unpin`], and [`Freeze`](core::marker::Freeze) for all `T`.
///
/// # Pointer metadata
///
/// Raw pointer types and reference types in Rust can be thought of as made of two parts:
/// a data pointer that contains the memory address of the value, and some metadata.
///
/// For statically-sized types (that implement the `Sized` traits)
/// as well as for `extern` types,
/// pointers are said to be “thin”: metadata is zero-sized.
///
/// Pointers to [dynamically-sized types][dst] are said to be “wide” or “fat”,
/// they have non-zero-sized metadata.
///
/// [dst]: https://doc.rust-lang.org/nomicon/exotic-sizes.html#dynamically-sized-types-dsts
///
/// # Fields
///
/// `Metadata<T>` has different fields depending on `T`:
///
/// * For primitive types which are always `Thin` (integers, `!`, `fn` pointers, `extern type`s, etc), `Metadata<T>` has no fields.
/// * For the `str` type, `Metadata<str>` has one field `len: usize`.
/// * For slice types `[T]`, `Metadata<[T]>` has two fields `len: usize` and `elem: Metadata<T>`.
/// * For array types `[T; N]`, `Metadata<[T]>` has one `elem: Metadata<T>`.
/// * For trait objects like `dyn SomeTrait`, `Metadata<dyn SomeTrait>` has one field of type [`DynMetadata<dyn SomeTrait>`][DynMetadata]
/// * For tuples, for each field `U` of the tuple, `Metadata<T>` has a corresponding field of type `Metadata<U>`.
/// * For structs and unions, for each field `f: FieldTy` of the struct/union, `Metadata<T>` has a corresponding field `f: Metadata<FieldTy>`.
/// * For enums, `Metadata<T>` has no fields.
/// * For `unsized type`s, `Metadata<T>` has the fields listed in the `unsized type` declaration.
///
/// In the future, the Rust language may gain new kinds of types
/// that have different pointer metadata.
///
/// # Usage
///
/// Raw pointers can be decomposed into the data pointer and metadata components
/// with their [`to_raw_parts`] method.
///
/// Alternatively, metadata alone can be extracted with the [`metadata`] function.
/// A reference can be passed to [`metadata`] and implicitly coerced.
///
/// A (possibly-wide) pointer can be put back together from its data pointer and metadata
/// with [`from_raw_parts`] or [`from_raw_parts_mut`].
///
/// [`to_raw_parts`]: *const::to_raw_parts
///
/// Metadata can be constructed from scratch using the [`build_metadata!`] macro by providing a value for each of `Metadata<T>`'s fields.
///
/// # "Safe" metadata
///
/// Raw pointers may have any metadata value that can be constructed with [`build_metadata!`].
///
/// Any metadata value which is used as the metadata for a reference must be "safe".
/// For types which implement [`MetaSized`], any "safe"
/// metadata value must have a valid size and alignment (or only alignment).
//FIXME(ptr_metadata_v2): make these docs better.
///
/// For types with builtin [`MetaSized`] impls, a metadata value is "safe" if and only if
/// it's computed layout (computed with infinite precision) has a size `<= isize::MAX`.
///
/// For `unsized type`s with custom [`MetaSized`] impls, the author of the `unsized type` should define what
/// a "safe" metadata value is for their type. Note that if an `unsized type T` implements `Thin`, then the single value of `Metadata<T>` *must* be safe.
///
/// # FIXME: put this somewhere
///
/// For `unsized type`s, privacy can prevent users from constructing arbitrary metadata values,
/// so `unsized type` authors can make safety assumptions even about non-"safe" metadata values.
pub type Metadata<T> = builtin!(ptr_metadata(T));

/// Macro to construct a [`Metadata<T>`].
///
/// The pointee type can be optionally specified with `for $ty;`, but if type inference can deduce
/// the type this may not be necessary.
///
/// This is followed by a a list of fields `$fieldname: $valueexpr`, each terminated by a comma.
///
/// This is followed by either `..` or `.. $baseexpr`. Similar to struct expressions, this
/// indicates that remaining fields are filled with their default values (for `..`), or values
/// taken from `$baseexpr`. For [`Metadata<T>`], "default values" means that any fields which are
/// [`Metadata<impl Thin>`] are filled in with their single possible value.
///
/// Either `..` or `..base` is required in all cases, so that more fields can be added to [`Metadata<T>`] for existing `T`s
/// without breaking existing code.
//FIXME(ptr_metadata_v2): actually change the parser to require that?
///
/// See the docs for [`Metadata`] for what fields are available for a given `T`.
///
/// # Usage
///
/// ```rust
/// #![feature(ptr_metadata)]
/// #![feature(ptr_metadata_v2)]
/// #![feature(more_unsized)]
/// # struct T;
/// # use core::ptr::{Metadata, build_metadata};
/// let _ = build_metadata!(for T; ..); // for `T: Thin`
/// let _: Metadata<T> = build_metadata!(..); // If type inference can deduce the pointee type, it does not need to be specified
///
/// let slice_42: Metadata<[u8]> = build_metadata!(len: 42, ..); // For most types, if the field is `Metadata<impl Thin>`, it can be omitted
/// let slice_37_of_slice_42: Metadata<[[u8]]> = build_metadata!(len: 37, elem: slice_42, ..);
/// let slice_25_of_slice_42: Metadata<[[u8]]> = build_metadata!(len: 25, ..slice_37_of_slice_42); // Takes `elem` from `slice_37_of_slice_42`.
/// ```
#[unstable(feature = "ptr_metadata_v2", issue = "none")]
#[allow_internal_unstable(builtin_syntax)]
pub macro build_metadata {
    ($($field:ident $(: $value:expr)?,)* ..$($base:expr)?) => { builtin # ptr_metadata($($field $(: $value)?,)* .. $($base)?) },
    (for $pointee:ty; $($field:ident $(: $value:expr)?,)* ..$($base:expr)?) => { builtin # ptr_metadata(for $pointee; $($field $(: $value)?,)* .. $($base)?) },
}

/// Pointers to types implementing this trait are “thin”.
///
/// This includes statically-`Sized` types and `extern` types.
///
/// # Example
///
/// ```rust
/// #![feature(ptr_metadata)]
///
/// fn this_never_panics<T: std::ptr::Thin>() {
///     assert_eq!(size_of::<&T>(), size_of::<usize>())
/// }
/// ```
///
/// # Safety
///
/// Implementations of `Thin` are provided by the compiler for most types.
///
/// Only `unsized type`s may have a manual implementation of `Thin`. If an `unsized type`
/// implements both `Thin` and `MetaSized`, then the single possible value
/// of [`Metadata<Self>`] must be "safe" as described in `Metadata`'s documentation.
#[unstable(feature = "ptr_metadata", issue = "81513")]
#[fundamental]
#[rustc_specialization_trait]
#[rustc_dyn_incompatible_trait]
// `Thin` being coinductive is okay for the same reasons as
// `Sized`.
#[rustc_coinductive]
// `Thin` can only be manually implemented for `unsized type`s.
#[lang = "thin_pointee_trait"]
pub unsafe trait Thin: PointeeSized {}

/// Extracts the metadata component of a pointer.
///
/// Values of type `*mut T`, `&T`, or `&mut T` can be passed directly to this function
/// as they implicitly coerce to `*const T`.
///
/// # Example
///
/// ```
/// #![feature(ptr_metadata)]
///
/// assert_eq!(std::ptr::metadata("foo").len, 3_usize);
/// ```
#[inline]
pub const fn metadata<T: PointeeSized>(ptr: *const T) -> Metadata<T> {
    ptr_metadata(ptr)
}

/// Forms a (possibly-wide) raw pointer from a data pointer and metadata.
///
/// This function is safe but the returned pointer is not necessarily safe to dereference.
/// For slices, see the documentation of [`slice::from_raw_parts`] for safety requirements.
/// For trait objects, the metadata must come from a pointer to the same underlying erased type.
///
/// If you are attempting to deconstruct a DST in a generic context to be reconstructed later,
/// a thin pointer can always be obtained by casting `*const T` to `*const ()`.
///
/// [`slice::from_raw_parts`]: crate::slice::from_raw_parts
#[unstable(feature = "ptr_metadata", issue = "81513")]
#[inline]
pub const fn from_raw_parts<T: PointeeSized>(
    data_pointer: *const impl Thin,
    metadata: Metadata<T>,
) -> *const T {
    aggregate_raw_ptr(data_pointer, metadata)
}

/// Performs the same functionality as [`from_raw_parts`], except that a
/// raw `*mut` pointer is returned, as opposed to a raw `*const` pointer.
///
/// See the documentation of [`from_raw_parts`] for more details.
#[unstable(feature = "ptr_metadata", issue = "81513")]
#[inline]
pub const fn from_raw_parts_mut<T: PointeeSized>(
    data_pointer: *mut impl Thin,
    metadata: Metadata<T>,
) -> *mut T {
    aggregate_raw_ptr(data_pointer, metadata)
}

/// The metadata for a `Dyn = dyn SomeTrait` trait object type.
///
/// It is a pointer to a vtable (virtual call table)
/// that represents all the necessary information
/// to manipulate the concrete type stored inside a trait object.
/// The vtable notably contains:
///
/// * type size
/// * type alignment
/// * a pointer to the type’s `drop_in_place` impl (may be a no-op for plain-old-data)
/// * pointers to all the methods for the type’s implementation of the trait
///
/// Note that the first three are special because they’re necessary to allocate, drop,
/// and deallocate any trait object.
///
/// It is possible to name this struct with a type parameter that is not a `dyn` trait object
/// (for example `DynMetadata<u64>`) but not to obtain a meaningful value of that struct.
///
/// Note that while this type implements `PartialEq`, comparing vtable pointers is unreliable:
/// pointers to vtables of the same type for the same trait can compare inequal (because vtables are
/// duplicated in multiple codegen units), and pointers to vtables of *different* types/traits can
/// compare equal (since identical vtables can be deduplicated within a codegen unit).
#[lang = "dyn_metadata"]
pub struct DynMetadata<Dyn: PointeeSized> {
    _vtable_ptr: NonNull<VTable>,
    _phantom: crate::marker::PhantomData<Dyn>,
}

unsafe extern "C" {
    /// Opaque type for accessing vtables.
    ///
    /// Private implementation detail of `DynMetadata::size_of` etc.
    /// There is conceptually not actually any Abstract Machine memory behind this pointer.
    type VTable;
}

impl<Dyn: PointeeSized> DynMetadata<Dyn> {
    /// When `DynMetadata` appears as the metadata field of a wide pointer, the rustc_middle layout
    /// computation does magic and the resulting layout is *not* a `FieldsShape::Aggregate`, instead
    /// it is a `FieldsShape::Primitive`. This means that the same type can have different layout
    /// depending on whether it appears as the metadata field of a wide pointer or as a stand-alone
    /// type, which understandably confuses codegen and leads to ICEs when trying to project to a
    /// field of `DynMetadata`. To work around that issue, we use `transmute` instead of using a
    /// field projection.
    #[inline]
    fn vtable_ptr(self) -> *const VTable {
        // SAFETY: this layout assumption is hard-coded into the compiler.
        // If it's somehow not a size match, the transmute will error.
        unsafe { crate::mem::transmute::<Self, *const VTable>(self) }
    }

    /// Returns the size of the type associated with this vtable.
    #[inline]
    pub fn size_of(self) -> usize {
        // Note that "size stored in vtable" is *not* the same as "result of size_of_val_raw".
        // Consider a reference like `&(i32, dyn Send)`: the vtable will only store the size of the
        // `Send` part!
        // SAFETY: DynMetadata always contains a valid vtable pointer
        unsafe { crate::intrinsics::vtable_size(self.vtable_ptr() as *const ()) }
    }

    /// Returns the alignment of the type associated with this vtable.
    #[inline]
    pub fn align_of(self) -> usize {
        // SAFETY: DynMetadata always contains a valid vtable pointer
        unsafe { crate::intrinsics::vtable_align(self.vtable_ptr() as *const ()) }
    }

    /// Returns the size and alignment together as a `Layout`
    #[inline]
    pub fn layout(self) -> crate::alloc::Layout {
        // SAFETY: the compiler emitted this vtable for a concrete Rust type which
        // is known to have a valid layout. Same rationale as in `Layout::for_value`.
        unsafe { crate::alloc::Layout::from_size_align_unchecked(self.size_of(), self.align_of()) }
    }
}

unsafe impl<Dyn: PointeeSized> Send for DynMetadata<Dyn> {}
unsafe impl<Dyn: PointeeSized> Sync for DynMetadata<Dyn> {}

impl<Dyn: PointeeSized> fmt::Debug for DynMetadata<Dyn> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DynMetadata").field(&self.vtable_ptr()).finish()
    }
}

// Manual impls needed to avoid `Dyn: $Trait` bounds.

impl<Dyn: PointeeSized> Unpin for DynMetadata<Dyn> {}

impl<Dyn: PointeeSized> Copy for DynMetadata<Dyn> {}

impl<Dyn: PointeeSized> Clone for DynMetadata<Dyn> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

#[doc(hidden)]
unsafe impl<Dyn: PointeeSized> TrivialClone for DynMetadata<Dyn> {}

impl<Dyn: PointeeSized> Eq for DynMetadata<Dyn> {}

impl<Dyn: PointeeSized> PartialEq for DynMetadata<Dyn> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        crate::ptr::eq::<VTable>(self.vtable_ptr(), other.vtable_ptr())
    }
}

impl<Dyn: PointeeSized> Ord for DynMetadata<Dyn> {
    #[inline]
    #[allow(ambiguous_wide_pointer_comparisons)]
    fn cmp(&self, other: &Self) -> crate::cmp::Ordering {
        <*const VTable>::cmp(&self.vtable_ptr(), &other.vtable_ptr())
    }
}

impl<Dyn: PointeeSized> PartialOrd for DynMetadata<Dyn> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<crate::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Dyn: PointeeSized> Hash for DynMetadata<Dyn> {
    #[inline]
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        crate::ptr::hash::<VTable, _>(self.vtable_ptr(), hasher)
    }
}

impl<T: MetaSized> Metadata<T> {
    /// Returns the size of the pointee associated with this metadata, or `None`
    /// if the layout calculation would overflow.
    #[unstable(feature = "ptr_metadata_v2", issue = "none")]
    //#[unstable(feature = "checked_layout_for_meta", issue = "none")]
    pub const fn checked_size_of(self) -> Option<usize> {
        crate::mem::checked_size_for_meta(self)
    }

    /// Returns the alignment of the pointee associated with this metadata, or `None`
    /// if the layout calculation would overflow.
    #[unstable(feature = "ptr_metadata_v2", issue = "none")]
    //#[unstable(feature = "checked_layout_for_meta", issue = "none")]
    pub const fn checked_align_of(self) -> Option<usize> {
        crate::mem::checked_align_for_meta(self)
    }

    /// Returns the layout of the pointee associated with this metadata, or `None`
    /// if the layout calculation would overflow.
    #[unstable(feature = "ptr_metadata_v2", issue = "none")]
    //#[unstable(feature = "checked_layout_for_meta", issue = "none")]
    pub const fn checked_layout(self) -> Option<crate::alloc::Layout> {
        let Some(size) = crate::mem::checked_size_for_meta(self) else { return None };
        let align = crate::mem::checked_align_for_meta(self).unwrap();
        // SAFETY: size and align came from size/align intrinsics for the same metadata
        Some(unsafe { crate::alloc::Layout::from_size_align_unchecked(size, align) })
    }
}

impl<T: PointeeSized> PartialEq for Metadata<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Self::cmp(self, other).is_eq()
    }
}

impl<T: PointeeSized> Eq for Metadata<T> {}

impl<T: PointeeSized> PartialOrd for Metadata<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<crate::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Ord for Metadata<T> is a builtin impl, because it cannot be written
// fully generically in the surface language.

impl<T: PointeeSized + Thin> Default for Metadata<T> {
    fn default() -> Self {
        builtin!(ptr_metadata(..))
    }
}

// Convenience impls to avoid needing to constantly use `build_metadata!`
/// Create a `Metadata<[T]>` with a given length.
#[unstable(feature = "ptr_metadata_v2", issue = "none")]
#[rustc_const_unstable(feature = "ptr_metadata_v2", issue = "none")]
impl<T: MetaSized + Thin> const From<usize> for Metadata<[T]> {
    fn from(len: usize) -> Self {
        build_metadata!(for [T]; len, ..)
    }
}

// Convenience impls to avoid needing to constantly use `build_metadata!`
/// Create a `Metadata<str>` with a given length.
#[unstable(feature = "ptr_metadata_v2", issue = "none")]
#[rustc_const_unstable(feature = "ptr_metadata_v2", issue = "none")]
impl const From<usize> for Metadata<str> {
    fn from(len: usize) -> Self {
        build_metadata!(for str; len, ..)
    }
}

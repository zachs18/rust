# `ptr_metadata_v2`

`core::ptr::Metadata<T>` is a new "magic" struct-like type that represents the metadata of a pointer to `T`

## Primitive Thin Pointees

For all Tintegers, `char`, `bool`, floats, never, pointers, references, `fn` pointers, `fn` items, closures,`extern type`s, or `Metadata` itself, `Metadata<T>` is a fieldless 1-ZST, like `struct PrimitiveMetadata {};`. These (like the metadata of all `Thin` types) are "trivial".

## Arrays and Slices
* For `str`: `struct StrMetadata { pub length: usize }`
* For slices `[T]`: `struct SliceMetadata<T: ?Sized> { pub length: usize, pub elem: Metadata<T> }`
* For arrays `[T; N]`: `struct ArrayMetadata<T: ?Sized> { pub elem: Metadata<T> }`. If `Metadata<T>` is trivial, then so is `Metadata<[T; N]>`.

## Trait objects

For `T = dyn Trait`: `struct DynTraitMetadata { pub vtable: std::ptr::DynMetadata<dyn Trait> }`


## Structs and Unions

When `T` is a `struct` or `union`, `Metadata<T>` has fields with the same names and visibilities and `T` does, whose types are `Metadata<F>` where `F` is the corresponding field's type. If `T` is `#[non_exhaustive]`, then so is `Metadata<T>` (TODO: flesh out `#[non_exhaustive]` interaction with default field values).

For example:

```Rust
pub struct Foo<T: ?Sized> {
	x: u32,
	pub(crate) y: u32,
	pub z: T
}
// expository
struct FooMetadata<T> {
	x: Metadata<u32>,
	pub(crate) y: Metadata<u32>,
	pub z: Metadata<T>,
}
```


## Tuples

Tuples are essentially the same as tuple-structs with all public fields.

For example:

```Rust
type Tuple = (u32, u32, [u8]);
// expository
struct TupleMetadata(pub Metadata<u32>, pub Metadata<u32>, pub Metadata<[u8]>);
```

## Enums

todo

```Rust  
enum Foo {
	A { x: u32, z: [u8] },
	B(dyn Trait),
	C
}
struct FooMetadata {
	A.x: Metadata<u32>,
	A.z: Metadata<[u8]>,
	B.0: Metadata<dyn Trait>,
}
```

## Alternative Idea to `BikeshedCustomMetadata`: `unsized type Foo;`

`unsized type Foo;` defined a new, local, nominal type `Foo`. The user must implement `std::ptr::BikeshedUnsizedTypeSemantics`, which is a trait that cannt be used in bounds. Like `Drop`, the bounds on its impls must be the same as on the type definition.

```Rust
pub unsafe trait BikeshedUnsizedTypeSemantics {
	type Metadata: Sized + Copy + Freeze + etc;
	/// This should return None if this type's layout cannot be determined statically,
	/// or if  TODO: other semantics
	const fn layout_for_meta(meta: Self::Metadata) -> Option<Layout>;
}
```

```Rust
unsized type SlicePair<T>;

unsafe impl 
```

TODO: how to specify variance of generic parameters?  


### Idea b2:

`unsized struct Foo` defines a new custom unsized struct with a given sized prefix. The user must implement `std::ptr::BikeshedUnsizedTypeSemantics`, which is a trait that cannt be used in bounds. Like `Drop`, the bounds on its impls must be the same as on the type definition. The sized prefix cannot have a field named `custom_metadata`.

```Rust
pub unsafe trait BikeshedUnsizedTypeSemantics {
	type Metadata: Sized + Copy + Freeze + etc;
	/// This should return None if this type's layout cannot be determined statically from the metadata, either from this particular metadata value (e.g. slice byte-length overflow), or in general (e.g. `CStr` or length-prefixed arrays).
	const fn tail_layout_for_meta(meta: Self::Metadata) -> Option<Layout>;
	const fn tail_layout_for_pointee(&self) -> Option<Layout> {
		Self::tail_layout_for_meta(ptr::metadata(self))
	}
}

#[repr(C)]
unsized struct Foo<T> {
	sized_prefix_fields: u32,
	variance_determined_by_these: PhantomData<T>,
}

unsafe impl<T> BikeshedUnsizedTypeSemantics for Foo<T> {
	// The `custom_metadata` field in `Metadata` is this type.
	// The other fields of `Metadata<Foo<_>>` are like normal structs for the sized prefix.
	type Metadata = todo;
}

```

Note that as far as the compiler is concerned, the whole tail is wrapped in `UnsafeCell`, so "overlapping" types are fine (as long as their sized prefix is fine etc).

```Rust
#[repr(C)]
unsized struct RingSlice<T>(PhantomData<[T]>);

struct RingSliceMetadata {
	extent: usize,
	/// `start < extent`
	start: usize,
	/// `length <= extent`
	length: usize,
}

unsafe impl<T> BikeshedUnsizedTypeSemantics for Foo<T> {
	// The `custom_metadata` field in `Metadata` is this type.
	// The other fields of `Metadata<Foo<_>>` are like normal structs for the sized prefix.
	type Metadata = RingSliceMetadata;
	
	const fn tail_layout_for_meta(meta: Self::Metadata) -> Option<Layout> {
		// The sized prefix is a 1-ZST, so the return value
		// is the same as the layout of the whole type, not just the tail.
		// A RingSlice is a slice of an array-based ring buffer; the layout
		// is that of the underlying array, of length `extent`.
		Layout::array::<T>(meta.extent).ok()
	}
}

impl<T> RingSlice<T> {
	fn get(&self, idx: usize) -> Option<&T> {
		if idx >= self.length {
			return None;
		}
		let ptr: *const Self = self;
		if 
		ptr.cast::<T>()
	}
}
```


## Idea c: `SingleMetadata`

Types whose metadata is "simple" or, like, a single field, are special and support pointer casts between other types with the same simple metadata (required for backcompat, since `*const [u8] as *const WithTail<[u8]>` and `*const [T] as *const str` etc are allowed).

TODO: flesh this out

## Trivial metadata

When constructing `Metadata`, fields may be omitted if they are "trivial", with the same semantics as if they were given a default value under [RFC #3681](https://github.com/rust-lang/rfcs/blob/master/text/3681-default-field-values.md).

When `as`-casting pointers, any pointer can be `as`-casted to a pointer whose pointee has trivial metadata.

Name bikeshedding: same as existing `Thin`.

The following types have trivial metadata:


* Primitive thin pointees
* Arrays whose element type has trivial metadata
* `struct`s, tuples, `union`s, and `enum`s all of whose fields have trivial metadata
* (maybe?) `BikeshedCustomMetadata<()>`


## Simple metadata

When casting pointers using `as` casts, pointers with simple metadata may be casted between if they have the same bikeshed-underlying metadata type.

The following types have simple metadata (and their underlying metadata type):

* (maybe?) Any type with trivial metadata (the underlying metadata is `()`)
* String slices (the underlying metadata is the `usize` length)
* Arrays whose element type has simple metadata (the underlying metadata is the element's underlying metadata)
* Slices whose element type has *trivial* metadata (the underlying metadata is the `usize` length)
* `struct`s, tuples, `union`s, and `enum`s where all but one field has trivial metadata, and the remaining field has simple metadata (the underlying metadata is that field's underlying metadata) (TODO: maybe restrict to last field for ease of implementation, and say that non-last fields having non-trivial metadata makes the aggregate have complex metadata)
* `dyn Trait` (the underlying metadata is the `DynMetadata<dyn Trait>` vtable pointer)
* (maybe?) `BikeshedCustomMetadata<M>` (the underlying metadata is `M`)

## Complex Metadata

All types that do not have simple metadata have complex metadata.

When `as`-casting pointers, pointees with complex metadata cannot be casted between (identity "casts" are still allowed). Note that coercions (e.g. unsizing) that can happen are still allowed to use `as` keyword as normal (e.g. `*const Struct<u8, [u32]> as *const Struct<dyn Debug, [u32]>` is allowed as an unsising coercion, not a "cast" per se).

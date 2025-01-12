# `ptr_metadata_v2`

`core::ptr::Metadata<T>` is a new "magic" struct-like type that represents the metadata of a pointer to `T`

### `#[repr(transparent_if_possible)]`

This is not a proposal to add this `repr` stably, it is just used as an expository tool. Informally, it is intended to mean that a particular instantiation of the annotated generic struct is laid out the same as a non-generic `#[repr(transparent)]` struct, if such a struct would compile, and otherwise as a `#[repr(Rust)]` struct.


## Primitive Thin Pointees

For integers, `char`, `bool`, floats, never, pointers, references, `fn` pointers, `fn` items, closures,`extern type`s, or `Metadata` itself, `Metadata<T>` is a fieldless 1-ZST, like `struct PrimitiveMetadata {};`. These (like the metadata of all `Thin` types) are "trivial".

## Arrays and Slices
* For `str`: `#[repr(transparent)] struct StrMetadata { pub length: usize }`
* For slices `[T]`: `#[repr(transparent_if_possible)] struct SliceMetadata<T> { pub length: usize, pub elem: Metadata<T> }`
* For arrays `[T; N]`: `#[repr(transparent)] struct ArrayMetadata<T> { pub elem: Metadata<T> }`. If `Metadata<T>` is trivial, then so is `Metadata<[T; N]>`.

## Trait objects

For `T = dyn Trait`: `#[repr(transparent)] struct DynTraitMetadata { pub vtable: std::ptr::DynMetadata<dyn Trait> }`


## Structs and Unions

When `T` is a `struct` or `union`, `Metadata<T>` has fields with the same names and visibilities and `T` does, whose types are `Metadata<F>` where `F` is the corresponding field's type. If `T` is `#[non_exhaustive]`, then so is `Metadata<T>`.

For example:

```Rust
pub struct Foo<T: ?Sized> {
	x: u32,
	pub(crate) y: u32,
	pub z: T
}
// expository
#[repr(transparent)]
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
#[repr(transparent)]
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

## BikeshedCustomMetadata

`core::ptr::BikeshedCustomMetadata<M>` is a "magic" type such that `Metadata<BikeshedCustomMetadata>>` is `#[repr(transparent)] struct { pub metadata: M }`.

TODO: variance (probably invariant or covariant)  

### Idea 1: 

`BikeshedCustomMetadata` is `!Sized`, but always has a dynamic size of 0 and align of 1. 

Not great, since it's not actually that useful for custom DSTs then; `Box`, `size_of_val`, etc still won't work right.

### Idea 2:

`BikeshedCustomMetadata` is magic, in that having as a field forces an ADT to manually implement `MetaSized`. Structs containing TODO

### Idea 3:

`BikeshedCustomMetadata<M>` is whatever `extern type`s end up being, but has ptr metadata `#[repr(transparent)] struct Metadata { pub metadata: M }` instead of `#[repr(transparent)] struct Metadata;`

## Alternative Idea to `BikeshedCustomMetadata`: `unsized type Foo;`

`unsized type Foo;` defined a new, local, nominal type `Foo`. The user must implement `std::ptr::BikeshedUnsizedTypeSemantics`, which is a trait that cannt be used in bounds. Like `Drop`, the bounds on its impls must be the same as on the type definition.

```Rust
pub unsafe trait BikeshedUnsizedTypeSemantics {
	type Metadata: Sized + Copy + Freeze + etc;
	/// This should return None if this type's layout cannot be determined statically,
	/// or if 
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

When constructing `Metadata`, fields may be omitted if they are "trivial".
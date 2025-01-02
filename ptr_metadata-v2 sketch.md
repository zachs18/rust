# `ptr_metadata_v2`

`core::ptr::Metadata<T>` is a new "magic" struct-like type that represents the metadata of a pointer to `T`

### `#[repr(transparent_if_possible)]`

This is not a proposal to add this `repr` stably, it is just used as an expository tool. Informally, it is intended to mean that a particular instantiation of the annotated generic struct is laid out the same as a non-generic `#[repr(transparent)]` struct, if such a struct would compile, and otherwise as a `#[repr(Rust)]` struct.


## Primitive `Sized` Pointees

For integers, `char`, `bool`, floats, never, pointers, references, `fn` pointers, `fn` items, closures, or `Metadata` itself, `Metadata<T>` is a fieldless 1-ZST, like `struct PrimitiveMetadata {};`. These (like the metadata of all `Thin` types) are "trivial".

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

`BikeshedCustomMetadata` is magic, in that having as a field forces an ADT to manually implement `MetaSized`. Structs containing

## Trivial metadata

When constructing `Metadata`, fields may be omitted if they are "trivial".
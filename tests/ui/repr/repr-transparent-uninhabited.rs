//@ check-pass
#![feature(never_type)]

#[derive(Clone, Copy)]
enum EmptyEnum {}

union InhabitedUnion {
    pub unit: (),
    pub never: EmptyEnum,
}

union MaybeUninhabitedUnion {
    pub never1: EmptyEnum,
    pub never2: !,
}

enum InhabitedEnum {
    A,
    B(EmptyEnum),
    C(EmptyEnum, [u8; 0]),
    D(EmptyEnum, EmptyEnum),
}

enum UninhabitedOneVariantEnum {
    A(EmptyEnum),
}

enum UninhabitedMultiVariantEnum {
    A(EmptyEnum),
    B(EmptyEnum, [u8; 0]),
    C(EmptyEnum, EmptyEnum),
}

// Errors involving EmptyEnum should directly reference it

#[repr(transparent)]
struct EmptyEnum1ZstDisallowed1(u32, EmptyEnum);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `EmptyEnum`, which is uninhabited and affects its ABI.
//~| NOTE: `#[warn(repr_transparent_uninhabited_fields)]` on by default

#[repr(transparent)]
struct EmptyEnum1ZstDisallowed2(EmptyEnum, EmptyEnum);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `EmptyEnum`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct EmptyEnum1ZstValidAsWrapped([u8; 0], EmptyEnum);

// Errors involving `UninhabitedOneVariantEnum` should refer to its field,
// since it has only one variant.

#[repr(transparent)]
struct UninhabitedOneVariantEnum1ZstDisallowed1(u32, UninhabitedOneVariantEnum);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `EmptyEnum`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct UninhabitedOneVariantEnum1ZstDisallowed2(
    UninhabitedOneVariantEnum,
    UninhabitedOneVariantEnum,
    //~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
    //~| WARN: this was previously accepted
    //~| NOTE: for more information
    //~| NOTE: this struct contains `EmptyEnum`, which is uninhabited and affects its ABI.
);

#[repr(transparent)]
struct UninhabitedOneVariantEnum1ZstValidAsWrapped([u8; 0], UninhabitedOneVariantEnum);

// Errors involving `UninhabitedMultiVariantEnum` should refer to itself,
// since it has multiple variants so no single variant is responsible for its uninhabitedness.

#[repr(transparent)]
struct UninhabitedMultiVariantEnum1ZstDisallowed1(u32, UninhabitedMultiVariantEnum);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `UninhabitedMultiVariantEnum`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct UninhabitedMultiVariantEnum1ZstDisallowed2(
    UninhabitedMultiVariantEnum,
    UninhabitedMultiVariantEnum,
    //~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
    //~| WARN: this was previously accepted
    //~| NOTE: for more information
    //~| NOTE: this struct contains `UninhabitedMultiVariantEnum`, which is uninhabited and affects its ABI.
);

#[repr(transparent)]
struct UninhabitedMultiVariantEnum1ZstValidAsWrapped([u8; 0], UninhabitedMultiVariantEnum);

// Errors involving the never type should refer to it.

#[repr(transparent)]
struct NeverDisallowed1(u32, !);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `!`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct NeverDisallowed2(!, !);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `!`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct NeverValidAsWrapped([u8; 0], !);

// Zero-length arrays of uninhabited 1-ZSTs should be allowed,
// as they are inhabited.

#[repr(transparent)]
struct ArrayZeroValid1(u32, [EmptyEnum; 0]);

#[repr(transparent)]
struct ArrayZeroValid2([EmptyEnum; 0], [EmptyEnum; 0]);

// A 1-ZST enum with one inhabited variant can have other uninhabited variants
// and still be valid.

#[repr(transparent)]
struct OneInhabitedVariantValid(u32, InhabitedEnum);

// A union with an inhabited field is definitely inhabited.

#[repr(transparent)]
struct InhabitedUnion1ZstValid1(u32, InhabitedUnion);

#[repr(transparent)]
struct InhabitedUnion1ZstValid2(InhabitedUnion, InhabitedUnion);

// A union without any inhabited fields *might* be considered uninhabited in the future.
// Currently all unions are treated as inhabited, so this is just future-proofing.

#[repr(transparent)]
struct MaybeUninhabitedUnionUnion1ZstDisallowed1(u32, MaybeUninhabitedUnion);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `MaybeUninhabitedUnion`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct MaybeUninhabitedUnionUnion1ZstDisallowed2(MaybeUninhabitedUnion, MaybeUninhabitedUnion);
//~^ WARN: zero-sized fields in `repr(transparent)` cannot be uninhabited
//~| WARN: this was previously accepted
//~| NOTE: for more information
//~| NOTE: this struct contains `MaybeUninhabitedUnion`, which is uninhabited and affects its ABI.

#[repr(transparent)]
struct MaybeUninhabitedUnionUnion1ZstValidAsWrapped([u8; 0], MaybeUninhabitedUnion);

fn main() {}

//@compile-flags: -Zmiri-disable-validation
//@ normalize-stderr-test: "(\n)ALLOC \(.*\) \{\n(.*\n)*\}(\n)" -> "${1}ALLOC DUMP${3}"
//@ normalize-stderr-test: "\[0x[0-9a-z]..0x[0-9a-z]\]" -> "[0xX..0xY]"

#![feature(core_intrinsics, custom_mir, ptr_metadata)]
use std::intrinsics::mir::*;

// This disables validation and uses custom MIR hit exactly what used to be the UB in the intrinsic,
// rather than getting UB from the typed load or parameter passing.
// Since `PtrMetadata` is now a "normal" field access, it's not UB if fields other than the one
// loaded are uninit.

#[custom_mir(dialect = "runtime")]
pub unsafe fn deref_meta(p: *const *const [i32]) -> std::ptr::Metadata<[i32]> {
    mir! {
        {
            RET = PtrMetadata(*p);
            Return()
        }
    }
}

fn main() {
    let mut p = std::mem::MaybeUninit::<*const [i32]>::uninit();
    unsafe {
        (*p.as_mut_ptr().cast::<[usize; 2]>())[1] = 4;
        let _meta = deref_meta(p.as_ptr().cast());
    }
}

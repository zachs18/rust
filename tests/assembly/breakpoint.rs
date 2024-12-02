//@ revisions: aarch64 x86_64
//@ assembly-output: emit-asm
//@[aarch64] compile-flags: --target aarch64-unknown-linux-gnu
//@[aarch64] needs-llvm-components: aarch64
//@[x86_64] compile-flags: --target x86_64-unknown-linux-gnu -C llvm-args=-x86-asm-syntax=intel
//@[x86_64] needs-llvm-components: x86

#![feature(breakpoint)]
#![crate_type = "lib"]

// CHECK-LABEL: use_bp
// aarch64: brk #0xf000
// x86_64: int3
pub fn use_bp() {
    core::arch::breakpoint();
}

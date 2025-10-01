// FIXME(ptr_metadata_v2) Tests to investigate because there's a `const{transmute}`
// EMIT_MIR_FOR_EACH_PANIC_STRATEGY
//@ test-mir-pass: DataflowConstProp
//@ compile-flags: -Zmir-enable-passes=+InstSimplify-after-simplifycfg
// EMIT_MIR_FOR_EACH_BIT_WIDTH

// EMIT_MIR slice_len.main.DataflowConstProp.diff

// CHECK-LABEL: fn main(
fn main() {
    // CHECK: debug local => [[local:_.*]];
    // CHECK: debug constant => [[constant:_.*]];

    // CHECK-NOT: {{_.*}} = Len(
    // CHECK-NOT: {{_.*}} = Lt(
    // CHECK-NOT: assert(move _
    // CHECK: {{_.*}} = const true;
    // CHECK: assert(const true, "index out of bounds: the length is {} but the index is {}", const 3_usize, const 1_usize)

    // CHECK: [[local]] = copy (*{{_.*}})[1 of 2];
    let local = (&[1u32, 2, 3] as &[u32])[1];

    // CHECK-NOT: {{_.*}} = Len(
    // CHECK-NOT: {{_.*}} = Lt(
    // CHECK-NOT: assert(move _
    const SLICE: &[u32] = &[1, 2, 3];
    // CHECK: {{_.*}} = const true;
    // CHECK: assert(const true, "index out of bounds: the length is {} but the index is {}", const 3_usize, const 1_usize)

    // CHECK-NOT: [[constant]] = {{copy|move}} (*{{_.*}})[_
    // CHECK: [[constant]] = copy (*{{_.*}})[1 of 2];
    let constant = SLICE[1];
}

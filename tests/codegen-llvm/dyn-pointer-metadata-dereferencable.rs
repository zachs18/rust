// ignore-tidy-linelength
//! This file tests that we correctly generate alignment info for trait object metadata,
//! and dereferenceable in release mode.
//@ revisions: zero three
//@ [zero] compile-flags: -C no-prepopulate-passes -Copt-level=0
//@ [three] compile-flags: -Copt-level=3

#![crate_type = "lib"]

// zero: @dyn_ref_arg(ptr align {{[0-9]+}} [[DATA_PTR:%.+]], ptr align {{[0-9]+}} [[VTABLE_PTR:%.+]])
// three: @dyn_ref_arg(ptr{{( nocapture)?}} noundef nonnull readnone align {{[0-9]+}}{{( captures\(none\))?}} [[DATA_PTR:%.+]], ptr noalias{{( nocapture)?}} noundef readonly align {{[0-9]+}}{{( captures\(none\))?}}  dereferenceable({{[0-9]+}}) [[VTABLE_PTR:%.+]])
#[no_mangle]
pub fn dyn_ref_arg(_: &dyn Drop) {}

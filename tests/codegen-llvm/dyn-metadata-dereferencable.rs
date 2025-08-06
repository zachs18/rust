// ignore-tidy-linelength
//! This file tests that we correctly generate alignment info for DynMetadata,
//! and dereferenceable in release mode.
//@ revisions: zero three
//@ [zero] compile-flags: -C no-prepopulate-passes -Copt-level=0
//@ [three] compile-flags: -Copt-level=3

#![crate_type = "lib"]
#![feature(ptr_metadata)]

use std::ptr::DynMetadata;

// COM: these are the expected results after the change
// COM: zero: @dyn_metadata_arg(ptr align {{[0-9]+}} [[VTABLE_PTR:%.+]])
// COM: three: @dyn_metadata_arg(ptr noalias{{( nocapture)?}} noundef readonly align {{[0-9]+}}{{( captures\(none\))?}} dereferenceable({{[0-9]+}}) [[VTABLE_PTR:%.+]])
// COM: the expected result before the change
// zero: @dyn_metadata_arg(ptr [[VTABLE_PTR:%.+]])
// three: @dyn_metadata_arg(ptr{{( nocapture)?}}  noundef nonnull readnone{{( captures\(none\))?}} [[VTABLE_PTR:%.+]])
#[no_mangle]
pub fn dyn_metadata_arg(_: DynMetadata<dyn Drop>) {}

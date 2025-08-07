// ignore-tidy-linelength
//! This file tests that we correctly generate alignment info for DynMetadata,
//! and dereferenceable in release mode.
//@ revisions: zero three
//@ [zero] compile-flags: -C no-prepopulate-passes -Copt-level=0
//@ [three] compile-flags: -Copt-level=3

#![crate_type = "lib"]
#![feature(ptr_metadata)]

use std::ptr::DynMetadata;

// zero: @dyn_metadata_arg(ptr align {{[0-9]+}} [[VTABLE_PTR:%.+]])
// three: @dyn_metadata_arg(ptr noalias{{( nocapture)?}} noundef readonly align {{[0-9]+}}{{( captures\(none\))?}} dereferenceable({{[0-9]+}}) [[VTABLE_PTR:%.+]])
#[no_mangle]
pub fn dyn_metadata_arg(_: DynMetadata<dyn Drop>) {}

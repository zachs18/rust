//@ compile-flags: -Z force-unstable-if-unmarked

// Check that the unstable marker is not added for "rustc_private".

//@ !matches internal/index.html \
//      '//*[@class="desc docblock-short"]/span[@class="stab unstable"]' \
//      ''
//@ !matches internal/index.html \
//      '//*[@class="desc docblock-short"]/span[@class="stab internal"]' \
//      ''
//@ matches - '//dd' 'Docs'

// FIXME(ptr_metadata_v2): This has a false(?) positive for `impl core::init::PinInit<T> for T`.
// disabling for now.
//x@ !has internal/struct.S.html '//*[@class="stab unstable"]' ''
//@ !has internal/struct.S.html '//*[@class="stab internal"]' ''
/// Docs
pub struct S;

fn main() {}

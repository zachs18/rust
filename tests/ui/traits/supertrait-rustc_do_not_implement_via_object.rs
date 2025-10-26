//@ run-pass
// Regression test for #148089, #[rustc_do_not_implement_via_object] traits are currently
// dyn-compatible, even though this makes them and all subtrait trait objects not implement
// their traits. However, subtraits trait objects can still implement their *other* supertraits.
// In #148089, codegen was wrong for such calls.
#![feature(rustc_attrs)]

#[rustc_do_not_implement_via_object]
pub trait NonObjectBase {
    fn non_object_method(&self) { panic!("NonObjectBase::non_object_method should not be called"); }
}
impl<T> NonObjectBase for T {}

pub trait ObjectBase {
    fn object_method(&self) { println!("ObjectBase::object_method"); }
}
impl<T> ObjectBase for T {}

pub trait Sub: NonObjectBase + ObjectBase {
    fn sub_method(&self) { panic!("Sub::sub_method should not be called"); }
}

impl<T> Sub for T {}

fn main() {
    let x: &dyn Sub = &42;
    x.object_method();
    let x: &dyn ObjectBase = x;
    x.object_method();
}

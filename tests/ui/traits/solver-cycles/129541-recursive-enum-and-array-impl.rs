// Regression test for #129541
//~? ERROR recursive type `Hello` has infinite size

//@ revisions: current next
//@ ignore-compare-mode-next-solver (explicit revisions)
//@[next] compile-flags: -Znext-solver

trait Bound {}
trait Normalize {
    type Assoc;
}

impl<T: Bound> Normalize for T {
    type Assoc = T;
}

impl<T: Bound> Normalize for [T] {
    type Assoc = T;
}

impl Bound for Hello {}
enum Hello {
    Variant(<[Hello] as Normalize>::Assoc),
}

fn main() {}

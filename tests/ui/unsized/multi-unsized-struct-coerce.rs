//@ revisions: works broken
//@[works] check-pass
//@[broken] check-fail
#![feature(rustc_attrs)]
#![feature(more_unsized)]
#![feature(unsize)]

use std::fmt::{self, Debug};

struct Pair<T: ?Sized, U: ?Sized> {
    #[rustc_unsizable_field]
    t: T,
    #[rustc_unsizable_field]
    u: U,
}

impl<T: ?Sized + Debug, U: ?Sized + Debug> Debug for Pair<T, U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pair")
            .field("t", &&self.t)
            .field("u", &&self.u)
            .finish()
    }
}

fn unsize_only_first<T: Debug + 'static, U: ?Sized>(p: &Pair<T, U>) -> &Pair<dyn Debug, U> {
    p
}
fn unsize_only_second<T: ?Sized, U: Debug + 'static>(p: &Pair<T, U>) -> &Pair<T, dyn Debug> {
    p
}

fn main() {
    let p: Pair<[u8; 3], [u8; 4]> = Pair { t: [0, 1, 2], u: [3, 4, 5, 6] };

    let a: &Pair<[u8; 3], [u8; 4]> = &p;

    let b: &Pair<[u8], [u8; 4]> = a;
    let c: &Pair<[u8; 3], [u8]> = a;
    let d: &Pair<[u8], [u8]> = a;

    let e: &Pair<[u8], [u8]> = b;
    let f: &Pair<[u8], [u8]> = c;
    let g: &Pair<[u8], [u8]> = d;

    dbg!(a, b, c, d, e, f, g);

    let a: &Pair<[u8; 3], [u8; 4]> = &p;

    let b: &Pair<dyn Debug, [u8; 4]> = a;
    let c: &Pair<[u8; 3], dyn Debug> = a;
    let d: &Pair<dyn Debug, dyn Debug> = a;

    // Because `dyn Debug: Unsize<dyn Debug>`, these two are ambiguous as to which set of fields
    // is being unsized, so the compiler gives up on unsizing and equates the types.
    #[cfg(broken)]
    let e: &Pair<dyn Debug, dyn Debug> = b;
    //[broken]~^ ERROR: mismatched types
    #[cfg(broken)]
    let f: &Pair<dyn Debug, dyn Debug> = c;
    //[broken]~^ ERROR: mismatched types

    // The ambiguity doesn't occur if there are other restrictions.
    #[cfg(works)]
    let e: &Pair<dyn Debug, dyn Debug> = unsize_only_second(b);
    #[cfg(works)]
    let f: &Pair<dyn Debug, dyn Debug> = unsize_only_first(c);

    // If the compiler gives up on unsizing and equates the types, this one still works.
    let g: &Pair<dyn Debug, dyn Debug> = d;

    dbg!(a, b, c, d, e, f, g);
}

//@ known-bug: #137187
use std::ops::Add;

const trait A where
    *const Box<Self>: const Add,
{
    fn b(c: *const Box<Self>) -> <*const Box<Self> as Add>::Output {
        c + c
    }
}

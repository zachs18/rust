//@ known-bug: #134587

use std::ops::Add;

pub fn foo<T>(slf: *const Box<T>)
where
    *const Box<T>: Add,
{
    slf + slf;
}

pub fn foo2<T>(slf: *const Box<T>)
where
    *const Box<T>: Add<u8>,
{
    slf + 1_u8;
}


pub trait TimesTwo
   where *const Box<Self>: Add<*const Box<Self>>,
{
   extern "C" fn t2_ptr(slf: *const Box<Self>)
   -> <*const Box<Self> as Add<*const Box<Self>>>::Output {
       slf + slf
   }
}

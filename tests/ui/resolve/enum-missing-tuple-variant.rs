enum Foo {
    //~^ HELP consider importing this tuple variant
    A(u32),
    B(u32),
}

fn main() {
    let _: Foo = Foo(0);
    //~^ ERROR expected function
    //~| HELP try to construct one of the enum's variants

    let s: Foo = Foo.A(0);
    //~^ ERROR expected value, found enum `Foo`
    //~| HELP use the path separator to refer to a variant

    let s: Foo = Foo.C(0);
    //~^ ERROR expected value, found enum `Foo`
    //~| HELP the following enum variants are available

    match s {
        A(..) => {}
        //~^ ERROR cannot find tuple struct or tuple variant `A` in this scope
        Foo(..) => {}
        //~^ ERROR expected tuple struct or tuple variant
        //~| HELP try to match against one of the enum's variants
        _ => {}
    }
}

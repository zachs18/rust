fn foo() {
    do init tuple ();
    //~^ ERROR `do init` expressions are experimental
}

fn bar() {
    do init struct Option::<i32>::Some { 0: 42 };
    //~^ ERROR `do init` expressions are experimental
}

fn main() {}

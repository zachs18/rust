fn main(){
    let my_var: String(String?);
    //~^ ERROR: invalid `?` in type
    //~| ERROR: parenthesized type parameters may only be used with a `Fn` trait
    //~| ERROR: use of unstable library feature 'allocator_api'
}

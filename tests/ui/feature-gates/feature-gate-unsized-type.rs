pub unsized type Foo { //~ERROR `unsized type`s are experimental
    pub x: usize,
    y: u8,
}

fn main() {}

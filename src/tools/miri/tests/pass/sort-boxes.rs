// Regression test for <https://github.com/rust-lang/rust/issues/151728>.
fn main() {
    let len = 100;
    let elem = |i: usize| {
        Box::new(match i.is_multiple_of(2) {
            true => i,
            false => len + i,
        })
    };
    let mut input: Vec<_> = (0..len).map(elem).collect();
    input.sort()
}

use std::sync::Mutex;

struct Test {
    comps: Mutex<String>,
}

fn main() {}

fn testing(test: Test) {
    let _ = test.comps.state.inner.try_lock();
    //~^ ERROR: field `state` of struct `Mutex` is private
}

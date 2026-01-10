//@ revisions: nofeature yesfeature
//@[yesfeature] check-pass
#![cfg_attr(yesfeature, feature(more_unsized))]

struct Table {
    rows: [[String]],
    //[nofeature]~^ ERROR the size for values of type
}

fn f(table: &Table) -> &[String] {
    &table.rows[0]
    //[nofeature]~^ ERROR the size for values of type
}

fn main() {}

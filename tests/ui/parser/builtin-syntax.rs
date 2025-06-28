#![feature(builtin_syntax)]
// These all need to be in separate functions because `builtin #` syntax currently
// doesn't do any recovery.

fn main() {}

fn unknown_expr() {
    builtin # foobar(); //~ ERROR unknown `builtin #` expression construct
}

fn unknown_ty() {
    type Ty = builtin # foobar(); //~ ERROR unknown `builtin #` type construct
}

fn unknown_pat() {
    match () {
        builtin # foobar() => {}, //~ ERROR unknown `builtin #` pattern construct
    }
}

fn not_identifier_expr() {
    builtin # {}(); //~ ERROR expected identifier after
}

fn not_identifier_ty() {
    type NotIdentifierTy = builtin # {}(); //~ ERROR expected identifier after
}

fn not_identifier_pat() {
    match () {
        builtin # {}() => {}, //~ ERROR expected identifier after
    }
}

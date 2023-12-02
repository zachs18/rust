// The compiler code necessary to support the compile_warning! extension.

use rustc_ast::tokenstream::TokenStream;
use rustc_expand::base::{self, *};
use rustc_span::Span;

pub fn expand_compile_warning<'cx>(
    cx: &'cx mut ExtCtxt<'_>,
    sp: Span,
    tts: TokenStream,
) -> Box<dyn base::MacResult + 'cx> {
    let Some(var) = get_single_str_from_tts(cx, sp, tts, "compile_warning!") else {
        return DummyResult::any(sp);
    };

    #[expect(
        rustc::diagnostic_outside_of_impl,
        reason = "diagnostic message is specified by user"
    )]
    #[expect(rustc::untranslatable_diagnostic, reason = "diagnostic message is specified by user")]
    cx.span_warn(sp, var.to_string());

    DummyResult::any(sp)
}

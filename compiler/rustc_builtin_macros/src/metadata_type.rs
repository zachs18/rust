use rustc_ast::ptr::P;
use rustc_ast::tokenstream::TokenStream;
use rustc_ast::{Ty, ast};
use rustc_errors::PResult;
use rustc_expand::base::{self, DummyResult, ExpandResult, ExtCtxt, MacroExpanderResult};
use rustc_span::Span;

pub(crate) fn expand<'cx>(
    cx: &'cx mut ExtCtxt<'_>,
    sp: Span,
    tts: TokenStream,
) -> MacroExpanderResult<'cx> {
    let ty = match parse_metadata_ty(cx, tts) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ExpandResult::Ready(DummyResult::any(sp, err.emit()));
        }
    };

    ExpandResult::Ready(base::MacEager::ty(cx.ty(sp, ast::TyKind::PtrMetadata(ty))))
}

fn parse_metadata_ty<'a>(cx: &mut ExtCtxt<'a>, stream: TokenStream) -> PResult<'a, P<Ty>> {
    let mut parser = cx.new_parser_from_tts(stream);

    let ty = parser.parse_ty()?;

    Ok(ty)
}

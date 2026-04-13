//! This pass lowers calls to core::slice::len to just PtrMetadata op and a field access.
//! It should run before inlining!

use rustc_abi::FieldIdx;
use rustc_hir::def_id::DefId;
use rustc_index::IndexVec;
use rustc_middle::mir::*;
use rustc_middle::ty::{Ty, TyCtxt};

pub(super) struct LowerSliceLenCalls;

impl<'tcx> crate::MirPass<'tcx> for LowerSliceLenCalls {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        sess.mir_opt_level() > 0
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let language_items = tcx.lang_items();
        let Some(slice_len_fn_item_def_id) = language_items.slice_len_fn() else {
            // there is no lang item to compare to :)
            return;
        };

        // The one successor remains unchanged, so no need to invalidate
        let basic_blocks = body.basic_blocks.as_mut_preserves_cfg();
        for block in basic_blocks {
            // lower `<[_]>::len` calls
            lower_slice_len_call(tcx, block, slice_len_fn_item_def_id, &mut body.local_decls);
        }
    }

    fn is_required(&self) -> bool {
        false
    }
}

fn lower_slice_len_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    block: &mut BasicBlockData<'tcx>,
    slice_len_fn_item_def_id: DefId,
    local_decls: &mut IndexVec<Local, LocalDecl<'tcx>>,
) {
    let terminator = block.terminator();
    if let TerminatorKind::Call {
        func,
        args,
        destination,
        target: Some(bb),
        call_source: CallSource::Normal,
        fn_span,
        ..
    } = &terminator.kind
        // some heuristics for fast rejection
        && let [arg] = &args[..]
        && let Some((fn_def_id, _)) = func.const_fn_def()
        && fn_def_id == slice_len_fn_item_def_id
        && let Some(pointee_ty) = arg.node.ty(local_decls, tcx).builtin_deref(true)
    {
        // perform modifications from something like:
        //     _5 = core::slice::<impl [u8]>::len(move _6) -> bb1
        // into:
        //     _tmp = PtrMetadata(move _6)
        //     _5 = copy (_tmp.0: usize);
        //     goto bb1

        // make new temp local for ptr metadata
        let metadata_local =
            local_decls.push(LocalDecl::new(Ty::new_ptr_metadata(tcx, pointee_ty), *fn_span));

        // make new RValue for PtrMetadata
        let metadata_rvalue = Rvalue::UnaryOp(UnOp::PtrMetadata, arg.node.clone());
        let metadata_statement_kind =
            StatementKind::Assign(Box::new((Place::from(metadata_local), metadata_rvalue)));
        let metadata_statement = Statement::new(terminator.source_info, metadata_statement_kind);

        // make new RValue for usize length
        // FIXME(ptr_metadata_v2_fields): implement multiple fields
        let len_rvalue = Rvalue::Use(Operand::Copy(
            Place::from(metadata_local)
                .project_deeper(&[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)], tcx),
        ));
        let len_statement_kind = StatementKind::Assign(Box::new((*destination, len_rvalue)));
        let len_statement = Statement::new(terminator.source_info, len_statement_kind);

        // modify terminator into simple Goto
        let new_terminator_kind = TerminatorKind::Goto { target: *bb };

        block.statements.push(metadata_statement);
        block.statements.push(len_statement);
        block.terminator_mut().kind = new_terminator_kind;
    }
}

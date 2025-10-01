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
    // inline BasicBlockData::terminator due to borrow splitting
    let terminator = block.terminator.as_ref().expect("invalid terminator state");
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
        //     _tmp = copy _6; // if needed
        //      // .0 is PtrMetadata field of ptr, .0 is len field of PtrMetadata<[u8]>
        //     _5 = copy (_6.1.0: usize);
        //     goto bb1

        let ptr_place = match arg.node {
            Operand::Copy(place) | Operand::Move(place) => place,
            ref op => {
                let ptr_local = local_decls.push(LocalDecl::new(op.ty(local_decls, tcx), *fn_span));
                let copy_ptr_statement =
                    StatementKind::Assign(Box::new((ptr_local.into(), Rvalue::Use(op.clone()))));
                block.statements.push(Statement::new(terminator.source_info, copy_ptr_statement));
                ptr_local.into()
            }
        };

        let meta_ty = Ty::new_ptr_metadata(tcx, pointee_ty);
        // make new RValue for usize length
        let len_rvalue = Rvalue::Use(Operand::Copy(ptr_place.project_deeper(
            &[
                PlaceElem::Field(FieldIdx::ONE, meta_ty),
                PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize),
            ],
            tcx,
        )));
        let len_statement_kind = StatementKind::Assign(Box::new((*destination, len_rvalue)));
        let len_statement = Statement::new(terminator.source_info, len_statement_kind);

        // modify terminator into simple Goto
        let new_terminator_kind = TerminatorKind::Goto { target: *bb };

        block.statements.push(len_statement);
        block.terminator_mut().kind = new_terminator_kind;
    }
}

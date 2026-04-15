// ignore-tidy-filelength
use std::{assert_matches, fmt, iter};

use either::Either;
use rustc_abi::{Align, ExternAbi, FIRST_VARIANT, FieldIdx, FieldsShape, VariantIdx};
use rustc_const_eval::interpret::PointerArithmetic;
use rustc_hir as hir;
use rustc_hir::def_id::DefId;
use rustc_hir::lang_items::LangItem;
use rustc_index::{Idx, IndexVec};
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::*;
use rustc_middle::query::Providers;
use rustc_middle::ty::layout::MetadataFields;
use rustc_middle::ty::{
    self, CoroutineArgs, CoroutineArgsExt, EarlyBinder, GenericArgs, InitAdtComponentArg,
    InitAdtComponentInfo, Ty, TyCtxt,
};
use rustc_middle::{bug, span_bug};
use rustc_span::{DUMMY_SP, Span, Spanned, Symbol, dummy_spanned};
use tracing::{debug, instrument};

use crate::deref_separator::deref_finder;
use crate::elaborate_drop::{DropElaborator, DropFlagMode, DropStyle, Unwind, elaborate_drop};
use crate::patch::MirPatch;
use crate::{
    abort_unwinding_calls, add_call_guards, add_moves_for_packed_drops, inline, instsimplify,
    mentioned_items, pass_manager as pm, remove_noop_landing_pads, run_optimization_passes,
    simplify,
};

mod async_destructor_ctor;

pub(super) fn provide(providers: &mut Providers) {
    providers.mir_shims = make_shim;
}

// Replace Pin<&mut ImplCoroutine> accesses (_1.0) into Pin<&mut ProxyCoroutine> accesses
struct FixProxyFutureDropVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    replace_to: Local,
}

impl<'tcx> MutVisitor<'tcx> for FixProxyFutureDropVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_place(
        &mut self,
        place: &mut Place<'tcx>,
        _context: PlaceContext,
        _location: Location,
    ) {
        if place.local == Local::from_u32(1) {
            if place.projection.len() == 1 {
                assert!(matches!(
                    place.projection.first(),
                    Some(ProjectionElem::Field(FieldIdx::ZERO, _))
                ));
                *place = Place::from(self.replace_to);
            } else if place.projection.len() == 2 {
                assert!(matches!(place.projection[0], ProjectionElem::Field(FieldIdx::ZERO, _)));
                assert!(matches!(place.projection[1], ProjectionElem::Deref));
                *place =
                    Place::from(self.replace_to).project_deeper(&[ProjectionElem::Deref], self.tcx);
            }
        }
    }
}

fn make_shim<'tcx>(tcx: TyCtxt<'tcx>, instance: ty::InstanceKind<'tcx>) -> Body<'tcx> {
    debug!("make_shim({:?})", instance);

    let mut result = match instance {
        ty::InstanceKind::Item(..) => bug!("item {:?} passed to make_shim", instance),
        ty::InstanceKind::VTableShim(def_id) => {
            let adjustment = Adjustment::Deref { source: DerefSource::MutPtr };
            build_call_shim(tcx, instance, Some(adjustment), CallKind::Direct(def_id))
        }
        ty::InstanceKind::FnPtrShim(def_id, ty) => {
            let trait_ = tcx.parent(def_id);
            // Supports `Fn` or `async Fn` traits.
            let adjustment = match tcx
                .fn_trait_kind_from_def_id(trait_)
                .or_else(|| tcx.async_fn_trait_kind_from_def_id(trait_))
            {
                Some(ty::ClosureKind::FnOnce) => Adjustment::Identity,
                Some(ty::ClosureKind::Fn) => Adjustment::Deref { source: DerefSource::ImmRef },
                Some(ty::ClosureKind::FnMut) => Adjustment::Deref { source: DerefSource::MutRef },
                None => bug!("fn pointer {:?} is not an fn", ty),
            };

            build_call_shim(tcx, instance, Some(adjustment), CallKind::Indirect(ty))
        }
        // We are generating a call back to our def-id, which the
        // codegen backend knows to turn to an actual call, be it
        // a virtual call, or a direct call to a function for which
        // indirect calls must be codegen'd differently than direct ones
        // (such as `#[track_caller]`).
        ty::InstanceKind::ReifyShim(def_id, _) => {
            build_call_shim(tcx, instance, None, CallKind::Direct(def_id))
        }
        ty::InstanceKind::ClosureOnceShim { call_once: _, track_caller: _ } => {
            let fn_mut = tcx.require_lang_item(LangItem::FnMut, DUMMY_SP);
            let call_mut = tcx
                .associated_items(fn_mut)
                .in_definition_order()
                .find(|it| it.is_fn())
                .unwrap()
                .def_id;

            build_call_shim(tcx, instance, Some(Adjustment::RefMut), CallKind::Direct(call_mut))
        }

        ty::InstanceKind::ConstructCoroutineInClosureShim {
            coroutine_closure_def_id,
            receiver_by_ref,
        } => build_construct_coroutine_by_move_shim(tcx, coroutine_closure_def_id, receiver_by_ref),

        ty::InstanceKind::DropGlue(def_id, ty) => {
            // FIXME(#91576): Drop shims for coroutines aren't subject to the MIR passes at the end
            // of this function. Is this intentional?
            if let Some(&ty::Coroutine(coroutine_def_id, args)) = ty.map(Ty::kind) {
                let coroutine_body = tcx.optimized_mir(coroutine_def_id);

                let ty::Coroutine(_, id_args) = *tcx.type_of(coroutine_def_id).skip_binder().kind()
                else {
                    bug!()
                };

                // If this is a regular coroutine, grab its drop shim. If this is a coroutine
                // that comes from a coroutine-closure, and the kind ty differs from the "maximum"
                // kind that it supports, then grab the appropriate drop shim. This ensures that
                // the future returned by `<[coroutine-closure] as AsyncFnOnce>::call_once` will
                // drop the coroutine-closure's upvars.
                let body = if id_args.as_coroutine().kind_ty() == args.as_coroutine().kind_ty() {
                    coroutine_body.coroutine_drop().unwrap()
                } else {
                    assert_eq!(
                        args.as_coroutine().kind_ty().to_opt_closure_kind().unwrap(),
                        ty::ClosureKind::FnOnce
                    );
                    tcx.optimized_mir(tcx.coroutine_by_move_body_def_id(coroutine_def_id))
                        .coroutine_drop()
                        .unwrap()
                };

                let mut body = EarlyBinder::bind(body.clone()).instantiate(tcx, args);
                debug!("make_shim({:?}) = {:?}", instance, body);

                pm::run_passes(
                    tcx,
                    &mut body,
                    &[
                        &mentioned_items::MentionedItems,
                        &abort_unwinding_calls::AbortUnwindingCalls,
                        &add_call_guards::CriticalCallEdges,
                    ],
                    Some(MirPhase::Runtime(RuntimePhase::Optimized)),
                    pm::Optimizations::Allowed,
                );

                return body;
            }

            build_drop_shim(tcx, def_id, ty)
        }
        ty::InstanceKind::ThreadLocalShim(..) => build_thread_local_shim(tcx, instance),
        ty::InstanceKind::CloneShim(..) => build_clone_shim(tcx, instance),
        ty::InstanceKind::PtrMetadataCmpShim(..) => build_ptr_metadata_cmp_shim(tcx, instance),
        ty::InstanceKind::PtrMetadataDebugShim(..) => build_ptr_metadata_fmt_shim(tcx, instance),
        ty::InstanceKind::PtrMetadataHashShim(..) => build_ptr_metadata_hash_shim(tcx, instance),
        ty::InstanceKind::InitShim { .. } => build_init_shim(tcx, instance),
        ty::InstanceKind::LayoutForMetaShim { .. } => build_layout_for_meta_shim(tcx, instance),
        ty::InstanceKind::FnPtrAddrShim(def_id, ty) => build_fn_ptr_addr_shim(tcx, def_id, ty),
        ty::InstanceKind::FutureDropPollShim(def_id, proxy_ty, impl_ty) => {
            let mut body =
                async_destructor_ctor::build_future_drop_poll_shim(tcx, def_id, proxy_ty, impl_ty);

            pm::run_passes(
                tcx,
                &mut body,
                &[
                    &mentioned_items::MentionedItems,
                    &abort_unwinding_calls::AbortUnwindingCalls,
                    &add_call_guards::CriticalCallEdges,
                ],
                Some(MirPhase::Runtime(RuntimePhase::PostCleanup)),
                pm::Optimizations::Allowed,
            );
            run_optimization_passes(tcx, &mut body);
            debug!("make_shim({:?}) = {:?}", instance, body);
            return body;
        }
        ty::InstanceKind::AsyncDropGlue(def_id, ty) => {
            let mut body = async_destructor_ctor::build_async_drop_shim(tcx, def_id, ty);

            // Main pass required here is StateTransform to convert sync drop ladder
            // into coroutine.
            // Others are minimal passes as for sync drop glue shim
            pm::run_passes(
                tcx,
                &mut body,
                &[
                    &mentioned_items::MentionedItems,
                    &abort_unwinding_calls::AbortUnwindingCalls,
                    &add_call_guards::CriticalCallEdges,
                    &simplify::SimplifyCfg::MakeShim,
                    &crate::coroutine::StateTransform,
                ],
                Some(MirPhase::Runtime(RuntimePhase::PostCleanup)),
                pm::Optimizations::Allowed,
            );
            run_optimization_passes(tcx, &mut body);
            debug!("make_shim({:?}) = {:?}", instance, body);
            return body;
        }

        ty::InstanceKind::AsyncDropGlueCtorShim(def_id, ty) => {
            let body = async_destructor_ctor::build_async_destructor_ctor_shim(tcx, def_id, ty);
            debug!("make_shim({:?}) = {:?}", instance, body);
            return body;
        }
        ty::InstanceKind::Virtual(..) => {
            bug!("InstanceKind::Virtual ({:?}) is for direct calls only", instance)
        }
        ty::InstanceKind::Intrinsic(_) => {
            bug!("creating shims from intrinsics ({:?}) is unsupported", instance)
        }
    };
    debug!("make_shim({:?}) = untransformed {:?}", instance, result);

    deref_finder(tcx, &mut result, false);

    // We don't validate MIR here because the shims may generate code that's
    // only valid in a `PostAnalysis` param-env. However, since we do initial
    // validation with the MirBuilt phase, which uses a user-facing param-env.
    // This causes validation errors when TAITs are involved.
    pm::run_passes_no_validate(
        tcx,
        &mut result,
        &[
            &mentioned_items::MentionedItems,
            &add_moves_for_packed_drops::AddMovesForPackedDrops,
            &remove_noop_landing_pads::RemoveNoopLandingPads,
            &simplify::SimplifyCfg::MakeShim,
            &instsimplify::InstSimplify::BeforeInline,
            // Perform inlining of `#[rustc_force_inline]`-annotated callees.
            &inline::ForceInline,
            &abort_unwinding_calls::AbortUnwindingCalls,
            &add_call_guards::CriticalCallEdges,
        ],
        Some(MirPhase::Runtime(RuntimePhase::Optimized)),
    );

    debug!("make_shim({:?}) = {:?}", instance, result);

    result
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum DerefSource {
    /// `fn shim(&self) { inner(*self )}`.
    ImmRef,
    /// `fn shim(&mut self) { inner(*self )}`.
    MutRef,
    /// `fn shim(*mut self) { inner(*self )}`.
    MutPtr,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Adjustment {
    /// Pass the receiver as-is.
    Identity,

    /// We get passed a reference or a raw pointer to `self` and call the target with `*self`.
    ///
    /// This either copies `self` (if `Self: Copy`, eg. for function items), or moves out of it
    /// (for `VTableShim`, which effectively is passed `&own Self`).
    Deref { source: DerefSource },

    /// We get passed `self: Self` and call the target with `&mut self`.
    ///
    /// In this case we need to ensure that the `Self` is dropped after the call, as the callee
    /// won't do it for us.
    RefMut,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum CallKind<'tcx> {
    /// Call the `FnPtr` that was passed as the receiver.
    Indirect(Ty<'tcx>),

    /// Call a known `FnDef`.
    Direct(DefId),
}

fn local_decls_for_sig<'tcx>(
    sig: &ty::FnSig<'tcx>,
    span: Span,
) -> IndexVec<Local, LocalDecl<'tcx>> {
    iter::once(LocalDecl::new(sig.output(), span))
        .chain(sig.inputs().iter().map(|ity| LocalDecl::new(*ity, span).immutable()))
        .collect()
}

fn dropee_emit_retag<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut Body<'tcx>,
    mut dropee_ptr: Place<'tcx>,
    span: Span,
) -> Place<'tcx> {
    if tcx.sess.opts.unstable_opts.mir_emit_retag {
        let source_info = SourceInfo::outermost(span);
        // We want to treat the function argument as if it was passed by `&mut`. As such, we
        // generate
        // ```
        // temp = &mut *arg;
        // Retag(temp, FnEntry)
        // ```
        // It's important that we do this first, before anything that depends on `dropee_ptr`
        // has been put into the body.
        let reborrow = Rvalue::Ref(
            tcx.lifetimes.re_erased,
            BorrowKind::Mut { kind: MutBorrowKind::Default },
            tcx.mk_place_deref(dropee_ptr),
        );
        let ref_ty = reborrow.ty(body.local_decls(), tcx);
        dropee_ptr = body.local_decls.push(LocalDecl::new(ref_ty, span)).into();
        let new_statements = [
            StatementKind::Assign(Box::new((dropee_ptr, reborrow))),
            StatementKind::Retag(RetagKind::FnEntry, Box::new(dropee_ptr)),
        ];
        for s in new_statements {
            body.basic_blocks_mut()[START_BLOCK].statements.push(Statement::new(source_info, s));
        }
    }
    dropee_ptr
}

fn build_drop_shim<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId, ty: Option<Ty<'tcx>>) -> Body<'tcx> {
    debug!("build_drop_shim(def_id={:?}, ty={:?})", def_id, ty);

    assert!(!matches!(ty, Some(ty) if ty.is_coroutine()));

    let args = if let Some(ty) = ty {
        tcx.mk_args(&[ty.into()])
    } else {
        GenericArgs::identity_for_item(tcx, def_id)
    };
    let sig = tcx.fn_sig(def_id).instantiate(tcx, args);
    let sig = tcx.instantiate_bound_regions_with_erased(sig);
    let span = tcx.def_span(def_id);

    let source_info = SourceInfo::outermost(span);

    let return_block = BasicBlock::new(1);
    let mut blocks = IndexVec::with_capacity(2);
    let block = |blocks: &mut IndexVec<_, _>, kind| {
        blocks.push(BasicBlockData::new(Some(Terminator { source_info, kind }), false))
    };
    block(&mut blocks, TerminatorKind::Goto { target: return_block });
    block(&mut blocks, TerminatorKind::Return);

    let source = MirSource::from_instance(ty::InstanceKind::DropGlue(def_id, ty));
    let mut body =
        new_body(source, blocks, local_decls_for_sig(&sig, span), sig.inputs().len(), span);

    // The first argument (index 0), but add 1 for the return value.
    let dropee_ptr = Place::from(Local::new(1 + 0));
    let dropee_ptr = dropee_emit_retag(tcx, &mut body, dropee_ptr, span);

    if ty.is_some() {
        let patch = {
            let typing_env = ty::TypingEnv::post_analysis(tcx, def_id);
            let mut elaborator = DropShimElaborator {
                body: &body,
                patch: MirPatch::new(&body),
                tcx,
                typing_env,
                produce_async_drops: false,
            };
            let dropee = tcx.mk_place_deref(dropee_ptr);
            let resume_block = elaborator.patch.resume_block();
            elaborate_drop(
                &mut elaborator,
                source_info,
                dropee,
                (),
                return_block,
                Unwind::To(resume_block),
                START_BLOCK,
                None,
            );
            elaborator.patch
        };
        patch.apply(&mut body);
    }

    body
}

fn new_body<'tcx>(
    source: MirSource<'tcx>,
    basic_blocks: IndexVec<BasicBlock, BasicBlockData<'tcx>>,
    local_decls: IndexVec<Local, LocalDecl<'tcx>>,
    arg_count: usize,
    span: Span,
) -> Body<'tcx> {
    let mut body = Body::new(
        source,
        basic_blocks,
        IndexVec::from_elem_n(
            SourceScopeData {
                span,
                parent_scope: None,
                inlined: None,
                inlined_parent_scope: None,
                local_data: ClearCrossCrate::Clear,
            },
            1,
        ),
        local_decls,
        IndexVec::new(),
        arg_count,
        vec![],
        span,
        None,
        // FIXME(compiler-errors): is this correct?
        None,
    );
    // Shims do not directly mention any consts.
    body.set_required_consts(Vec::new());
    body
}

pub(super) struct DropShimElaborator<'a, 'tcx> {
    pub body: &'a Body<'tcx>,
    pub patch: MirPatch<'tcx>,
    pub tcx: TyCtxt<'tcx>,
    pub typing_env: ty::TypingEnv<'tcx>,
    pub produce_async_drops: bool,
}

impl fmt::Debug for DropShimElaborator<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("DropShimElaborator").finish_non_exhaustive()
    }
}

impl<'a, 'tcx> DropElaborator<'a, 'tcx> for DropShimElaborator<'a, 'tcx> {
    type Path = ();

    fn patch_ref(&self) -> &MirPatch<'tcx> {
        &self.patch
    }
    fn patch(&mut self) -> &mut MirPatch<'tcx> {
        &mut self.patch
    }
    fn body(&self) -> &'a Body<'tcx> {
        self.body
    }
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }
    fn typing_env(&self) -> ty::TypingEnv<'tcx> {
        self.typing_env
    }

    fn terminator_loc(&self, bb: BasicBlock) -> Location {
        self.patch.terminator_loc(self.body, bb)
    }
    fn allow_async_drops(&self) -> bool {
        self.produce_async_drops
    }

    fn drop_style(&self, _path: Self::Path, mode: DropFlagMode) -> DropStyle {
        match mode {
            DropFlagMode::Shallow => {
                // Drops for the contained fields are "shallow" and "static" - they will simply call
                // the field's own drop glue.
                DropStyle::Static
            }
            DropFlagMode::Deep => {
                // The top-level drop is "deep" and "open" - it will be elaborated to a drop ladder
                // dropping each field contained in the value.
                DropStyle::Open
            }
        }
    }

    fn get_drop_flag(&mut self, _path: Self::Path) -> Option<Operand<'tcx>> {
        None
    }

    fn clear_drop_flag(&mut self, _location: Location, _path: Self::Path, _mode: DropFlagMode) {}

    fn field_subpath(&self, _path: Self::Path, _field: FieldIdx) -> Option<Self::Path> {
        None
    }
    fn deref_subpath(&self, _path: Self::Path) -> Option<Self::Path> {
        None
    }
    fn downcast_subpath(&self, _path: Self::Path, _variant: VariantIdx) -> Option<Self::Path> {
        Some(())
    }
    fn array_subpath(&self, _path: Self::Path, _index: u64, _size: u64) -> Option<Self::Path> {
        None
    }
}

fn build_thread_local_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
) -> Body<'tcx> {
    let def_id = instance.def_id();

    let span = tcx.def_span(def_id);
    let source_info = SourceInfo::outermost(span);

    let blocks = IndexVec::from_raw(vec![BasicBlockData::new_stmts(
        vec![Statement::new(
            source_info,
            StatementKind::Assign(Box::new((
                Place::return_place(),
                Rvalue::ThreadLocalRef(def_id),
            ))),
        )],
        Some(Terminator { source_info, kind: TerminatorKind::Return }),
        false,
    )]);

    new_body(
        MirSource::from_instance(instance),
        blocks,
        IndexVec::from_raw(vec![LocalDecl::new(tcx.thread_local_ptr_ty(def_id), span)]),
        0,
        span,
    )
}

struct ShimBuilder<'tcx, Extra> {
    tcx: TyCtxt<'tcx>,
    local_decls: IndexVec<Local, LocalDecl<'tcx>>,
    blocks: IndexVec<BasicBlock, BasicBlockData<'tcx>>,
    span: Span,
    sig: ty::FnSig<'tcx>,
    instance: ty::InstanceKind<'tcx>,
    extra: Extra,
}

impl<'tcx, Extra> ShimBuilder<'tcx, Extra> {
    fn into_mir(self) -> Body<'tcx> {
        let source = MirSource::from_instance(self.instance);
        new_body(source, self.blocks, self.local_decls, self.sig.inputs().len(), self.span)
    }

    fn source_info(&self) -> SourceInfo {
        SourceInfo::outermost(self.span)
    }

    fn block(
        &mut self,
        statements: Vec<Statement<'tcx>>,
        kind: TerminatorKind<'tcx>,
        is_cleanup: bool,
    ) -> BasicBlock {
        let source_info = self.source_info();
        self.blocks.push(BasicBlockData::new_stmts(
            statements,
            Some(Terminator { source_info, kind }),
            is_cleanup,
        ))
    }

    /// Gives the index of an upcoming BasicBlock, with an offset.
    /// offset=0 will give you the index of the next BasicBlock,
    /// offset=1 will give the index of the next-to-next block,
    /// offset=-1 will give you the index of the last-created block
    fn block_index_offset(&self, offset: usize) -> BasicBlock {
        BasicBlock::new(self.blocks.len() + offset)
    }

    fn make_statement(&self, kind: StatementKind<'tcx>) -> Statement<'tcx> {
        Statement::new(self.source_info(), kind)
    }

    fn make_assign(&self, dest: Place<'tcx>, value: Rvalue<'tcx>) -> Statement<'tcx> {
        Statement::new(self.source_info(), StatementKind::Assign(Box::new((dest, value))))
    }

    fn make_place(&mut self, mutability: Mutability, ty: Ty<'tcx>) -> Place<'tcx> {
        let span = self.span;
        let mut local = LocalDecl::new(ty, span);
        if mutability.is_not() {
            local = local.immutable();
        }
        Place::from(self.local_decls.push(local))
    }

    fn make_set_bool_stmt(&self, place: Place<'tcx>, value: bool) -> Statement<'tcx> {
        self.make_assign(
            place,
            Rvalue::Use(Operand::const_from_scalar(
                self.tcx,
                self.tcx.types.bool,
                interpret::Scalar::from_bool(value),
                self.span,
            )),
        )
    }
}

/// Builds a `Clone::clone` shim for `self_ty`. Here, `def_id` is `Clone::clone`.
fn build_clone_shim<'tcx>(tcx: TyCtxt<'tcx>, instance: ty::InstanceKind<'tcx>) -> Body<'tcx> {
    let ty::InstanceKind::CloneShim(def_id, self_ty) = instance else { unreachable!() };
    debug!("build_clone_shim(def_id={:?})", def_id);

    let mut builder = CloneShimBuilder::new(tcx, instance, def_id, self_ty);

    let dest = Place::return_place();
    let src = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));

    match self_ty.kind() {
        ty::FnDef(..) | ty::FnPtr(..) => builder.copy_shim(),
        ty::Closure(_, args) => builder.tuple_like_shim(dest, src, args.as_closure().upvar_tys()),
        ty::CoroutineClosure(_, args) => {
            builder.tuple_like_shim(dest, src, args.as_coroutine_closure().upvar_tys())
        }
        ty::Tuple(..) => builder.tuple_like_shim(dest, src, self_ty.tuple_fields()),
        ty::Coroutine(coroutine_def_id, args) => {
            assert_eq!(tcx.coroutine_movability(*coroutine_def_id), hir::Movability::Movable);
            builder.coroutine_shim(dest, src, *coroutine_def_id, args.as_coroutine())
        }
        _ => bug!("clone shim for `{:?}` which is not `Copy` and is not an aggregate", self_ty),
    };

    builder.into_mir()
}

struct CloneShimExtra {
    clone_def_id: DefId,
}

type CloneShimBuilder<'tcx> = ShimBuilder<'tcx, CloneShimExtra>;

impl<'tcx> CloneShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        def_id: DefId,
        self_ty: Ty<'tcx>,
    ) -> Self {
        // we must instantiate the self_ty because it's
        // otherwise going to be TySelf and we can't index
        // or access fields of a Place of type TySelf.
        let sig = tcx.fn_sig(def_id).instantiate(tcx, &[self_ty.into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(def_id);

        CloneShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: CloneShimExtra { clone_def_id: def_id },
        }
    }

    fn copy_shim(&mut self) {
        let rcvr = self.tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
        let ret_statement =
            self.make_assign(Place::return_place(), Rvalue::Use(Operand::Copy(rcvr)));
        self.block(vec![ret_statement], TerminatorKind::Return, false);
    }

    fn make_clone_call(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        ty: Ty<'tcx>,
        next: BasicBlock,
        cleanup: BasicBlock,
    ) {
        let tcx = self.tcx;

        // `func == Clone::clone(&ty) -> ty`
        let func_ty = Ty::new_fn_def(tcx, self.extra.clone_def_id, [ty]);
        let func = Operand::Constant(Box::new(ConstOperand {
            span: self.span,
            user_ty: None,
            const_: Const::zero_sized(func_ty),
        }));

        let ref_loc =
            self.make_place(Mutability::Not, Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, ty));

        // `let ref_loc: &ty = &src;`
        let statement = self
            .make_assign(ref_loc, Rvalue::Ref(tcx.lifetimes.re_erased, BorrowKind::Shared, src));

        // `let loc = Clone::clone(ref_loc);`
        self.block(
            vec![statement],
            TerminatorKind::Call {
                func,
                args: [Spanned { node: Operand::Move(ref_loc), span: DUMMY_SP }].into(),
                destination: dest,
                target: Some(next),
                unwind: UnwindAction::Cleanup(cleanup),
                call_source: CallSource::Normal,
                fn_span: self.span,
            },
            false,
        );
    }

    fn clone_fields<I>(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        target: BasicBlock,
        mut unwind: BasicBlock,
        tys: I,
    ) -> BasicBlock
    where
        I: IntoIterator<Item = Ty<'tcx>>,
    {
        // For an iterator of length n, create 2*n + 1 blocks.
        for (i, ity) in tys.into_iter().enumerate() {
            // Each iteration creates two blocks, referred to here as block 2*i and block 2*i + 1.
            //
            // Block 2*i attempts to clone the field. If successful it branches to 2*i + 2 (the
            // next clone block). If unsuccessful it branches to the previous unwind block, which
            // is initially the `unwind` argument passed to this function.
            //
            // Block 2*i + 1 is the unwind block for this iteration. It drops the cloned value
            // created by block 2*i. We store this block in `unwind` so that the next clone block
            // will unwind to it if cloning fails.

            let field = FieldIdx::new(i);
            let src_field = self.tcx.mk_place_field(src, field, ity);

            let dest_field = self.tcx.mk_place_field(dest, field, ity);

            let next_unwind = self.block_index_offset(1);
            let next_block = self.block_index_offset(2);
            self.make_clone_call(dest_field, src_field, ity, next_block, unwind);
            self.block(
                vec![],
                TerminatorKind::Drop {
                    place: dest_field,
                    target: unwind,
                    unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
                    replace: false,
                    drop: None,
                    async_fut: None,
                },
                /* is_cleanup */ true,
            );
            unwind = next_unwind;
        }
        // If all clones succeed then we end up here.
        self.block(vec![], TerminatorKind::Goto { target }, false);
        unwind
    }

    fn tuple_like_shim<I>(&mut self, dest: Place<'tcx>, src: Place<'tcx>, tys: I)
    where
        I: IntoIterator<Item = Ty<'tcx>>,
    {
        self.block(vec![], TerminatorKind::Goto { target: self.block_index_offset(3) }, false);
        let unwind = self.block(vec![], TerminatorKind::UnwindResume, true);
        let target = self.block(vec![], TerminatorKind::Return, false);

        let _final_cleanup_block = self.clone_fields(dest, src, target, unwind, tys);
    }

    fn coroutine_shim(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        coroutine_def_id: DefId,
        args: CoroutineArgs<TyCtxt<'tcx>>,
    ) {
        self.block(vec![], TerminatorKind::Goto { target: self.block_index_offset(3) }, false);
        let unwind = self.block(vec![], TerminatorKind::UnwindResume, true);
        // This will get overwritten with a switch once we know the target blocks
        let switch = self.block(vec![], TerminatorKind::Unreachable, false);
        let unwind = self.clone_fields(dest, src, switch, unwind, args.upvar_tys());
        let target = self.block(vec![], TerminatorKind::Return, false);
        let unreachable = self.block(vec![], TerminatorKind::Unreachable, false);
        let mut cases = Vec::with_capacity(args.state_tys(coroutine_def_id, self.tcx).count());
        for (index, state_tys) in args.state_tys(coroutine_def_id, self.tcx).enumerate() {
            let variant_index = VariantIdx::new(index);
            let dest = self.tcx.mk_place_downcast_unnamed(dest, variant_index);
            let src = self.tcx.mk_place_downcast_unnamed(src, variant_index);
            let clone_block = self.block_index_offset(1);
            let start_block = self.block(
                vec![self.make_statement(StatementKind::SetDiscriminant {
                    place: Box::new(Place::return_place()),
                    variant_index,
                })],
                TerminatorKind::Goto { target: clone_block },
                false,
            );
            cases.push((index as u128, start_block));
            let _final_cleanup_block = self.clone_fields(dest, src, target, unwind, state_tys);
        }
        let discr_ty = args.discr_ty(self.tcx);
        let temp = self.make_place(Mutability::Mut, discr_ty);
        let rvalue = Rvalue::Discriminant(src);
        let statement = self.make_assign(temp, rvalue);
        match &mut self.blocks[switch] {
            BasicBlockData { statements, terminator: Some(Terminator { kind, .. }), .. } => {
                statements.push(statement);
                *kind = TerminatorKind::SwitchInt {
                    discr: Operand::Move(temp),
                    targets: SwitchTargets::new(cases.into_iter(), unreachable),
                };
            }
            BasicBlockData { terminator: None, .. } => unreachable!(),
        }
    }
}

/// Builds a `Meta(Sized|Aligned)::(un)checked_(size|align|layout)_for_meta` shim for `self_ty`.
/// Here, `def_id` is of the method.
fn build_layout_for_meta_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
) -> Body<'tcx> {
    let ty::InstanceKind::LayoutForMetaShim { method_def, checked, layout_part, self_ty } =
        instance
    else {
        unreachable!()
    };
    debug!("build_clone_shim(def_id={:?})", method_def);

    let mut builder =
        LayoutForMetaShimBuilder::new(tcx, instance, method_def, checked, layout_part, self_ty);

    match layout_part {
        ty::LayoutPart::Alignment => builder.only_alignment_shim(),
        ty::LayoutPart::Size | ty::LayoutPart::Layout => builder.layout_shim(),
    }

    builder.into_mir()
}

struct LayoutForMetaShimExtra<'tcx> {
    method_def_id: DefId,
    self_ty: Ty<'tcx>,
    checked: bool,
    layout_part: ty::LayoutPart,
}
type LayoutForMetaShimBuilder<'tcx> = ShimBuilder<'tcx, LayoutForMetaShimExtra<'tcx>>;

impl<'tcx> LayoutForMetaShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        method_def_id: DefId,
        checked: bool,
        layout_part: ty::LayoutPart,
        self_ty: Ty<'tcx>,
    ) -> Self {
        let sig = tcx.fn_sig(method_def_id).instantiate(tcx, &[self_ty.into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(method_def_id);

        LayoutForMetaShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: LayoutForMetaShimExtra { method_def_id, self_ty, checked, layout_part },
        }
    }

    fn layout_shim(&mut self) {
        let LayoutForMetaShimExtra { method_def_id, self_ty, checked, layout_part } = self.extra;
        let tcx = self.tcx;
        let typing_env = ty::TypingEnv::fully_monomorphized();

        let dest = Place::return_place();
        let dest_ty = dest.ty(&self.local_decls, tcx).ty;
        let meta = Place::from(Local::new(1 + 0));
        let option_did = tcx.require_lang_item(LangItem::Option, self.span);
        let alignment_struct_ty = tcx.ty_alignment_struct(self.span);
        let max_size = Operand::const_from_scalar(
            tcx,
            tcx.types.usize,
            interpret::Scalar::from_target_usize(tcx.max_size_of_val().bytes(), &tcx),
            self.span,
        );
        let size_align_tup_ty = Ty::new_tup(tcx, &[tcx.types.usize, alignment_struct_ty]);
        let include_alignment = matches!(layout_part, ty::LayoutPart::Layout);
        // only used if `checked`
        let checked_dest_inner_ty =
            if include_alignment { size_align_tup_ty } else { tcx.types.usize };

        let alignment_max_fn = Operand::function_handle(
            tcx,
            tcx.require_lang_item(LangItem::AlignmentMax, self.span),
            [],
            self.span,
        );
        let alignment_min_fn = Operand::function_handle(
            tcx,
            tcx.require_lang_item(LangItem::AlignmentMin, self.span),
            [],
            self.span,
        );

        let layout = match tcx.layout_of(typing_env.as_query_input(self_ty)) {
            Ok(layout) => layout,
            Err(err) => {
                // If `self_ty` doesn't have a valid layout, then there should already have been an error,
                // but we still need to emit some MIR.
                tcx.dcx().delayed_bug(format!(
                    "layout_for_meta shim for type with invalid layout: {err:?}"
                ));
                self.block(vec![], TerminatorKind::Unreachable, false);
                return;
            }
        };
        let (size, alignment) = match self_ty.kind() {
            _ if self_ty.is_sized(tcx, typing_env) => {
                // If `Self: Sized`, then `layout.size/align` are accurate, and we can just return them.
                (
                    Operand::const_from_scalar(
                        tcx,
                        tcx.types.usize,
                        interpret::Scalar::from_target_usize(layout.size.bytes(), &tcx),
                        self.span,
                    ),
                    Some(Operand::const_from_scalar(
                        self.tcx,
                        alignment_struct_ty,
                        interpret::Scalar::from_target_usize(layout.align.abi.bytes(), &tcx),
                        self.span,
                    )),
                )
            }
            ty::Bool
            | ty::Char
            | ty::Int(_)
            | ty::Uint(_)
            | ty::Float(_)
            | ty::FnDef(..)
            | ty::FnPtr(..)
            | ty::Closure(..)
            | ty::CoroutineClosure(..)
            | ty::Coroutine(..)
            | ty::CoroutineWitness(..)
            | ty::Never
            | ty::UntypedPtr { .. }
            | ty::PtrMetadata(..)
            | ty::RawPtr(..)
            | ty::Ref(..)
            | ty::InitAdt(..)
            | ty::InitArray(..)
            | ty::InitArrayRepeat(..)
            | ty::InitSliceRepeat(..)
            | ty::InitTuple(..)
            | ty::Error(_) => bug!("{} should be `Sized`", self_ty),
            ty::Foreign(..) => bug!("{} should not be `MetaSized`", self_ty),

            ty::Adt(def, ..) if def.is_unsized_type() => bug!(
                "AdtKind::UnsizedType should have manual MetaSized/MetaAligned impls, not builtin"
            ),
            ty::Adt(def, ..) if def.is_enum() => todo!("unsized enums"),
            ty::Adt(def, args) if def.is_union() => {
                let size_acc = self.make_place(ty::Mutability::Mut, tcx.types.usize);
                let align_acc = self.make_place(ty::Mutability::Mut, alignment_struct_ty);
                // FIXME: keep track of statically-sized/aligned field separately to avoid bloat.

                // We need to get the alignment of all the fields, since we need to get the alignment of the type
                // to round the size up, so we use `(un)checked_layout_for_meta` for the field layout calls,
                // even if this call itself is `*_size_for_meta`.
                let meta_sized =
                    tcx.associated_items(tcx.require_lang_item(LangItem::MetaSized, self.span));
                let (field_method_sym, field_ret_ty) = if checked {
                    ("checked_layout_for_meta", Ty::new_option(tcx, size_align_tup_ty))
                } else {
                    ("unchecked_layout_for_meta", size_align_tup_ty)
                };
                let field_method_def_id = meta_sized
                    .filter_by_name_unhygienic(Symbol::intern(field_method_sym))
                    .next()
                    .unwrap()
                    .def_id;

                for (field_idx, field) in def.non_enum_variant().fields.iter_enumerated() {
                    let field_ty = field.ty(tcx, args);
                    let field_meta_ty = Ty::new_ptr_metadata(tcx, field_ty);
                    let field_meta =
                        meta.project_deeper(&[PlaceElem::Field(field_idx, field_meta_ty)], tcx);

                    let field_layout_ret = self.make_place(ty::Mutability::Not, field_ret_ty);

                    // bb:
                    //  field_ret = <T as MetaSized>::(un)checked_layout_for_meta(Copy meta) [return -> nextbb, unwind continue]
                    // nextbb: ...
                    self.block(
                        vec![],
                        TerminatorKind::Call {
                            func: Operand::function_handle(
                                self.tcx,
                                field_method_def_id,
                                [ty::GenericArg::from(field_ty)],
                                self.span,
                            ),
                            args: Box::new([Spanned {
                                node: Operand::Copy(field_meta),
                                span: self.span,
                            }]),
                            destination: field_layout_ret,
                            target: Some(self.block_index_offset(1)),
                            // `UnwindAction::Continue` is fine since layout computation shims never have any locals with drop glue,
                            // only `ptr::Metadata<_>`, `Alignment`, `usize`, and tuple or `Option`.
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );

                    let field_layout = if checked {
                        self.question_mark_blocks(
                            field_layout_ret,
                            size_align_tup_ty,
                            checked_dest_inner_ty,
                        )
                    } else {
                        field_layout_ret
                    };

                    let field_size = field_layout
                        .project_deeper(&[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)], tcx);
                    let field_align = field_layout.project_deeper(
                        &[PlaceElem::Field(FieldIdx::ONE, alignment_struct_ty)],
                        tcx,
                    );

                    if field_idx.as_usize() == 0 {
                        // First field, just set the accumulators to the field layout.
                        // Don't need to do any checking or rounding.
                        self.block(
                            vec![
                                self.make_assign(size_acc, Rvalue::Use(Operand::Copy(field_size))),
                                self.make_assign(
                                    align_acc,
                                    Rvalue::Use(Operand::Copy(field_align)),
                                ),
                            ],
                            TerminatorKind::Goto { target: self.block_index_offset(1) },
                            false,
                        );
                    } else {
                        // sizecmpbb:
                        //  _size_gt_result = field_size > size_acc;
                        //  switchInt(_size_gt_result) [1 => setbb, 2 => alignbb];
                        // sizesetbb:
                        //  size_acc = field_size;
                        //  goto -> nextbb;
                        // alignbb:
                        //  align_acc = Alignment::max(align_acc, field_align) [return -> nextbb, unwind continue];
                        // nextbb: ...

                        // sizecmpbb:
                        let size_gt_result = self.make_place(ty::Mutability::Not, tcx.types.bool);
                        self.block(
                            vec![self.make_assign(
                                size_gt_result,
                                Rvalue::BinaryOp(
                                    BinOp::Gt,
                                    Box::new((Operand::Copy(field_size), Operand::Copy(size_acc))),
                                ),
                            )],
                            TerminatorKind::SwitchInt {
                                discr: Operand::Copy(size_gt_result),
                                targets: SwitchTargets::static_if(
                                    1,
                                    self.block_index_offset(1),
                                    self.block_index_offset(2),
                                ),
                            },
                            false,
                        );

                        // sizesetbb:
                        self.block(
                            vec![
                                self.make_assign(size_acc, Rvalue::Use(Operand::Copy(field_size))),
                            ],
                            TerminatorKind::Goto { target: self.block_index_offset(1) },
                            false,
                        );

                        // alignbb:
                        self.block(
                            vec![],
                            TerminatorKind::Call {
                                func: alignment_max_fn.clone(),
                                args: Box::new([
                                    Spanned { node: Operand::Copy(align_acc), span: self.span },
                                    Spanned { node: Operand::Copy(field_align), span: self.span },
                                ]),
                                destination: align_acc,
                                target: Some(self.block_index_offset(1)),
                                unwind: UnwindAction::Continue,
                                call_source: CallSource::Misc,
                                fn_span: self.span,
                            },
                            false,
                        );
                    }
                }

                // Clamp alignment to `repr(packed)`
                if let Some(pack) = def.repr().pack {
                    // alignbb:
                    self.block(
                        vec![],
                        TerminatorKind::Call {
                            func: alignment_min_fn.clone(),
                            args: Box::new([
                                Spanned { node: Operand::Copy(align_acc), span: self.span },
                                Spanned {
                                    node: Operand::const_from_scalar(
                                        self.tcx,
                                        alignment_struct_ty,
                                        interpret::Scalar::from_target_usize(pack.bytes(), &tcx),
                                        self.span,
                                    ),
                                    span: self.span,
                                },
                            ]),
                            destination: align_acc,
                            target: Some(self.block_index_offset(1)),
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );
                }

                // Raise alignment to `repr(align)`
                if let Some(overalign) = def.repr().align {
                    // alignbb:
                    self.block(
                        vec![],
                        TerminatorKind::Call {
                            func: alignment_max_fn.clone(),
                            args: Box::new([
                                Spanned { node: Operand::Copy(align_acc), span: self.span },
                                Spanned {
                                    node: Operand::const_from_scalar(
                                        self.tcx,
                                        alignment_struct_ty,
                                        interpret::Scalar::from_target_usize(
                                            overalign.bytes(),
                                            &tcx,
                                        ),
                                        self.span,
                                    ),
                                    span: self.span,
                                },
                            ]),
                            destination: align_acc,
                            target: Some(self.block_index_offset(1)),
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );
                }

                let full_size = self.round_size_up_to_alignment(
                    size_acc,
                    Operand::Copy(align_acc),
                    checked_dest_inner_ty,
                );
                (Operand::Copy(full_size), Some(Operand::Copy(align_acc)))
            }
            ty::Adt(def, args) => {
                let FieldsShape::Arbitrary { offsets: _, ref in_memory_order } = layout.fields
                else {
                    bug!("struct had non-Arbitrary FieldsShape")
                };
                let variant = def.non_enum_variant();
                let in_order_fields = in_memory_order.iter().map(|&field_idx| {
                    let field_ty = variant.fields[field_idx].ty(tcx, args);
                    (field_idx, field_ty)
                });
                let (size, alignment) = self.struct_like_layout(
                    in_order_fields,
                    meta,
                    def.repr().pack,
                    def.repr().align,
                    checked_dest_inner_ty,
                );
                (size, Some(alignment))
            }
            ty::Tuple(..) => todo!(),

            ty::Str => (
                Operand::Copy(
                    meta.project_deeper(&[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)], tcx),
                ),
                Some(Operand::const_from_scalar(
                    self.tcx,
                    alignment_struct_ty,
                    interpret::Scalar::from_target_usize(1, &tcx),
                    self.span,
                )),
            ),
            &ty::Slice(elem_ty) | &ty::Array(elem_ty, ..) => {
                // The alignment of a slice or array is the alignment of the element,
                // and the size is the element size times the length

                let elem_meta_ty = Ty::new_ptr_metadata(tcx, elem_ty);
                let (elem_meta_idx, len) = match self_ty.kind() {
                    ty::Slice(_) => (
                        FieldIdx::ONE,
                        Operand::Copy(meta.project_deeper(
                            &[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)],
                            tcx,
                        )),
                    ),
                    ty::Array(_, len) => {
                        let Some(len) = len.try_to_target_usize(tcx) else {
                            tcx.dcx().delayed_bug(format!(
                                "layout_for_meta shim for array with invalid length: {len:?}"
                            ));
                            self.block(vec![], TerminatorKind::Unreachable, false);
                            return;
                        };
                        (
                            FieldIdx::ZERO,
                            Operand::const_from_scalar(
                                tcx,
                                tcx.types.usize,
                                interpret::Scalar::from_target_usize(len, &tcx),
                                self.span,
                            ),
                        )
                    }
                    _ => unreachable!(),
                };
                let elem_meta =
                    meta.project_deeper(&[PlaceElem::Field(elem_meta_idx, elem_meta_ty)], tcx);

                let elem_result = self.make_place(ty::Mutability::Not, dest_ty);

                self.block(
                    vec![],
                    TerminatorKind::Call {
                        func: Operand::function_handle(
                            self.tcx,
                            method_def_id,
                            [ty::GenericArg::from(elem_ty)],
                            self.span,
                        ),
                        args: Box::new([Spanned {
                            node: Operand::Copy(elem_meta),
                            span: self.span,
                        }]),
                        destination: elem_result,
                        target: Some(self.block_index_offset(1)),
                        // `UnwindAction::Continue` is fine since layout computation shims never have any locals with drop glue,
                        // only `ptr::Metadata<_>`, `Alignment`, `usize`, and tuple or `Option`.
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Misc,
                        fn_span: self.span,
                    },
                    false,
                );

                if checked {
                    // if elem_result.is_none() { return None }
                    // `elem_result` is the same type as this function's return type
                    let elem_result_tup = self.question_mark_blocks(
                        elem_result,
                        checked_dest_inner_ty,
                        checked_dest_inner_ty,
                    );

                    let (elem_size, alignment) = if include_alignment {
                        // `elem_result_tup: (usize, Alignment)`
                        (
                            Operand::Copy(elem_result_tup.project_deeper(
                                &[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)],
                                tcx,
                            )),
                            Some(Operand::Copy(elem_result_tup.project_deeper(
                                &[PlaceElem::Field(FieldIdx::ONE, alignment_struct_ty)],
                                tcx,
                            ))),
                        )
                    } else {
                        // `elem_result_tup: usize`
                        (Operand::Copy(elem_result_tup), None)
                    };

                    // let size = elem_size.checked_mul(len)?;
                    // if size <= isize::MAX as usize {
                    //  Some(..)
                    // } else {
                    //  None
                    // }
                    let sum_tuple = self.make_place(
                        ty::Mutability::Not,
                        Ty::new_tup(tcx, &[tcx.types.usize, tcx.types.bool]),
                    );
                    let sum_value = sum_tuple
                        .project_deeper(&[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)], tcx);
                    let sum_overflow = sum_tuple
                        .project_deeper(&[PlaceElem::Field(FieldIdx::ONE, tcx.types.bool)], tcx);

                    let sum_stmt = self.make_assign(
                        sum_tuple,
                        Rvalue::BinaryOp(BinOp::MulWithOverflow, Box::new((elem_size, len))),
                    );

                    // if overflow { return None }
                    self.block(
                        vec![sum_stmt],
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(sum_overflow),
                            targets: SwitchTargets::static_if(
                                1,
                                self.block_index_offset(1),
                                self.block_index_offset(2),
                            ),
                        },
                        false,
                    );
                    self.return_none_block(checked_dest_inner_ty);

                    // if sum > max { return None }
                    let gt_result = self.make_place(ty::Mutability::Not, tcx.types.bool);
                    let cmp_stmt = self.make_assign(
                        gt_result,
                        Rvalue::BinaryOp(BinOp::Gt, Box::new((Operand::Copy(sum_value), max_size))),
                    );
                    self.block(
                        vec![cmp_stmt],
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(gt_result),
                            targets: SwitchTargets::static_if(
                                1,
                                self.block_index_offset(1),
                                self.block_index_offset(2),
                            ),
                        },
                        false,
                    );
                    self.return_none_block(checked_dest_inner_ty);

                    (Operand::Copy(sum_value), alignment)
                } else {
                    let (elem_size, alignment) = if include_alignment {
                        // `elem_result: (usize, Alignment)`
                        (
                            Operand::Copy(elem_result.project_deeper(
                                &[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)],
                                tcx,
                            )),
                            Some(Operand::Copy(elem_result.project_deeper(
                                &[PlaceElem::Field(FieldIdx::ONE, alignment_struct_ty)],
                                tcx,
                            ))),
                        )
                    } else {
                        // `elem_result: usize`
                        (Operand::Copy(elem_result), None)
                    };

                    let sum_value = self.make_place(ty::Mutability::Not, tcx.types.usize);
                    let sum_stmt = self.make_assign(
                        sum_value,
                        Rvalue::BinaryOp(BinOp::MulUnchecked, Box::new((elem_size, len))),
                    );

                    // assume(sum <= max)
                    let le_result = self.make_place(ty::Mutability::Not, tcx.types.bool);
                    let cmp_stmt = self.make_assign(
                        le_result,
                        Rvalue::BinaryOp(BinOp::Le, Box::new((Operand::Copy(sum_value), max_size))),
                    );

                    let assume_stmt = self.make_statement(StatementKind::Intrinsic(Box::new(
                        NonDivergingIntrinsic::Assume(Operand::Copy(le_result)),
                    )));

                    self.block(
                        vec![sum_stmt, cmp_stmt, assume_stmt],
                        TerminatorKind::Goto { target: self.block_index_offset(1) },
                        false,
                    );

                    (Operand::Copy(sum_value), alignment)
                }
            }
            ty::Dynamic(..) => {
                // The `vtable_size/vtable_align` intrinsics take a `*const ()`.
                let vtable_ptr_ty = Ty::new_ptr(tcx, tcx.types.unit, ty::Mutability::Not);
                let vtable_ptr = self.make_place(ty::Mutability::Not, vtable_ptr_ty);

                let vtable_size = self.make_place(ty::Mutability::Not, tcx.types.usize);
                self.block(
                    vec![self.make_assign(
                        vtable_ptr,
                        // `Metadata<dyn Trait>` is essentially just a vtable ptr.
                        Rvalue::Cast(CastKind::Transmute, Operand::Copy(meta), vtable_ptr_ty),
                    )],
                    TerminatorKind::Call {
                        func: Operand::function_handle(
                            tcx,
                            tcx.require_lang_item(LangItem::VtableSize, self.span),
                            [],
                            self.span,
                        ),
                        args: Box::new([Spanned {
                            node: Operand::Copy(vtable_ptr),
                            span: self.span,
                        }]),
                        destination: vtable_size,
                        target: Some(self.block_index_offset(1)),
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Misc,
                        fn_span: self.span,
                    },
                    false,
                );

                let size = Operand::Copy(vtable_size);

                let align = include_alignment.then(|| {
                    let vtable_align = self.make_place(ty::Mutability::Not, tcx.types.usize);
                    self.block(
                        vec![self.make_assign(
                            vtable_ptr,
                            // `Metadata<dyn Trait>` is essentially just a vtable ptr.
                            Rvalue::Cast(CastKind::Transmute, Operand::Copy(meta), vtable_ptr_ty),
                        )],
                        TerminatorKind::Call {
                            func: Operand::function_handle(
                                tcx,
                                tcx.require_lang_item(LangItem::VtableAlign, self.span),
                                [],
                                self.span,
                            ),
                            args: Box::new([Spanned {
                                node: Operand::Copy(vtable_ptr),
                                span: self.span,
                            }]),
                            destination: vtable_align,
                            target: Some(self.block_index_offset(1)),
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );

                    // `Alignment` is a newtype around a `repr(usize)` enum,
                    // so we can `transmute` a power-of-two `usize` into it.
                    let align = self.make_place(ty::Mutability::Not, alignment_struct_ty);

                    self.block(
                        vec![self.make_assign(
                            align,
                            Rvalue::Cast(
                                CastKind::Transmute,
                                Operand::Copy(vtable_align),
                                alignment_struct_ty,
                            ),
                        )],
                        TerminatorKind::Goto { target: self.block_index_offset(1) },
                        false,
                    );

                    Operand::Copy(align)
                });

                (size, align)
            }
            ty::Pat(_inner_ty, _) => todo!(),
            ty::UnsafeBinder(..) => todo!(),

            ty::Alias(..) | ty::Param(..) | ty::Bound(..) | ty::Placeholder(..) | ty::Infer(..) => {
                bug!("{} should not occur here", self_ty)
            }
        };

        let mut stmts = vec![];
        match (checked, layout_part, alignment) {
            (false, ty::LayoutPart::Size, _) => {
                // Return type is `usize` of just the size
                stmts.push(self.make_assign(dest, Rvalue::Use(size)));
            }
            (true, ty::LayoutPart::Size, _) => {
                // Return type is `Option<usize>` of just the size
                stmts.push(self.make_assign(
                    dest,
                    Rvalue::Aggregate(
                        Box::new(AggregateKind::Adt(
                            option_did,
                            VariantIdx::from_usize(1),
                            tcx.mk_args(&[tcx.types.usize.into()]),
                            None,
                            None,
                        )),
                        [size].into(),
                    ),
                ));
            }
            (false, ty::LayoutPart::Layout, Some(alignment)) => {
                // Return type is `(usize, Alignment)`
                stmts.push(self.make_assign(
                    dest,
                    Rvalue::Aggregate(Box::new(AggregateKind::Tuple), [size, alignment].into()),
                ));
            }
            (true, ty::LayoutPart::Layout, Some(alignment)) => {
                // Return type is `Option<(usize, Alignment)>`, so we need a temp for the tuple.
                let tuple_ty = Ty::new_tup(tcx, &[tcx.types.usize, alignment_struct_ty]);
                let tuple = self.make_place(ty::Mutability::Not, tuple_ty);
                stmts.extend([
                    self.make_assign(
                        tuple,
                        Rvalue::Aggregate(Box::new(AggregateKind::Tuple), [size, alignment].into()),
                    ),
                    self.make_assign(
                        dest,
                        Rvalue::Aggregate(
                            Box::new(AggregateKind::Adt(
                                option_did,
                                VariantIdx::from_usize(1),
                                tcx.mk_args(&[tuple_ty.into()]),
                                None,
                                None,
                            )),
                            [Operand::Move(tuple)].into(),
                        ),
                    ),
                ]);
            }
            (_, ty::LayoutPart::Alignment, _) | (_, ty::LayoutPart::Layout, None) => unreachable!(),
        }
        self.block(stmts, TerminatorKind::Return, false);
    }

    fn struct_like_layout(
        &mut self,
        in_order_fields: impl Iterator<Item = (FieldIdx, Ty<'tcx>)>,
        meta: Place<'tcx>,
        pack: Option<Align>,
        overalign: Option<Align>,
        checked_dest_inner_ty: Ty<'tcx>,
    ) -> (Operand<'tcx>, Operand<'tcx>) {
        let tcx = self.tcx;
        let checked = self.extra.checked;
        let alignment_struct_ty = tcx.ty_alignment_struct(self.span);
        let size_align_tup_ty = Ty::new_tup(tcx, &[tcx.types.usize, alignment_struct_ty]);

        let alignment_max_fn = Operand::function_handle(
            tcx,
            tcx.require_lang_item(LangItem::AlignmentMax, self.span),
            [],
            self.span,
        );
        let alignment_min_fn = Operand::function_handle(
            tcx,
            tcx.require_lang_item(LangItem::AlignmentMin, self.span),
            [],
            self.span,
        );

        let size_acc = self.make_place(ty::Mutability::Mut, tcx.types.usize);
        let align_acc = self.make_place(ty::Mutability::Mut, alignment_struct_ty);
        // FIXME: keep track of statically-sized/aligned fields separately to avoid bloat.

        // We need to get the alignment of all the fields, since we need to get the alignment of the type
        // to round the size up, so we use `(un)checked_layout_for_meta` for the field layout calls,
        // even if this call itself is `*_size_for_meta`.
        let meta_sized =
            tcx.associated_items(tcx.require_lang_item(LangItem::MetaSized, self.span));
        let (field_method_sym, field_ret_ty) = if checked {
            ("checked_layout_for_meta", Ty::new_option(tcx, size_align_tup_ty))
        } else {
            ("unchecked_layout_for_meta", size_align_tup_ty)
        };
        let field_method_def_id = meta_sized
            .filter_by_name_unhygienic(Symbol::intern(field_method_sym))
            .next()
            .unwrap()
            .def_id;

        let mut is_first_field = true;
        for (field_idx, field_ty) in in_order_fields {
            let field_meta_ty = Ty::new_ptr_metadata(tcx, field_ty);
            let field_meta =
                meta.project_deeper(&[PlaceElem::Field(field_idx, field_meta_ty)], tcx);

            let field_layout_ret = self.make_place(ty::Mutability::Not, field_ret_ty);

            // bb:
            //  field_layout_ret = <FIELDTY as MetaSized>::(un)checked_layout_for_meta(Copy meta.FIELDIDX) [return -> nextbb, unwind continue]
            // nextbb: ...
            self.block(
                vec![],
                TerminatorKind::Call {
                    func: Operand::function_handle(
                        self.tcx,
                        field_method_def_id,
                        [ty::GenericArg::from(field_ty)],
                        self.span,
                    ),
                    args: Box::new([Spanned { node: Operand::Copy(field_meta), span: self.span }]),
                    destination: field_layout_ret,
                    target: Some(self.block_index_offset(1)),
                    // `UnwindAction::Continue` is fine since layout computation shims never have any locals with drop glue,
                    // only `ptr::Metadata<_>`, `Alignment`, `usize`, and tuple or `Option`.
                    unwind: UnwindAction::Continue,
                    call_source: CallSource::Misc,
                    fn_span: self.span,
                },
                false,
            );

            let field_layout = if checked {
                self.question_mark_blocks(
                    field_layout_ret,
                    size_align_tup_ty,
                    checked_dest_inner_ty,
                )
            } else {
                field_layout_ret
            };

            let field_size = field_layout
                .project_deeper(&[PlaceElem::Field(FieldIdx::ZERO, tcx.types.usize)], tcx);
            let field_align = field_layout
                .project_deeper(&[PlaceElem::Field(FieldIdx::ONE, alignment_struct_ty)], tcx);

            if is_first_field {
                is_first_field = false;
                // First field, just set the accumulators to the field layout.
                // Don't need to do any checking or rounding.
                self.block(
                    vec![
                        self.make_assign(size_acc, Rvalue::Use(Operand::Copy(field_size))),
                        self.make_assign(align_acc, Rvalue::Use(Operand::Copy(field_align))),
                    ],
                    TerminatorKind::Goto { target: self.block_index_offset(1) },
                    false,
                );
            } else {
                // Round the current size up to the field's effective alignment,
                // then add the field's size.
                //
                // sizecmpbb:
                //  _size_gt_result = field_size > size_acc;
                //  switchInt(_size_gt_result) [1 => setbb, 2 => alignbb];
                // sizesetbb:
                //  size_acc = field_size;
                //  goto -> nextbb;
                // alignbb:
                //  align_acc = Alignment::max(align_acc, field_align) [return -> nextbb, unwind continue];
                // nextbb: ...

                let effective_field_align = if let Some(pack) = pack {
                    if pack.bytes() == 1 {
                        Operand::const_from_scalar(
                            self.tcx,
                            self.tcx.types.usize,
                            interpret::Scalar::from_target_usize(1, &tcx),
                            self.span,
                        )
                    } else {
                        let clamped_field_align =
                            self.make_place(ty::Mutability::Not, alignment_struct_ty);
                        self.block(
                            vec![],
                            TerminatorKind::Call {
                                func: alignment_min_fn.clone(),
                                args: Box::new([
                                    Spanned { node: Operand::Copy(field_align), span: self.span },
                                    Spanned {
                                        node: Operand::const_from_scalar(
                                            self.tcx,
                                            alignment_struct_ty,
                                            interpret::Scalar::from_target_usize(
                                                pack.bytes(),
                                                &tcx,
                                            ),
                                            self.span,
                                        ),
                                        span: self.span,
                                    },
                                ]),
                                destination: clamped_field_align,
                                target: Some(self.block_index_offset(1)),
                                unwind: UnwindAction::Continue,
                                call_source: CallSource::Misc,
                                fn_span: self.span,
                            },
                            false,
                        );
                        Operand::Copy(clamped_field_align)
                    }
                } else {
                    Operand::Copy(field_align)
                };

                let field_offset = self.round_size_up_to_alignment(
                    size_acc,
                    effective_field_align,
                    checked_dest_inner_ty,
                );

                let field_end_offset = self.add_sizes(
                    Operand::Copy(field_offset),
                    Operand::Copy(field_size),
                    checked_dest_inner_ty,
                );
                self.block(
                    vec![self.make_assign(size_acc, Rvalue::Use(Operand::Copy(field_end_offset)))],
                    TerminatorKind::Goto { target: self.block_index_offset(1) },
                    false,
                );

                // alignbb:
                self.block(
                    vec![],
                    TerminatorKind::Call {
                        func: alignment_max_fn.clone(),
                        args: Box::new([
                            Spanned { node: Operand::Copy(align_acc), span: self.span },
                            Spanned { node: Operand::Copy(field_align), span: self.span },
                        ]),
                        destination: align_acc,
                        target: Some(self.block_index_offset(1)),
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Misc,
                        fn_span: self.span,
                    },
                    false,
                );
            }
        }

        // Clamp alignment to `repr(packed)`
        if let Some(pack) = pack {
            // alignbb:
            self.block(
                vec![],
                TerminatorKind::Call {
                    func: alignment_min_fn.clone(),
                    args: Box::new([
                        Spanned { node: Operand::Copy(align_acc), span: self.span },
                        Spanned {
                            node: Operand::const_from_scalar(
                                self.tcx,
                                alignment_struct_ty,
                                interpret::Scalar::from_target_usize(pack.bytes(), &tcx),
                                self.span,
                            ),
                            span: self.span,
                        },
                    ]),
                    destination: align_acc,
                    target: Some(self.block_index_offset(1)),
                    unwind: UnwindAction::Continue,
                    call_source: CallSource::Misc,
                    fn_span: self.span,
                },
                false,
            );
        }

        // Raise alignment to `repr(align)`
        if let Some(overalign) = overalign {
            // alignbb:
            self.block(
                vec![],
                TerminatorKind::Call {
                    func: alignment_max_fn.clone(),
                    args: Box::new([
                        Spanned { node: Operand::Copy(align_acc), span: self.span },
                        Spanned {
                            node: Operand::const_from_scalar(
                                self.tcx,
                                alignment_struct_ty,
                                interpret::Scalar::from_target_usize(overalign.bytes(), &tcx),
                                self.span,
                            ),
                            span: self.span,
                        },
                    ]),
                    destination: align_acc,
                    target: Some(self.block_index_offset(1)),
                    unwind: UnwindAction::Continue,
                    call_source: CallSource::Misc,
                    fn_span: self.span,
                },
                false,
            );
        }

        let full_size = self.round_size_up_to_alignment(
            size_acc,
            Operand::Copy(align_acc),
            checked_dest_inner_ty,
        );
        (Operand::Copy(full_size), Operand::Copy(align_acc))
    }

    /// `size` must be `<= isize::MAX`, and `alignment` must be a `mem::Alignment`.
    /// If the rounded-up value is `> isize::MAX` in `checked` mode, `None::<dest_inner_ty>` will be returned.
    /// In unchecked mode, there will be an `assume(value <= isize::MAX)`.
    fn round_size_up_to_alignment(
        &mut self,
        unrounded_size: Place<'tcx>,
        alignment: Operand<'tcx>,
        checked_dest_inner_ty: Ty<'tcx>,
    ) -> Place<'tcx> {
        let tcx = self.tcx;
        let checked = self.extra.checked;

        // Round size up to alignment, and check if that caused it to go over `isize::MAX`.
        // `align_minus_one = align - 1;`
        // `align_mask = !align_minus_one;`
        // `size = (size + align_minus_one) & align_mask;`
        // This can't cause unsigned overflow even intermediately
        // * since the alignment is at least 1, the subtraction can't overflow.
        // * the sum is at most `isize::MAX` (highest alignment - 1) + isize::MAX (highest size) < usize::MAX.
        // * bitops cannot overflow.
        // We still need to check that the result is `<= isize::MAX`, since it could have become `isize::MAX + 1` (at most)

        let mut stmts = vec![];

        let align_usize = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(self.make_assign(
            align_usize,
            Rvalue::Cast(CastKind::Transmute, alignment, tcx.types.usize),
        ));

        let align_minus_one = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(self.make_assign(
            align_minus_one,
            Rvalue::BinaryOp(
                BinOp::SubUnchecked,
                Box::new((
                    Operand::Copy(align_usize),
                    Operand::const_from_scalar(
                        tcx,
                        tcx.types.usize,
                        interpret::Scalar::from_target_usize(1, &tcx),
                        self.span,
                    ),
                )),
            ),
        ));

        let align_mask = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(
            self.make_assign(
                align_mask,
                Rvalue::UnaryOp(UnOp::Not, Operand::Copy(align_minus_one)),
            ),
        );

        let size_plus_align_minus_one = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(self.make_assign(
            size_plus_align_minus_one,
            Rvalue::BinaryOp(
                BinOp::AddUnchecked,
                Box::new((Operand::Copy(unrounded_size), Operand::Copy(align_minus_one))),
            ),
        ));

        let full_size = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(self.make_assign(
            full_size,
            Rvalue::BinaryOp(
                BinOp::BitAnd,
                Box::new((Operand::Copy(size_plus_align_minus_one), Operand::Copy(align_mask))),
            ),
        ));

        let max_size = Operand::const_from_scalar(
            tcx,
            tcx.types.usize,
            interpret::Scalar::from_target_usize(tcx.max_size_of_val().bytes(), &tcx),
            self.span,
        );
        // le_result = full_size <= max;
        let le_result = self.make_place(ty::Mutability::Not, tcx.types.bool);
        stmts.push(self.make_assign(
            le_result,
            Rvalue::BinaryOp(BinOp::Le, Box::new((Operand::Copy(full_size), max_size))),
        ));

        if checked {
            // if !(full_size <= max) { return None }
            self.block(
                stmts,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(le_result),
                    targets: SwitchTargets::static_if(
                        0,
                        self.block_index_offset(1),
                        self.block_index_offset(2),
                    ),
                },
                false,
            );
            self.return_none_block(checked_dest_inner_ty);
        } else {
            // assume(full_size <= max);
            stmts.push(self.make_statement(StatementKind::Intrinsic(Box::new(
                NonDivergingIntrinsic::Assume(Operand::Copy(le_result)),
            ))));
            self.block(stmts, TerminatorKind::Goto { target: self.block_index_offset(1) }, false);
        }

        full_size
    }

    /// Both inputs must be `<= isize::MAX`, so there can never be any unsigned overflow.
    /// If the sum is `> isize::MAX` in `checked` mode, `None::<dest_inner_ty>` will be returned.
    /// In unchecked mode, there will be an `assume(full_sum <= isize::MAX)`.
    fn add_sizes(
        &mut self,
        lhs: Operand<'tcx>,
        rhs: Operand<'tcx>,
        checked_dest_inner_ty: Ty<'tcx>,
    ) -> Place<'tcx> {
        let tcx = self.tcx;
        let checked = self.extra.checked;

        let mut stmts = vec![];

        let sum = self.make_place(ty::Mutability::Not, tcx.types.usize);
        stmts.push(
            self.make_assign(sum, Rvalue::BinaryOp(BinOp::AddUnchecked, Box::new((lhs, rhs)))),
        );

        let max_size = Operand::const_from_scalar(
            tcx,
            tcx.types.usize,
            interpret::Scalar::from_target_usize(tcx.max_size_of_val().bytes(), &tcx),
            self.span,
        );
        // le_result = full_size <= max;
        let le_result = self.make_place(ty::Mutability::Not, tcx.types.bool);
        stmts.push(self.make_assign(
            le_result,
            Rvalue::BinaryOp(BinOp::Le, Box::new((Operand::Copy(sum), max_size))),
        ));

        if checked {
            // if !(full_size <= max) { return None }
            self.block(
                stmts,
                TerminatorKind::SwitchInt {
                    discr: Operand::Copy(le_result),
                    targets: SwitchTargets::static_if(
                        0,
                        self.block_index_offset(1),
                        self.block_index_offset(2),
                    ),
                },
                false,
            );
            self.return_none_block(checked_dest_inner_ty);
        } else {
            // assume(full_size <= max);
            stmts.push(self.make_statement(StatementKind::Intrinsic(Box::new(
                NonDivergingIntrinsic::Assume(Operand::Copy(le_result)),
            ))));
            self.block(stmts, TerminatorKind::Goto { target: self.block_index_offset(1) }, false);
        }

        sum
    }

    fn only_alignment_shim(&mut self) {
        let LayoutForMetaShimExtra { method_def_id, self_ty, .. } = self.extra;
        let tcx = self.tcx;
        let typing_env = ty::TypingEnv::fully_monomorphized();
        let alignment_struct_ty = tcx.ty_alignment_struct(self.span);

        let dest = Place::return_place();
        let dest_ty = dest.ty(&self.local_decls, tcx).ty;
        let meta = Place::from(Local::new(1 + 0));

        let layout = match tcx.layout_of(typing_env.as_query_input(self_ty)) {
            Ok(layout) => layout,
            Err(err) => {
                // If `self_ty` doesn't have a valid layout, then there should already have been an error,
                // but we still need to emit some MIR.
                tcx.dcx().delayed_bug(format!(
                    "layout_for_meta shim for type with invalid layout: {err:?}"
                ));
                self.block(vec![], TerminatorKind::Unreachable, false);
                return;
            }
        };

        if self_ty.is_aligned(tcx, typing_env) {
            // If `Self: Aligned`, then `layout.align` is accurate, and we can just return it.
            // The return type is either `Alignment` (which is a newtype around a `repr(usize)` enum),
            // or `Option<Alignment>` (which is niche-optimized), so a nonzero-`usize`-valued
            // scalar constant is valid
            let alignment = Operand::const_from_scalar(
                self.tcx,
                dest_ty,
                interpret::Scalar::from_target_usize(layout.align.abi.bytes(), &tcx),
                self.span,
            );
            let stmt = self.make_assign(dest, Rvalue::Use(alignment));
            self.block(vec![stmt], TerminatorKind::Return, false);
            return;
        }

        let alignment = match self_ty.kind() {
            ty::Bool
            | ty::Char
            | ty::Int(_)
            | ty::Uint(_)
            | ty::Float(_)
            | ty::FnDef(..)
            | ty::FnPtr(..)
            | ty::Closure(..)
            | ty::CoroutineClosure(..)
            | ty::Coroutine(..)
            | ty::CoroutineWitness(..)
            | ty::Never
            | ty::UntypedPtr { .. }
            | ty::PtrMetadata(..)
            | ty::RawPtr(..)
            | ty::Ref(..)
            | ty::InitAdt(..)
            | ty::InitArray(..)
            | ty::InitArrayRepeat(..)
            | ty::InitSliceRepeat(..)
            | ty::InitTuple(..)
            | ty::Str
            | ty::Error(_) => bug!("{} should be `Aligned`", self_ty),
            ty::Foreign(..) => bug!("{} should not be `MetaAligned`", self_ty),

            ty::Adt(def, ..) if def.is_unsized_type() => bug!(
                "AdtKind::UnsizedType should have manual MetaSized/MetaAligned impls, not builtin"
            ),
            ty::Adt(def, ..) if def.is_enum() => todo!("unsized enums"),
            ty::Adt(def, args) => 'adt_alignment: {
                // Alignment of a struct/union is the max of the fields' alignments, clamped if `repr(packed)`.
                // There will always be at least one field with dynamic alignment, as otherwise the whole ADT
                // would be `Aligned`.

                let pack = def.repr().pack;
                if pack == Some(Align::ONE) {
                    // The alignment of a `repr(packed(1))` ADT is always 1, so we can just return that.
                    // The return type is either `Alignment` (which is a newtype around a `repr(usize)` enum),
                    // or `Option<Alignment>` (which is niche-optimized), so a nonzero-`usize`-valued
                    // scalar constant is valid
                    break 'adt_alignment Rvalue::Use(Operand::const_from_scalar(
                        self.tcx,
                        dest_ty,
                        interpret::Scalar::from_target_usize(1, &tcx),
                        self.span,
                    ));
                }

                let alignment_max_fn = Operand::function_handle(
                    tcx,
                    tcx.require_lang_item(LangItem::AlignmentMax, self.span),
                    [],
                    self.span,
                );
                let alignment_min_fn = Operand::function_handle(
                    tcx,
                    tcx.require_lang_item(LangItem::AlignmentMin, self.span),
                    [],
                    self.span,
                );

                let dyn_acc = self.make_place(ty::Mutability::Not, alignment_struct_ty);
                let mut first_dyn = true;
                let mut static_acc = def.repr().align.unwrap_or(Align::ONE);

                for (field_idx, field) in def.non_enum_variant().fields.iter_enumerated() {
                    let field_ty = field.ty(tcx, args);
                    let field_dynamic_alignment = match self.field_align(
                        field_ty,
                        meta.project_deeper(
                            &[PlaceElem::Field(field_idx, Ty::new_ptr_metadata(tcx, field_ty))],
                            tcx,
                        ),
                    ) {
                        Either::Left(dynamic_alignment) => dynamic_alignment,
                        Either::Right(static_alignment) => {
                            static_acc = Align::max(static_acc, static_alignment);
                            continue;
                        }
                    };
                    if first_dyn {
                        // For the first dynamically-aligned field, just do
                        // _dyn_acc = field_dynamic_alignment;
                        first_dyn = false;
                        self.block(
                            vec![self.make_assign(dyn_acc, Rvalue::Use(field_dynamic_alignment))],
                            TerminatorKind::Goto { target: self.block_index_offset(1) },
                            false,
                        );
                    } else {
                        // For later fields, do
                        // _dyn_acc = Alignment::max(_dyn_acc, field_dynamic_alignment) [return -> next, unwind continue]
                        self.block(
                            vec![],
                            TerminatorKind::Call {
                                func: alignment_max_fn.clone(),
                                args: Box::new([
                                    Spanned { node: Operand::Copy(dyn_acc), span: self.span },
                                    Spanned { node: field_dynamic_alignment, span: self.span },
                                ]),
                                target: Some(self.block_index_offset(1)),
                                destination: dyn_acc,
                                unwind: UnwindAction::Continue,
                                call_source: CallSource::Misc,
                                fn_span: self.span,
                            },
                            false,
                        );
                    }
                }
                debug_assert!(
                    !first_dyn,
                    "non-`Aligned` struct/union should have at least one non-`Aligned` field"
                );

                // Raise to `max(repr(align), max(field_aligns))`
                if static_acc > Align::ONE {
                    // _dyn_acc = Alignment::max(_dyn_acc, max_static_field_alignment_or_repr_align) [return -> next, unwind continue]
                    self.block(
                        vec![],
                        TerminatorKind::Call {
                            func: alignment_max_fn,
                            args: Box::new([
                                Spanned { node: Operand::Copy(dyn_acc), span: self.span },
                                Spanned {
                                    node: Operand::const_from_scalar(
                                        self.tcx,
                                        alignment_struct_ty,
                                        interpret::Scalar::from_target_usize(
                                            static_acc.bytes(),
                                            &tcx,
                                        ),
                                        self.span,
                                    ),
                                    span: self.span,
                                },
                            ]),
                            target: Some(self.block_index_offset(1)),
                            destination: dyn_acc,
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );
                }

                if let Some(pack) = pack {
                    // _dyn_acc = Alignment::min(_dyn_acc, pack_alignment) [return -> next, unwind continue]
                    self.block(
                        vec![],
                        TerminatorKind::Call {
                            func: alignment_min_fn,
                            args: Box::new([
                                Spanned { node: Operand::Copy(dyn_acc), span: self.span },
                                Spanned {
                                    node: Operand::const_from_scalar(
                                        self.tcx,
                                        alignment_struct_ty,
                                        interpret::Scalar::from_target_usize(pack.bytes(), &tcx),
                                        self.span,
                                    ),
                                    span: self.span,
                                },
                            ]),
                            target: Some(self.block_index_offset(1)),
                            destination: dyn_acc,
                            unwind: UnwindAction::Continue,
                            call_source: CallSource::Misc,
                            fn_span: self.span,
                        },
                        false,
                    );
                }

                // The return type is either `Alignment` (which is a newtype around a `repr(usize)` enum),
                // or `Option<Alignment>` (which is niche-optimized), so we can `transmute` an `Alignment`
                // into it.
                Rvalue::Cast(CastKind::Transmute, Operand::Copy(dyn_acc), dest_ty)
            }

            &ty::Slice(elem_ty) | &ty::Array(elem_ty, _) => {
                // The alignment of a slice or array is the alignment of the element, so just do
                // (effectively) a tail call.

                let elem_meta_ty = Ty::new_ptr_metadata(tcx, elem_ty);
                let elem_meta_idx = match self_ty.kind() {
                    ty::Slice(..) => FieldIdx::ONE,
                    ty::Array(..) => FieldIdx::ZERO,
                    _ => unreachable!(),
                };
                let elem_meta =
                    meta.project_deeper(&[PlaceElem::Field(elem_meta_idx, elem_meta_ty)], tcx);

                self.block(
                    vec![],
                    TerminatorKind::Call {
                        func: Operand::function_handle(
                            self.tcx,
                            method_def_id,
                            [ty::GenericArg::from(elem_ty)],
                            self.span,
                        ),
                        args: Box::new([Spanned {
                            node: Operand::Copy(elem_meta),
                            span: self.span,
                        }]),
                        destination: Place::return_place(),
                        target: Some(self.block_index_offset(1)),
                        // `UnwindAction::Continue` is fine since layout computation shims never have any locals with drop glue,
                        // only `ptr::Metadata<_>`, `Alignment`, `usize`, and tuple or `Option`.
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Misc,
                        fn_span: self.span,
                    },
                    false,
                );
                self.block(vec![], TerminatorKind::Return, false);
                return;
            }
            ty::Dynamic(..) => {
                let vtable_align = self.make_place(ty::Mutability::Not, tcx.types.usize);
                // The `vtable_size` intrinsic takes a `*const ()`.
                let vtable_ptr_ty = Ty::new_ptr(tcx, tcx.types.unit, ty::Mutability::Not);
                let vtable_ptr = self.make_place(ty::Mutability::Not, vtable_ptr_ty);
                self.block(
                    vec![self.make_assign(
                        vtable_ptr,
                        // `Metadata<dyn Trait>` is essentially just a vtable ptr.
                        Rvalue::Cast(CastKind::Transmute, Operand::Copy(meta), vtable_ptr_ty),
                    )],
                    TerminatorKind::Call {
                        func: Operand::function_handle(
                            tcx,
                            tcx.require_lang_item(LangItem::VtableAlign, self.span),
                            [],
                            self.span,
                        ),
                        args: Box::new([Spanned {
                            node: Operand::Copy(vtable_ptr),
                            span: self.span,
                        }]),
                        destination: vtable_align,
                        target: Some(self.block_index_offset(1)),
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Misc,
                        fn_span: self.span,
                    },
                    false,
                );

                // The return type is either `Alignment` (which is a newtype around a `repr(usize)` enum),
                // or `Option<Alignment>` (which is niche-optimized), so we can `transmute` a power-of-two
                // `usize` into it.
                Rvalue::Cast(CastKind::Transmute, Operand::Copy(vtable_align), dest_ty)
            }

            ty::Tuple(..) => todo!(),
            ty::Pat(_inner_ty, _) => todo!(),
            ty::UnsafeBinder(..) => todo!(),

            ty::Alias(..) | ty::Param(..) | ty::Bound(..) | ty::Placeholder(..) | ty::Infer(..) => {
                bug!("{} should not occur here", self_ty)
            }
        };
        let stmt = self.make_assign(dest, alignment);
        self.block(vec![stmt], TerminatorKind::Return, false);
    }

    /// Given a `Place` of type `Option<scrutinee_inner_ty>`, perform `?` on it
    /// in a function returning `Option<dest_inner_ty>`, and return the projected
    /// `Some.0` place.
    ///
    /// ```text
    /// bb:
    ///  _discr = discriminant(scrutinee);
    ///  switchInt(_discr) [0 -> returnbb, otherwise -> nextbb]
    /// returnbb:
    ///  _0 = None::<dest_inner_ty>;
    ///  return
    /// nextbb: ... (not added)
    /// ```
    ///
    fn question_mark_blocks(
        &mut self,
        scrutinee: Place<'tcx>,
        scrutinee_inner_ty: Ty<'tcx>,
        dest_inner_ty: Ty<'tcx>,
    ) -> Place<'tcx> {
        let tcx = self.tcx;
        let scrutinee_ty = scrutinee.ty(&self.local_decls, tcx).ty;

        let discr_place = self.make_place(ty::Mutability::Not, scrutinee_ty.discriminant_ty(tcx));

        let discr_stmt = self.make_assign(discr_place, Rvalue::Discriminant(scrutinee));
        self.block(
            vec![discr_stmt],
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(discr_place),
                targets: SwitchTargets::static_if(
                    // 0 -> None -> returnbb
                    // 1 -> Some -> nextbb
                    0,
                    self.block_index_offset(1),
                    self.block_index_offset(2),
                ),
            },
            false,
        );

        self.return_none_block(dest_inner_ty);

        scrutinee.project_deeper(
            &[
                PlaceElem::Downcast(None, VariantIdx::from_usize(1)),
                PlaceElem::Field(FieldIdx::ZERO, scrutinee_inner_ty),
            ],
            tcx,
        )
    }

    fn return_none_block(&mut self, none_inner_ty: Ty<'tcx>) {
        let tcx = self.tcx;

        let assign_none_stmt = self.make_assign(
            Place::return_place(),
            Rvalue::Aggregate(
                Box::new(AggregateKind::Adt(
                    tcx.require_lang_item(LangItem::Option, self.span),
                    VariantIdx::ZERO,
                    tcx.mk_args(&[none_inner_ty.into()]),
                    None,
                    None,
                )),
                [].into(),
            ),
        );
        self.block(vec![assign_none_stmt], TerminatorKind::Return, false);
    }

    /// Returns an `Operand` that represents the alignment of `ty` with `meta`,
    /// adding a new block calling `MetaAligned::(un)checked_align_for_meta` if necessary.
    ///
    /// If this is a `checked` operation, also handles the `?`.
    ///
    /// Should only be used in `only_alignment_shim`.
    fn field_align(&mut self, ty: Ty<'tcx>, meta: Place<'tcx>) -> Either<Operand<'tcx>, Align> {
        let LayoutForMetaShimExtra { method_def_id, checked, .. } = self.extra;
        let tcx = self.tcx;
        let typing_env = ty::TypingEnv::fully_monomorphized();
        let alignment_struct_ty = tcx.ty_alignment_struct(self.span);

        if ty.is_aligned(tcx, typing_env) {
            // If `T: Aligned`, then `layout.align` is accurate, and we can just return it
            // as `Alignment` (which is a newtype around a `repr(usize)` enum),
            // so a `usize`-valued scalar constant is valid.
            let layout = tcx
                .layout_of(typing_env.as_query_input(ty))
                .expect("type is as a field of a type with a valid layout");
            return Either::Right(layout.align.abi);
        }

        let ret_ty =
            if checked { Ty::new_option(tcx, alignment_struct_ty) } else { alignment_struct_ty };

        let field_align_ret = self.make_place(ty::Mutability::Not, ret_ty);

        // bb:
        //  field_align_ret = <T as MetaAligned>::(un)checked_align_for_meta(Copy meta) [return -> nextbb, unwind continue]
        // nextbb: ...
        self.block(
            vec![],
            TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    method_def_id,
                    [ty::GenericArg::from(ty)],
                    self.span,
                ),
                args: Box::new([Spanned { node: Operand::Copy(meta), span: self.span }]),
                destination: field_align_ret,
                target: Some(self.block_index_offset(1)),
                // `UnwindAction::Continue` is fine since layout computation shims never have any locals with drop glue,
                // only `ptr::Metadata<_>`, `Alignment`, `usize`, and tuple or `Option`.
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: self.span,
            },
            false,
        );

        if checked {
            // bb:
            //  _discr = discriminant(field_align_ret);
            //  switchInt _discr [None -> returnbb, Some(_) -> nextbb]
            // returnbb:
            //  _0 = None
            //  return
            // nextbb: ...
            //
            // and return (field_align_ret as Some).0: Alignment

            let field_align_ret = self.question_mark_blocks(
                field_align_ret,
                alignment_struct_ty,
                alignment_struct_ty,
            );

            Either::Left(Operand::Copy(field_align_ret))
        } else {
            Either::Left(Operand::Copy(field_align_ret))
        }
    }

    #[cfg(false)]
    fn clone_fields<I>(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        target: BasicBlock,
        mut unwind: BasicBlock,
        tys: I,
    ) -> BasicBlock
    where
        I: IntoIterator<Item = Ty<'tcx>>,
    {
        // For an iterator of length n, create 2*n + 1 blocks.
        for (i, ity) in tys.into_iter().enumerate() {
            // Each iteration creates two blocks, referred to here as block 2*i and block 2*i + 1.
            //
            // Block 2*i attempts to clone the field. If successful it branches to 2*i + 2 (the
            // next clone block). If unsuccessful it branches to the previous unwind block, which
            // is initially the `unwind` argument passed to this function.
            //
            // Block 2*i + 1 is the unwind block for this iteration. It drops the cloned value
            // created by block 2*i. We store this block in `unwind` so that the next clone block
            // will unwind to it if cloning fails.

            let field = FieldIdx::new(i);
            let src_field = self.tcx.mk_place_field(src, field, ity);

            let dest_field = self.tcx.mk_place_field(dest, field, ity);

            let next_unwind = self.block_index_offset(1);
            let next_block = self.block_index_offset(2);
            self.make_clone_call(dest_field, src_field, ity, next_block, unwind);
            self.block(
                vec![],
                TerminatorKind::Drop {
                    place: dest_field,
                    target: unwind,
                    unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
                    replace: false,
                    drop: None,
                    async_fut: None,
                },
                /* is_cleanup */ true,
            );
            unwind = next_unwind;
        }
        // If all clones succeed then we end up here.
        self.block(vec![], TerminatorKind::Goto { target }, false);
        unwind
    }

    #[cfg(false)]
    fn tuple_like_shim<I>(&mut self, dest: Place<'tcx>, src: Place<'tcx>, tys: I)
    where
        I: IntoIterator<Item = Ty<'tcx>>,
    {
        self.block(vec![], TerminatorKind::Goto { target: self.block_index_offset(3) }, false);
        let unwind = self.block(vec![], TerminatorKind::UnwindResume, true);
        let target = self.block(vec![], TerminatorKind::Return, false);

        let _final_cleanup_block = self.clone_fields(dest, src, target, unwind, tys);
    }

    #[cfg(false)]
    fn coroutine_shim(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        coroutine_def_id: DefId,
        args: CoroutineArgs<TyCtxt<'tcx>>,
    ) {
        self.block(vec![], TerminatorKind::Goto { target: self.block_index_offset(3) }, false);
        let unwind = self.block(vec![], TerminatorKind::UnwindResume, true);
        // This will get overwritten with a switch once we know the target blocks
        let switch = self.block(vec![], TerminatorKind::Unreachable, false);
        let unwind = self.clone_fields(dest, src, switch, unwind, args.upvar_tys());
        let target = self.block(vec![], TerminatorKind::Return, false);
        let unreachable = self.block(vec![], TerminatorKind::Unreachable, false);
        let mut cases = Vec::with_capacity(args.state_tys(coroutine_def_id, self.tcx).count());
        for (index, state_tys) in args.state_tys(coroutine_def_id, self.tcx).enumerate() {
            let variant_index = VariantIdx::new(index);
            let dest = self.tcx.mk_place_downcast_unnamed(dest, variant_index);
            let src = self.tcx.mk_place_downcast_unnamed(src, variant_index);
            let clone_block = self.block_index_offset(1);
            let start_block = self.block(
                vec![self.make_statement(StatementKind::SetDiscriminant {
                    place: Box::new(Place::return_place()),
                    variant_index,
                })],
                TerminatorKind::Goto { target: clone_block },
                false,
            );
            cases.push((index as u128, start_block));
            let _final_cleanup_block = self.clone_fields(dest, src, target, unwind, state_tys);
        }
        let discr_ty = args.discr_ty(self.tcx);
        let temp = self.make_place(Mutability::Mut, discr_ty);
        let rvalue = Rvalue::Discriminant(src);
        let statement = self.make_assign(temp, rvalue);
        match &mut self.blocks[switch] {
            BasicBlockData { statements, terminator: Some(Terminator { kind, .. }), .. } => {
                statements.push(statement);
                *kind = TerminatorKind::SwitchInt {
                    discr: Operand::Move(temp),
                    targets: SwitchTargets::new(cases.into_iter(), unreachable),
                };
            }
            BasicBlockData { terminator: None, .. } => unreachable!(),
        }
    }
}

/// Builds a `Ord::cmp` shim for `builtin # ptr_metadata(pointee_ty)`. Here, `def_id` is `Ord::cmp`.
fn build_ptr_metadata_cmp_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
) -> Body<'tcx> {
    let ty::InstanceKind::PtrMetadataCmpShim(def_id, pointee_ty) = instance else { unreachable!() };
    debug!("build_ptr_metadata_cmp_shim(def_id={:?})", def_id);

    let typing_env = ty::TypingEnv::post_analysis(tcx, def_id);
    let fields = match pointee_ty.metadata_fields_for_pointee(tcx, Some(typing_env)) {
        MetadataFields::KnownFields(fields) => fields,
        fields => bug!(
            "ptr_metadata cmp shim for `{:?}` which is not monomorphic enough ({fields:?})",
            pointee_ty
        ),
    };

    let fields_to_compare: Vec<_> = fields
        .iter()
        .zip(FieldIdx::ZERO..)
        .filter_map(|((_, _, _, field_ty), field_idx)| match field_ty.kind() {
            ty::PtrMetadata(field_pointee_ty) if field_pointee_ty.is_thin(tcx, typing_env) => None,
            _ => Some((field_ty, field_idx)),
        })
        .collect();

    let mut builder = PtrMetadataCmpShimBuilder::new(tcx, instance, def_id, pointee_ty);

    let dest = Place::return_place();
    let lhs = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
    let rhs = tcx.mk_place_deref(Place::from(Local::new(2 + 0)));

    builder.compare_fields(dest, lhs, rhs, &fields_to_compare);

    builder.into_mir()
}

struct PtrMetadataCmpShimExtra;
type PtrMetadataCmpShimBuilder<'tcx> = ShimBuilder<'tcx, PtrMetadataCmpShimExtra>;

impl<'tcx> PtrMetadataCmpShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        def_id: DefId,
        pointee_ty: Ty<'tcx>,
    ) -> Self {
        let sig =
            tcx.fn_sig(def_id).instantiate(tcx, &[Ty::new_ptr_metadata(tcx, pointee_ty).into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(def_id);

        PtrMetadataCmpShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: PtrMetadataCmpShimExtra,
        }
    }

    fn compare_fields(
        &mut self,
        dest: Place<'tcx>,
        lhs: Place<'tcx>,
        rhs: Place<'tcx>,
        fields: &[(Ty<'tcx>, FieldIdx)],
    ) {
        let ordering_enum = self.tcx.ty_ordering_enum(self.span);

        let [prefix_fields @ .., last_field] = fields else {
            // Thin pointees' metadatas are trivially always equal.

            let equal_const = Operand::const_from_scalar(
                self.tcx,
                ordering_enum,
                interpret::Scalar::from_i8(0),
                self.span,
            );

            let retval_stmt = self.make_assign(dest, Rvalue::Use(equal_const));

            self.block(vec![retval_stmt], TerminatorKind::Return, false);
            return;
        };

        // For n fields, create 3*(n-1)+2 blocks.

        // For the (n-1) prefix fields:
        // bb0:
        //  _lhs_field = &(*_lhs).field_idx;
        //  _rhs_field = &(*_lhs).field_idx;
        //  _tmp_ordering = Call(<field_ty as Ord>::cmp, _lhs_field, _rhs_field) [return -> bb1, unwind resume]
        // bb1:
        //  _tmp = discriminant(_tmp_ordering);
        //  switchInt(_tmp) -> [0: bb3, otherwise: bb2]
        // bb2:
        //  _0 = copy _tmp_ordering;
        //  return;
        // bb3: (other prefix fields)
        //  ...
        // bb(3*(n-1)):
        //  _lhs_field = &(*_lhs).field_idx;
        //  _rhs_field = &(*_lhs).field_idx;
        //  _0 = Call(<field_ty as Ord>::cmp, _lhs_field, _rhs_field) [return -> return_block, unwind resume]
        // return_block = bb(3*(n-1)+1):
        //  return;

        let cmp_method = self.tcx.require_lang_item(LangItem::OrdCmp, self.span);

        for &(field_ty, field_idx) in prefix_fields {
            let field_ref_ty = Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, field_ty);
            let lhs_ref = self.make_place(Mutability::Not, field_ref_ty);
            let rhs_ref = self.make_place(Mutability::Not, field_ref_ty);
            let tmp_ordering = self.make_place(Mutability::Not, ordering_enum);
            let tmp_discriminant = self.make_place(Mutability::Not, self.tcx.types.i8);

            // `let lhs_ref: &ty = &(*lhs).field_idx;`
            let lhs_ref_stmt = self.make_assign(
                lhs_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    lhs.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );
            // `let rhs_ref: &ty = &(*rhs).field_idx;`
            let rhs_ref_stmt = self.make_assign(
                rhs_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    rhs.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );

            let discriminant_check_block = self.block_index_offset(1);
            let return_this_ordering_block = self.block_index_offset(2);
            let next_compare_block = self.block_index_offset(3);

            let args = [lhs_ref, rhs_ref]
                .into_iter()
                .map(|p| Spanned { node: Operand::Copy(p), span: DUMMY_SP })
                .collect();
            let cmp_call = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    cmp_method,
                    [ty::GenericArg::from(field_ty)],
                    self.span,
                ),
                args,
                destination: tmp_ordering,
                target: Some(discriminant_check_block),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: DUMMY_SP,
            };

            self.block(vec![lhs_ref_stmt, rhs_ref_stmt], cmp_call, false);

            let tmp_discriminant_stmt =
                self.make_assign(tmp_discriminant, Rvalue::Discriminant(tmp_ordering));

            let discriminant_switch_int = TerminatorKind::SwitchInt {
                discr: Operand::Copy(tmp_discriminant),
                targets: SwitchTargets::static_if(
                    0,
                    next_compare_block,
                    return_this_ordering_block,
                ),
            };

            self.block(vec![tmp_discriminant_stmt], discriminant_switch_int, false);

            let retval_stmt = self.make_assign(dest, Rvalue::Use(Operand::Copy(tmp_ordering)));

            self.block(vec![retval_stmt], TerminatorKind::Return, false);
        }

        // Compare the last field
        {
            let &(field_ty, field_idx) = last_field;

            let field_ref_ty = Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, field_ty);
            let lhs_ref = self.make_place(Mutability::Not, field_ref_ty);
            let rhs_ref = self.make_place(Mutability::Not, field_ref_ty);

            // `let lhs_ref: &ty = &(*lhs).field_idx;`
            let lhs_ref_stmt = self.make_assign(
                lhs_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    lhs.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );
            // `let rhs_ref: &ty = &(*rhs).field_idx;`
            let rhs_ref_stmt = self.make_assign(
                rhs_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    rhs.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );

            let return_block = self.block_index_offset(1);

            let args = [lhs_ref, rhs_ref]
                .into_iter()
                .map(|p| Spanned { node: Operand::Copy(p), span: DUMMY_SP })
                .collect();
            let cmp_call = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    cmp_method,
                    [ty::GenericArg::from(field_ty)],
                    self.span,
                ),
                args,
                destination: dest,
                target: Some(return_block),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: DUMMY_SP,
            };

            self.block(vec![lhs_ref_stmt, rhs_ref_stmt], cmp_call, false);

            self.block(vec![], TerminatorKind::Return, false);
        }
    }
}

/// Builds a `Debug::fmt` shim for `builtin # ptr_metadata(pointee_ty)`. Here, `def_id` is `Debug::fmt`.
fn build_ptr_metadata_fmt_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
) -> Body<'tcx> {
    let ty::InstanceKind::PtrMetadataDebugShim(def_id, pointee_ty) = instance else {
        unreachable!()
    };
    debug!("build_ptr_metadata_fmt_shim(def_id={:?})", def_id);

    let typing_env = ty::TypingEnv::post_analysis(tcx, def_id);
    let fields = match pointee_ty.metadata_fields_for_pointee(tcx, Some(typing_env)) {
        MetadataFields::KnownFields(fields) => fields,
        fields => bug!(
            "ptr_metadata fmt shim for `{:?}` which is not monomorphic enough ({fields:?})",
            pointee_ty
        ),
    };

    let fields_to_print: Vec<_> = fields
        .iter()
        .zip(FieldIdx::ZERO..)
        .filter_map(|((field_name, _, _, field_ty), field_idx)| match field_ty.kind() {
            ty::PtrMetadata(field_pointee_ty) if field_pointee_ty.is_thin(tcx, typing_env) => None,
            _ => Some((field_ty, field_idx, field_name)),
        })
        .collect();

    // FIXME(ptr_metadata_v2): handle #[non_exhaustive] pointees here too.
    let should_finish_non_exhaustive = fields_to_print.len() != fields.len();

    let mut builder = PtrMetadataFmtShimBuilder::new(tcx, instance, def_id, pointee_ty);

    let dest = Place::return_place();
    let this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
    let formatter_ref = Place::from(Local::new(2 + 0));

    builder.print_fields(dest, this, formatter_ref, &fields_to_print, should_finish_non_exhaustive);

    builder.into_mir()
}

struct PtrMetadataFmtShimExtra;
type PtrMetadataFmtShimBuilder<'tcx> = ShimBuilder<'tcx, PtrMetadataFmtShimExtra>;

impl<'tcx> PtrMetadataFmtShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        def_id: DefId,
        pointee_ty: Ty<'tcx>,
    ) -> Self {
        let sig =
            tcx.fn_sig(def_id).instantiate(tcx, &[Ty::new_ptr_metadata(tcx, pointee_ty).into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(def_id);

        PtrMetadataFmtShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: PtrMetadataFmtShimExtra,
        }
    }

    fn print_fields(
        &mut self,
        dest: Place<'tcx>,
        this: Place<'tcx>,
        formatter_ref: Place<'tcx>,
        fields: &[(Ty<'tcx>, FieldIdx, Symbol)],
        should_finish_non_exhaustive: bool,
    ) {
        let debug_struct_type = self.tcx.require_lang_item(LangItem::DebugStruct, self.span);
        let debug_struct_type = self.tcx.type_of(debug_struct_type);
        let debug_struct_type =
            self.tcx.erase_and_anonymize_regions(debug_struct_type.skip_binder());
        let debug_struct_mut_ref_type =
            Ty::new_mut_ref(self.tcx, self.tcx.lifetimes.re_erased, debug_struct_type);

        let debug_trait = self.tcx.require_lang_item(LangItem::DebugTrait, self.span);
        let debug_trait_ref =
            ty::ExistentialTraitRef::new_from_args(self.tcx, debug_trait, ty::List::empty());
        let debug_trait_predicate = ty::ExistentialPredicate::Trait(debug_trait_ref);
        let obj =
            self.tcx.mk_poly_existential_predicates(&[ty::Binder::dummy(debug_trait_predicate)]);
        let dyn_debug_type = Ty::new_dynamic(self.tcx, obj, self.tcx.lifetimes.re_erased);
        let dyn_debug_ref_type =
            Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, dyn_debug_type);

        let static_str_ref = Ty::new_static_str(self.tcx);

        let make_const_str_operand = {
            let tcx = self.tcx;
            let span = self.span;
            move |s: &str| -> Operand<'tcx> {
                let s = s.as_bytes();
                let len = s.len();
                let allocation = tcx.allocate_bytes_dedup(s, interpret::CTFE_ALLOC_SALT);
                let name_value =
                    ConstValue::Slice { alloc_id: allocation, meta: len.try_into().unwrap() };

                let name = Const::Val(name_value, static_str_ref);
                Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_: name }))
            }
        };

        // Create a `DebugStruct` using `fmt::Formatter::debug_struct`
        // bb0:
        //  _debug_struct = Formatter::debug_struct(move _formatter_ref, const "Metadata") [return -> bb1, unwind continue];

        let name = make_const_str_operand("Metadata");

        let debug_struct_place = self.make_place(Mutability::Mut, debug_struct_type);

        let args = [
            Spanned { node: Operand::Move(formatter_ref), span: DUMMY_SP },
            Spanned { node: name, span: DUMMY_SP },
        ];
        let target = self.block_index_offset(1);
        let terminator = TerminatorKind::Call {
            func: Operand::function_handle(
                self.tcx,
                self.tcx.require_lang_item(LangItem::FormatterDebugStructMethod, self.span),
                [self.tcx.lifetimes.re_erased.into()],
                self.span,
            ),
            args: Box::new(args),
            destination: debug_struct_place,
            target: Some(target),
            unwind: UnwindAction::Continue,
            call_source: CallSource::Misc,
            fn_span: self.span,
        };

        self.block(vec![], terminator, false);

        for &(field_ty, field_idx, field_name) in fields {
            // For each to-be-printed field, print it
            // bbn:
            //  _debug_struct_ref = &mut _debug_struct;
            //  _field_ref = &(*this).field_idx;
            //  _dyn_field_ref = move _field_ref as &dyn std::fmt::Debug (PointerCoercion(Unsize, Implicit));
            //  _tmp_2 = DebugStruct::field(move _tmp, const field_name, move _dyn_field_ref) [return -> bbn+1, unwind continue];

            let debug_struct_ref_place =
                self.make_place(Mutability::Mut, debug_struct_mut_ref_type);
            let unused_return_place = self.make_place(Mutability::Not, debug_struct_mut_ref_type);
            let field_ref_place = self.make_place(
                Mutability::Not,
                Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, field_ty),
            );
            let dyn_field_ref_place = self.make_place(Mutability::Not, dyn_debug_ref_type);

            let reborrow_debug_struct_stmt = self.make_assign(
                debug_struct_ref_place,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Mut { kind: MutBorrowKind::Default },
                    debug_struct_place,
                ),
            );

            let field_ref_stmt = self.make_assign(
                field_ref_place,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    this.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );

            let dyn_field_ref_stmt = self.make_assign(
                dyn_field_ref_place,
                Rvalue::Cast(
                    CastKind::PointerCoercion(
                        ty::adjustment::PointerCoercion::Unsize,
                        CoercionSource::Implicit,
                    ),
                    Operand::Move(field_ref_place),
                    dyn_debug_ref_type,
                ),
            );

            let name = make_const_str_operand(field_name.as_str());
            let args = [
                Spanned { node: Operand::Move(debug_struct_ref_place), span: DUMMY_SP },
                Spanned { node: name, span: DUMMY_SP },
                Spanned { node: Operand::Move(dyn_field_ref_place), span: DUMMY_SP },
            ];
            let target = self.block_index_offset(1);
            let terminator = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    self.tcx.require_lang_item(LangItem::DebugStructField, self.span),
                    [self.tcx.lifetimes.re_erased.into(); 2],
                    self.span,
                ),
                args: Box::new(args),
                destination: unused_return_place,
                target: Some(target),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: self.span,
            };

            self.block(
                vec![reborrow_debug_struct_stmt, field_ref_stmt, dyn_field_ref_stmt],
                terminator,
                false,
            );
        }

        // Call `DebugStruct::finish` or `DebugString::finish_non_exhaustive`,
        // writing into the return place,
        // then return

        // bbn:
        //  _debug_struct_ref = &mut _debug_struct;
        //  _0 = DebugStruct::finish/finish_non_exhaustive(move _debug_struct_ref) -> [return -> bbN+1, unwind continue];
        // bbn+1:
        //  return

        let debug_struct_ref_place = self.make_place(Mutability::Mut, debug_struct_mut_ref_type);

        let reborrow_debug_struct_stmt = self.make_assign(
            debug_struct_ref_place,
            Rvalue::Ref(
                self.tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                debug_struct_place,
            ),
        );

        let args = [Spanned { node: Operand::Move(debug_struct_ref_place), span: DUMMY_SP }];
        let target = self.block_index_offset(1);
        let finish = if should_finish_non_exhaustive {
            self.tcx.require_lang_item(LangItem::DebugStructFinishNonExhaustive, self.span)
        } else {
            self.tcx.require_lang_item(LangItem::DebugStructFinish, self.span)
        };
        let terminator = TerminatorKind::Call {
            func: Operand::function_handle(
                self.tcx,
                finish,
                [self.tcx.lifetimes.re_erased.into(); 2],
                self.span,
            ),
            args: Box::new(args),
            destination: dest,
            target: Some(target),
            unwind: UnwindAction::Continue,
            call_source: CallSource::Misc,
            fn_span: self.span,
        };

        self.block(vec![reborrow_debug_struct_stmt], terminator, false);

        self.block(vec![], TerminatorKind::Return, false);
    }
}

/// Builds a `Hash::hash` shim for `builtin # ptr_metadata(pointee_ty)`. Here, `def_id` is `Hash::hash`.
fn build_ptr_metadata_hash_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
) -> Body<'tcx> {
    let ty::InstanceKind::PtrMetadataHashShim(def_id, pointee_ty, hasher_ty) = instance else {
        unreachable!()
    };
    debug!("build_ptr_metadata_hash_shim(def_id={:?})", def_id);

    let typing_env = ty::TypingEnv::post_analysis(tcx, def_id);
    let fields = match pointee_ty.metadata_fields_for_pointee(tcx, Some(typing_env)) {
        MetadataFields::KnownFields(fields) => fields,
        fields => bug!(
            "ptr_metadata hash shim for `{:?}` which is not monomorphic enough ({fields:?})",
            pointee_ty
        ),
    };

    let fields_to_hash: Vec<_> = fields
        .iter()
        .zip(FieldIdx::ZERO..)
        .filter_map(|((_, _, _, field_ty), field_idx)| match field_ty.kind() {
            ty::PtrMetadata(field_pointee_ty) if field_pointee_ty.is_thin(tcx, typing_env) => None,
            _ => Some((field_ty, field_idx)),
        })
        .collect();

    let mut builder = PtrMetadataHashShimBuilder::new(tcx, instance, def_id, pointee_ty, hasher_ty);

    let dest = Place::return_place();
    let this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
    let hasher_mut_ref = Place::from(Local::new(2 + 0));

    builder.hash_fields(dest, this, hasher_mut_ref, &fields_to_hash);

    builder.into_mir()
}

struct PtrMetadataHashShimExtra<'tcx> {
    hasher_ty: Ty<'tcx>,
}
type PtrMetadataHashShimBuilder<'tcx> = ShimBuilder<'tcx, PtrMetadataHashShimExtra<'tcx>>;

impl<'tcx> PtrMetadataHashShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        def_id: DefId,
        pointee_ty: Ty<'tcx>,
        hasher_ty: Ty<'tcx>,
    ) -> Self {
        let sig = tcx
            .fn_sig(def_id)
            .instantiate(tcx, &[Ty::new_ptr_metadata(tcx, pointee_ty).into(), hasher_ty.into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(def_id);

        PtrMetadataHashShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: PtrMetadataHashShimExtra { hasher_ty },
        }
    }

    fn hash_fields(
        &mut self,
        dest: Place<'tcx>,
        this: Place<'tcx>,
        hasher_mut_ref: Place<'tcx>,
        fields: &[(Ty<'tcx>, FieldIdx)],
    ) {
        for &(field_ty, field_idx) in fields {
            // For each to-be-hashed field, hash it
            // bbn:
            //  _field_ref = &(*this).field_idx;
            //  _0 = DebugStruct::(move _field_ref, copy _hasher_mut_ref) [return -> bbn+1, unwind continue];

            let field_ref_place = self.make_place(
                Mutability::Not,
                Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, field_ty),
            );

            let field_ref_stmt = self.make_assign(
                field_ref_place,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    this.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx),
                ),
            );

            let args = [
                Spanned { node: Operand::Move(field_ref_place), span: DUMMY_SP },
                Spanned { node: Operand::Copy(hasher_mut_ref), span: DUMMY_SP },
            ];
            let target = self.block_index_offset(1);
            let terminator = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    self.tcx.require_lang_item(LangItem::HashMethod, self.span),
                    [field_ty.into(), self.extra.hasher_ty.into()],
                    self.span,
                ),
                args: Box::new(args),
                destination: dest,
                target: Some(target),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: self.span,
            };

            self.block(vec![field_ref_stmt], terminator, false);
        }

        // Just return. The return place is `()` which doesn't need need to be initialized.

        // bbn+1:
        //  return

        self.block(vec![], TerminatorKind::Return, false);
    }
}

/// Builds a shim for a method from the `std::init::Init` family of traits
/// for a builtin initializer type.
fn build_init_shim<'tcx>(tcx: TyCtxt<'tcx>, instance: ty::InstanceKind<'tcx>) -> Body<'tcx> {
    let ty::InstanceKind::InitShim { method_def, method, self_ty, dst_ty, error_ty, arg_ty } =
        instance
    else {
        unreachable!()
    };
    debug!("build_init_shim(def_id={:?})", method_def);

    let mut builder =
        InitShimBuilder::new(tcx, instance, method_def, self_ty, dst_ty, error_ty, arg_ty);

    let dest = Place::return_place();

    match method {
        ty::InitMethod::Metadata => {
            let this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
            match (self_ty.kind(), dst_ty.kind()) {
                (&ty::InitTuple(init_elem_tys), &ty::Tuple(elem_tys)) => {
                    builder.tuple_metadata(dest, this, init_elem_tys, elem_tys);
                }
                (&ty::InitAdt(info), &ty::Adt(adt_def, adt_args)) => {
                    builder.adt_metadata(dest, this, info, adt_def, adt_args);
                }
                pair => bug!("unsupported {pair:?}"),
            }
        }
        ty::InitMethod::ShouldZero => {
            let _this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
            // FIXME(in_place_init): query the component initializers

            let false_const = Operand::const_from_scalar(
                tcx,
                tcx.types.bool,
                interpret::Scalar::from_i8(0),
                builder.span,
            );
            let assign_false_to_return_place = builder.make_assign(dest, Rvalue::Use(false_const));
            builder.block(vec![assign_false_to_return_place], TerminatorKind::Return, false);
        }
        ty::InitMethod::InitOnce => {
            let this = Place::from(Local::new(1 + 0));
            let dst_mut_ref = Place::from(Local::new(2 + 0));
            let arg = Place::from(Local::new(3 + 0));
            let pre_zeroed = Place::from(Local::new(4 + 0));
            match (self_ty.kind(), dst_ty.kind()) {
                (&ty::InitTuple(init_elem_tys), &ty::Tuple(elem_tys)) => {
                    builder.tuple_init_once(
                        dest,
                        this,
                        dst_mut_ref,
                        arg,
                        pre_zeroed,
                        init_elem_tys,
                        elem_tys,
                    );
                }
                (&ty::InitAdt(info), &ty::Adt(adt_def, adt_args)) => {
                    builder.adt_init_once(
                        dest,
                        this,
                        dst_mut_ref,
                        arg,
                        pre_zeroed,
                        info,
                        adt_def,
                        adt_args,
                    );
                }
                pair => bug!("unsupported {pair:?}"),
            }
        }
        ty::InitMethod::InitMut => {
            let this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
            if true {
                todo!("{:?}", (&dest, &this));
            }
        }
        ty::InitMethod::InitRef => {
            let this = tcx.mk_place_deref(Place::from(Local::new(1 + 0)));
            if true {
                todo!("{:?}", (&dest, &this));
            }
        }
    }

    builder.into_mir()
}

struct InitShimExtra<'tcx> {
    init_method_def_id: DefId,
    dst_ty: Ty<'tcx>,
    error_ty: Ty<'tcx>,
    arg_ty: Ty<'tcx>,
}
type InitShimBuilder<'tcx> = ShimBuilder<'tcx, InitShimExtra<'tcx>>;

impl<'tcx> InitShimBuilder<'tcx> {
    fn new(
        tcx: TyCtxt<'tcx>,
        instance: ty::InstanceKind<'tcx>,
        init_method_def_id: DefId,
        self_ty: Ty<'tcx>,
        dst_ty: Ty<'tcx>,
        error_ty: Ty<'tcx>,
        arg_ty: Ty<'tcx>,
    ) -> Self {
        let sig = tcx
            .fn_sig(init_method_def_id)
            .instantiate(tcx, &[self_ty.into(), dst_ty.into(), error_ty.into(), arg_ty.into()]);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        let span = tcx.def_span(init_method_def_id);

        InitShimBuilder {
            tcx,
            local_decls: local_decls_for_sig(&sig, span),
            blocks: IndexVec::new(),
            span,
            sig,
            instance,
            extra: InitShimExtra { init_method_def_id, dst_ty, error_ty, arg_ty },
        }
    }

    fn tuple_metadata(
        &mut self,
        dest: Place<'tcx>,
        this: Place<'tcx>,
        init_elem_tys: &[Ty<'tcx>],
        elem_tys: &[Ty<'tcx>],
    ) {
        let InitShimExtra { init_method_def_id, dst_ty, error_ty, arg_ty } = self.extra;

        if dst_ty.is_thin(self.tcx, ty::TypingEnv::post_analysis(self.tcx, init_method_def_id)) {
            // if `dst_ty` is `Thin`, then its metadata is zero-sized, so we can just `return`
            self.block(vec![], TerminatorKind::Return, false);
            return;
        }
        // For each element, write its metadata to the corresponding field of the `Metadata<DST>` return place,
        // then return. Metadata never has drop glue, so we never need any cleanup blocks.
        // bbn:
        //  _elem_init_ref = &(*this).IDX;
        //  _0.IDX = <Self::IDX as PinInitOnce<DST::IDX, Error, Arg>>::metadata(_elem_init_ref) [return -> bbn+1, unwind continue];

        for (field_idx, (&init_elem_ty, &elem_ty)) in
            std::iter::zip(init_elem_tys, elem_tys).enumerate()
        {
            let field_idx = FieldIdx::new(field_idx);
            let elem_init_ref_place = self.make_place(
                Mutability::Not,
                Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, init_elem_ty),
            );

            let elem_init_ref_stmt = self.make_assign(
                elem_init_ref_place,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    this.project_deeper(&[PlaceElem::Field(field_idx, init_elem_ty)], self.tcx),
                ),
            );

            let elem_metadata_dest = dest.project_deeper(
                &[PlaceElem::Field(field_idx, Ty::new_ptr_metadata(self.tcx, elem_ty))],
                self.tcx,
            );

            let args = [Spanned { node: Operand::Move(elem_init_ref_place), span: DUMMY_SP }];
            let target = self.block_index_offset(1);
            let terminator = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    self.tcx.require_lang_item(LangItem::InitMetadataFn, self.span),
                    [init_elem_ty.into(), elem_ty.into(), error_ty.into(), arg_ty.into()],
                    self.span,
                ),
                args: Box::new(args),
                destination: elem_metadata_dest,
                target: Some(target),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: self.span,
            };

            self.block(vec![elem_init_ref_stmt], terminator, false);
        }

        self.block(vec![], TerminatorKind::Return, false);
    }

    fn tuple_init_once(
        &mut self,
        return_place: Place<'tcx>,
        this: Place<'tcx>,
        dst_mu_ref: Place<'tcx>,
        arg: Place<'tcx>,
        pre_zeroed: Place<'tcx>,
        init_elem_tys: &[Ty<'tcx>],
        elem_tys: &[Ty<'tcx>],
    ) {
        let InitShimExtra { init_method_def_id, dst_ty, error_ty, arg_ty } = self.extra;

        if init_elem_tys.is_empty() {
            // If there are no elements, then just drop Arg and return `Ok(())`.

            // Drop Arg
            let target = self.block_index_offset(1);
            self.block(
                vec![],
                TerminatorKind::Drop {
                    place: arg,
                    target,
                    // If there are no elements, then there's nothing to clean up if this unwinds
                    unwind: UnwindAction::Continue,
                    replace: false,
                    drop: None,
                    async_fut: None,
                },
                false,
            );

            // Return `Ok(())`
            let result_ty = return_place.ty(&self.local_decls, self.tcx).ty;
            let ty::Adt(result_def, result_args) = result_ty.kind() else { unreachable!() };

            let ok_unit = Rvalue::Aggregate(
                Box::new(AggregateKind::Adt(
                    result_def.did(),
                    VariantIdx::ZERO,
                    result_args,
                    None,
                    None,
                )),
                [Operand::Constant(Box::new(ConstOperand {
                    span: self.span,
                    user_ty: None,
                    const_: Const::Val(ConstValue::ZeroSized, self.tcx.types.unit),
                }))]
                .into(),
            );

            let write_ok_unit_stmt = self.make_assign(return_place, ok_unit);

            self.block(vec![write_ok_unit_stmt], TerminatorKind::Return, false);
            return;
        }

        // bb0:
        //  _arg_needs_drop = true;
        //  _return_needs_drop_on_unwind = false;
        //  _init_*_needs_drop = true;
        //  _elem_*_needs_drop = false;
        //  _dst_MU_ptr = &raw mut *dst_MU_ref;
        //  _dst_ptr = _dst_MU_ptr as *mut DST;
        //  goto -> bb3 (first bb that does work);

        let mut entry_stmts = vec![];

        // We keep one bool for each initializer element, each destination element,
        // the `Arg`, and the return place to determine if they need to be dropped
        // on failure or unwind.
        // These bools are ignored on success.
        // `return_needs_drop_on_unwind` is only used on an unwind; it is set to `true` after a failure,
        // but is ignored unless failure cleanup unwinds before finishing.

        let arg_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
        entry_stmts.push(self.make_set_bool_stmt(arg_needs_drop, true));

        let return_needs_drop_on_unwind = self.make_place(Mutability::Mut, self.tcx.types.bool);
        entry_stmts.push(self.make_set_bool_stmt(return_needs_drop_on_unwind, false));

        let init_elems_needs_drop: Vec<_> = init_elem_tys
            .iter()
            .map(|_| {
                let init_elem_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
                entry_stmts.push(self.make_set_bool_stmt(init_elem_needs_drop, true));
                init_elem_needs_drop
            })
            .collect();

        let elems_needs_drop: Vec<_> = init_elem_tys
            .iter()
            .map(|_| {
                let elem_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
                entry_stmts.push(self.make_set_bool_stmt(elem_needs_drop, false));
                elem_needs_drop
            })
            .collect();

        let dst_mu_ptr_ty = Ty::new_mut_ptr(self.tcx, Ty::new_maybe_uninit(self.tcx, dst_ty));
        let dst_mu_ptr = self.make_place(Mutability::Not, dst_mu_ptr_ty);
        // Note that reference-to-raw-ptr casts are translated into &raw mut/const *r, i.e., they are not actually casts.
        entry_stmts.push(self.make_assign(
            dst_mu_ptr,
            Rvalue::RawPtr(
                RawPtrKind::Mut,
                dst_mu_ref.project_deeper(&[PlaceElem::Deref], self.tcx),
            ),
        ));

        let dst_ptr_ty = Ty::new_mut_ptr(self.tcx, dst_ty);
        let dst_ptr = self.make_place(Mutability::Not, dst_ptr_ty);
        entry_stmts.push(self.make_assign(
            dst_ptr,
            Rvalue::Cast(CastKind::PtrToPtr, Operand::Move(dst_mu_ptr), dst_ptr_ty),
        ));

        let entry_target = self.block_index_offset(3);
        self.block(entry_stmts, TerminatorKind::Goto { target: entry_target }, false);

        // Failure block
        // bb1:
        //  return_needs_drop_on_unwind = true;
        //  goto -> first failure cleanup bb; (patched after loop)
        let failure_cleanup_target = self.block_index_offset(0);
        self.block(
            vec![self.make_set_bool_stmt(return_needs_drop_on_unwind, true)],
            TerminatorKind::Goto { target: failure_cleanup_target },
            false,
        );

        // bb2 (cleanup):
        //  goto -> first unwind cleanup bb; (patched after loop)
        let unwind_cleanup_target = self.block_index_offset(0);
        self.block(vec![], TerminatorKind::Goto { target: unwind_cleanup_target }, true);

        // Each element has several blocks:
        // 1. Clone (or set arg_needs_drop=false and move) `arg` for that element [return -> keep going, unwind -> bb2]
        // 2. set init_elem_needs_drop=false, then call `init_once` for that element [return -> keep going, unwind -> bb2]
        // 3. check if `init_once` succeeded [yes -> keep going, no -> bb1]
        // 4. set elem_needs_drop=true, then keep going

        for (idx, (&init_elem_ty, &elem_ty)) in std::iter::zip(init_elem_tys, elem_tys).enumerate()
        {
            let field_idx = FieldIdx::new(idx);

            // Get the Arg for the call
            let elem_arg = self.make_place(Mutability::Not, arg_ty);
            let target = self.block_index_offset(1);
            if idx + 1 == init_elem_tys.len() {
                // Move out of arg
                // bb:
                //  _arg_needs_drop = false;
                //  _elem_arg = move arg;
                //  goto -> next;
                let move_stmt = self.make_assign(elem_arg, Rvalue::Use(Operand::Move(arg)));
                let set_arg_needs_drop_stmt = self.make_assign(
                    arg_needs_drop,
                    Rvalue::Use(Operand::const_from_scalar(
                        self.tcx,
                        self.tcx.types.bool,
                        interpret::Scalar::from_bool(false),
                        self.span,
                    )),
                );
                self.block(
                    vec![move_stmt, set_arg_needs_drop_stmt],
                    TerminatorKind::Goto { target },
                    false,
                );
            } else {
                // Clone arg
                // bb:
                //  _elem_arg = Arg::clone(&arg) [return -> next, unwind -> unwind_cleanup_target];
                self.make_clone_call(elem_arg, arg, arg_ty, target, unwind_cleanup_target);
            }

            // Get the Dst MU reference for the call, then
            // set the initializer as moved and do the call
            // bb:
            //  _dst_elem_ptr = &raw mut (*dst_ptr).IDX;
            //  _dst_mu_elem_ptr = _dst_elem_ptr as *mut MaybeUninit<ELEM>;
            //  _dst_mu_elem_ref = &mut *_dst_mu_elem_ptr;
            //  _init_elem_IDX_needs_drop = false;
            //  _return_place = <INITELEM as PinInitOnce<ELEM, Error, Arg>>::init_once(
            //    move this.IDX,
            //    move _dst_mu_elem_ref,
            //    move elem_arg,
            //    copy pre_zeroed,
            //  ) [return -> next, unwind -> unwind_cleanup_target];
            let mu_elem_ty = Ty::new_maybe_uninit(self.tcx, elem_ty);

            //  _dst_elem_ptr = &raw mut (*dst_ptr).IDX;
            let dst_elem_place = dst_ptr.project_deeper(
                &[PlaceElem::Deref, PlaceElem::Field(field_idx, elem_ty)],
                self.tcx,
            );
            let dst_elem_ptr_ty = Ty::new_mut_ptr(self.tcx, elem_ty);
            let dst_elem_ptr = self.make_place(Mutability::Not, dst_elem_ptr_ty);
            let dst_elem_ptr_stmt =
                self.make_assign(dst_elem_ptr, Rvalue::RawPtr(RawPtrKind::Mut, dst_elem_place));

            //  _dst_mu_elem_ptr = _dst_elem_ptr as *mut MaybeUninit<ELEM>;
            let dst_mu_elem_ptr_ty = Ty::new_mut_ptr(self.tcx, mu_elem_ty);
            let dst_mu_elem_ptr = self.make_place(Mutability::Not, dst_mu_elem_ptr_ty);
            let cast_elem_ptr_to_mu_elem_ptr_stmt = self.make_assign(
                dst_mu_elem_ptr,
                Rvalue::Cast(CastKind::PtrToPtr, Operand::Move(dst_elem_ptr), dst_mu_elem_ptr_ty),
            );

            //  _dst_mu_elem_ref = &mut *_dst_mu_elem_ptr;
            let dst_mu_elem_ref_ty =
                Ty::new_mut_ref(self.tcx, self.tcx.lifetimes.re_erased, mu_elem_ty);
            let dst_mu_elem_ref = self.make_place(Mutability::Not, dst_mu_elem_ref_ty);

            let dst_mu_elem_ref_stmt = self.make_assign(
                dst_mu_elem_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Mut { kind: MutBorrowKind::Default },
                    dst_mu_elem_ptr.project_deeper(&[PlaceElem::Deref], self.tcx),
                ),
            );

            //  _init_elem_IDX_needs_drop = false;
            let set_init_elem_needs_drop_stmt = self.make_assign(
                init_elems_needs_drop[idx],
                Rvalue::Use(Operand::const_from_scalar(
                    self.tcx,
                    self.tcx.types.bool,
                    interpret::Scalar::from_bool(false),
                    self.span,
                )),
            );

            //  _return_place = <INITELEM as PinInitOnce<ELEM, Error, Arg>>::init_once(
            //    move this.IDX,
            //    move _dst_mu_elem_ref,
            //    move elem_arg,
            //    copy pre_zeroed,
            //  ) [return -> next, unwind -> unwind_cleanup_target];
            let target = self.block_index_offset(1);
            let func_ty = Ty::new_fn_def(
                self.tcx,
                init_method_def_id,
                [init_elem_ty, elem_ty, error_ty, arg_ty],
            );
            let func = Operand::Constant(Box::new(ConstOperand {
                span: self.span,
                user_ty: None,
                const_: Const::zero_sized(func_ty),
            }));
            let args = [
                Operand::Move(
                    this.project_deeper(&[PlaceElem::Field(field_idx, init_elem_ty)], self.tcx),
                ),
                Operand::Move(dst_mu_elem_ref),
                Operand::Move(elem_arg),
                Operand::Copy(pre_zeroed),
            ]
            .map(|arg| Spanned { node: arg, span: self.span });
            self.block(
                vec![
                    dst_elem_ptr_stmt,
                    cast_elem_ptr_to_mu_elem_ptr_stmt,
                    dst_mu_elem_ref_stmt,
                    set_init_elem_needs_drop_stmt,
                ],
                TerminatorKind::Call {
                    func,
                    args: args.into(),
                    destination: return_place,
                    target: Some(target),
                    unwind: UnwindAction::Cleanup(unwind_cleanup_target),
                    call_source: CallSource::Normal,
                    fn_span: self.span,
                },
                false,
            );

            // Check if it succeeded
            // bb:
            //  _result_discr = discriminant(_return_place);
            //  // 0 is discriminant of Result::Ok
            //  switchInt [0 -> next, otherwise -> failure_cleanup_target]
            let continue_target = self.block_index_offset(1);
            let result_discr = self.make_place(Mutability::Not, self.tcx.types.isize);
            let get_result_discr_stmt =
                self.make_assign(result_discr, Rvalue::Discriminant(return_place));
            self.block(
                vec![get_result_discr_stmt],
                TerminatorKind::SwitchInt {
                    discr: Operand::Move(result_discr),
                    targets: SwitchTargets::static_if(0, continue_target, failure_cleanup_target),
                },
                false,
            );

            // Set the element as initialized, then go to the next loop iteration (or the return).
            // bb:
            //  _elem_IDX_needs_drop = true;
            //  goto -> next;
            let target = self.block_index_offset(1);
            self.block(
                vec![self.make_set_bool_stmt(elems_needs_drop[idx], true)],
                TerminatorKind::Goto { target },
                false,
            );
        }

        // All the initializations succeeded and `Arg` was moved, so there's no drops to do.
        // return
        self.block(vec![], TerminatorKind::Return, false);

        // Now we make the two cleanup loops and patch bb1 and bb2 to point at them

        // Cleanup loop for non-panic failure.
        let real_failure_cleanup_target = self.block_index_offset(0);
        self.blocks[failure_cleanup_target].terminator = Some(Terminator {
            source_info: self.source_info(),
            kind: TerminatorKind::Goto { target: real_failure_cleanup_target },
        });

        // Cleanup element initializers
        for (idx, &init_elem_needs_drop) in init_elems_needs_drop.iter().enumerate() {
            // Check if this element initializer needs to be dropped
            self.make_cleanup_blocks(
                init_elem_needs_drop,
                this.project_deeper(
                    &[PlaceElem::Field(FieldIdx::new(idx), init_elem_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                false,
            );
        }

        // Cleanup destination elements
        for (idx, &elem_needs_drop) in elems_needs_drop.iter().enumerate() {
            // Check if this element needs to be dropped
            self.make_cleanup_blocks(
                elem_needs_drop,
                dst_ptr.project_deeper(
                    &[PlaceElem::Deref, PlaceElem::Field(FieldIdx::new(idx), elem_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                false,
            );
        }

        // Cleanup the `Arg`
        self.make_cleanup_blocks(arg_needs_drop, arg, unwind_cleanup_target, false);

        // Done with failure cleanup, `return_place` contains the `Err` from
        // the initializer that failed, return.
        self.block(vec![], TerminatorKind::Return, false);

        // Cleanup loop for unwinds
        let real_unwind_cleanup_target = self.block_index_offset(0);
        self.blocks[unwind_cleanup_target].terminator = Some(Terminator {
            source_info: self.source_info(),
            kind: TerminatorKind::Goto { target: real_unwind_cleanup_target },
        });

        // Cleanup element initializers
        for (idx, &init_elem_needs_drop) in init_elems_needs_drop.iter().enumerate() {
            // Check if this element initializer needs to be dropped
            self.make_cleanup_blocks(
                init_elem_needs_drop,
                this.project_deeper(
                    &[PlaceElem::Field(FieldIdx::new(idx), init_elem_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                /* is_cleanup */ true,
            );
        }

        // Cleanup destination elements
        for (idx, &elem_needs_drop) in elems_needs_drop.iter().enumerate() {
            // Check if this element needs to be dropped
            self.make_cleanup_blocks(
                elem_needs_drop,
                dst_ptr.project_deeper(
                    &[PlaceElem::Deref, PlaceElem::Field(FieldIdx::new(idx), elem_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                /* is_cleanup */ true,
            );
        }

        // Cleanup the `Arg`
        self.make_cleanup_blocks(arg_needs_drop, arg, unwind_cleanup_target, true);

        // Cleanup the return place
        self.make_cleanup_blocks(
            return_needs_drop_on_unwind,
            return_place,
            unwind_cleanup_target,
            true,
        );

        // Done with unwind cleanup, resume unwinding
        self.block(vec![], TerminatorKind::UnwindResume, true);
    }

    fn adt_metadata(
        &mut self,
        dest: Place<'tcx>,
        this: Place<'tcx>,
        init_info: ty::InitAdtInfo<'tcx>,
        adt_def: ty::AdtDef<'tcx>,
        adt_args: ty::GenericArgsRef<'tcx>,
    ) {
        let InitShimExtra { init_method_def_id, dst_ty, error_ty, arg_ty } = self.extra;
        let typing_env = ty::TypingEnv::post_analysis(self.tcx, init_method_def_id);

        if dst_ty.is_thin(self.tcx, typing_env) {
            // if `dst_ty` is `Thin`, then its metadata is zero-sized, so we can just `return`
            self.block(vec![], TerminatorKind::Return, false);
            return;
        }
        if adt_def.is_enum() {
            todo!("unsized enums");
        }

        let adt_variant = adt_def.variant(init_info.variant);
        let adt_fields: IndexVec<FieldIdx, Ty<'tcx>> = adt_variant
            .fields
            .iter()
            .map(|field| {
                let adt_field_ty = field.ty(self.tcx, adt_args);
                adt_field_ty
            })
            .collect();

        #[derive(Clone, Copy)]
        enum FieldMetadataHandled {
            // The field's metadata has already been handled.
            Yes,
            // The field's metadata has not yet been handled.
            No,
            // The field is `Thin`, so does not need metadata.
            Unnecessary,
        }

        let mut remaining_adt_fields: IndexVec<FieldIdx, FieldMetadataHandled> = adt_fields
            .iter()
            .map(|adt_field_ty| {
                if adt_field_ty.is_thin(self.tcx, typing_env) {
                    FieldMetadataHandled::Unnecessary
                } else {
                    FieldMetadataHandled::No
                }
            })
            .collect();

        for (init_field_idx, (component_ty, component_info)) in
            std::iter::zip(init_info.component_tys, init_info.component_infos).enumerate()
        {
            let Some(adt_field_idx) = component_info.field else {
                // `_` components do not affect metadata
                continue;
            };

            match remaining_adt_fields[adt_field_idx] {
                FieldMetadataHandled::Yes => unreachable!("duplicate field"),
                FieldMetadataHandled::No => {
                    remaining_adt_fields[adt_field_idx] = FieldMetadataHandled::Yes
                }
                FieldMetadataHandled::Unnecessary => continue,
            }

            let adt_field_ty = adt_fields[adt_field_idx];

            let arg_tys =
                self.tcx.mk_type_list_from_iter(component_info.args.iter().map(|arg| match arg {
                    ty::InitAdtComponentArg::Arg => arg_ty,
                    ty::InitAdtComponentArg::Ptr(field_idx) => {
                        Ty::new_mut_ptr(self.tcx, adt_fields[field_idx])
                    }
                    ty::InitAdtComponentArg::Ref(..) | ty::InitAdtComponentArg::PinRef(..) => {
                        todo!("InitAdt refs")
                    }
                }));
            // FIXME(in_place_init): should `with arg` and `with (arg,)` have different syntax?
            let arg_ty =
                if arg_tys.len() == 1 { arg_tys[0] } else { Ty::new_tup(self.tcx, arg_tys) };

            let init_field_idx = FieldIdx::new(init_field_idx);
            let component_ref_place = self.make_place(
                Mutability::Not,
                Ty::new_imm_ref(self.tcx, self.tcx.lifetimes.re_erased, component_ty),
            );

            let component_ref_stmt = self.make_assign(
                component_ref_place,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Shared,
                    this.project_deeper(
                        &[PlaceElem::Field(init_field_idx, component_ty)],
                        self.tcx,
                    ),
                ),
            );

            let elem_metadata_dest = dest.project_deeper(
                &[PlaceElem::Field(adt_field_idx, Ty::new_ptr_metadata(self.tcx, adt_field_ty))],
                self.tcx,
            );

            let args = [Spanned { node: Operand::Move(component_ref_place), span: DUMMY_SP }];
            let target = self.block_index_offset(1);
            let terminator = TerminatorKind::Call {
                func: Operand::function_handle(
                    self.tcx,
                    self.tcx.require_lang_item(LangItem::InitMetadataFn, self.span),
                    [component_ty.into(), adt_field_ty.into(), error_ty.into(), arg_ty.into()],
                    self.span,
                ),
                args: Box::new(args),
                destination: elem_metadata_dest,
                target: Some(target),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: self.span,
            };

            self.block(vec![component_ref_stmt], terminator, false);
        }

        // FIXME(in_place_init): disallow `do init struct` exprs for unions with multiple unsized fields.
        assert!(remaining_adt_fields.iter().all(|h| !matches!(h, FieldMetadataHandled::No)));

        self.block(vec![], TerminatorKind::Return, false);
    }

    fn adt_init_once(
        &mut self,
        return_place: Place<'tcx>,
        this: Place<'tcx>,
        dst_mu_ref: Place<'tcx>,
        arg: Place<'tcx>,
        pre_zeroed: Place<'tcx>,
        init_info: ty::InitAdtInfo<'tcx>,
        adt_def: ty::AdtDef<'tcx>,
        adt_args: ty::GenericArgsRef<'tcx>,
    ) {
        let InitShimExtra { init_method_def_id, dst_ty, error_ty, arg_ty } = self.extra;
        let adt_variant = adt_def.variant(init_info.variant);

        if adt_def.is_union() {
            todo!("implement init_once for unions");
        }

        if init_info.component_tys.is_empty() {
            // If there are no components, then:
            // 1. get a pointer to the destination to use for 2/3
            // 2. fill all fields with their default values,
            // 3. set the discriminant (if enum)
            // 4. return `Ok(())`.
            // (We don't need to drop the arg, since `Arg = ()` if there are no components)
            debug_assert!(arg_ty.is_unit());

            let mut stmts = vec![];

            // Get a pointer to the destination:
            // bb:
            //  _dst_MU_ptr = &raw mut *dst_MU_ref;
            //  _dst_ptr = _dst_MU_ptr as *mut DST;
            // then we use `*_dst_ptr`

            let dst_mu_ptr_ty = Ty::new_mut_ptr(self.tcx, Ty::new_maybe_uninit(self.tcx, dst_ty));
            let dst_mu_ptr = self.make_place(Mutability::Not, dst_mu_ptr_ty);
            // Note that reference-to-raw-ptr casts are translated into &raw mut/const *r, i.e., they are not actually casts.
            stmts.push(self.make_assign(
                dst_mu_ptr,
                Rvalue::RawPtr(
                    RawPtrKind::Mut,
                    dst_mu_ref.project_deeper(&[PlaceElem::Deref], self.tcx),
                ),
            ));

            let dst_ptr_ty = Ty::new_mut_ptr(self.tcx, dst_ty);
            let dst_ptr = self.make_place(Mutability::Not, dst_ptr_ty);
            stmts.push(self.make_assign(
                dst_ptr,
                Rvalue::Cast(CastKind::PtrToPtr, Operand::Move(dst_mu_ptr), dst_ptr_ty),
            ));
            let dst_place = if adt_def.is_enum() {
                dst_ptr.project_deeper(
                    &[PlaceElem::Deref, PlaceElem::Downcast(None, init_info.variant)],
                    self.tcx,
                )
            } else {
                dst_ptr.project_deeper(&[PlaceElem::Deref], self.tcx)
            };

            // Fill all fields with their default
            for (field_idx, field) in adt_variant.fields.iter_enumerated() {
                let Some(value_const_did) = field.value else { bug!() };
                let const_ = Const::from_unevaluated(self.tcx, value_const_did)
                    .instantiate(self.tcx, adt_args);
                let field_ty = const_.ty();
                let op = Operand::Constant(Box::new(ConstOperand {
                    span: self.span,
                    user_ty: None,
                    const_,
                }));

                // (*_dst_ptr).IDX = const CONST;
                let field_dst_place =
                    dst_place.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx);

                stmts.push(self.make_assign(field_dst_place, Rvalue::Use(op)));
            }

            // Set the discriminant if this is an enum
            if adt_def.is_enum() {
                stmts.push(self.make_statement(StatementKind::SetDiscriminant {
                    place: Box::new(dst_ptr.project_deeper(&[PlaceElem::Deref], self.tcx)),
                    variant_index: init_info.variant,
                }));
            }

            // Return `Ok(())`
            let result_ty = return_place.ty(&self.local_decls, self.tcx).ty;
            let ty::Adt(result_def, result_args) = result_ty.kind() else { unreachable!() };

            let ok_unit = Rvalue::Aggregate(
                Box::new(AggregateKind::Adt(
                    result_def.did(),
                    VariantIdx::ZERO,
                    result_args,
                    None,
                    None,
                )),
                [Operand::Constant(Box::new(ConstOperand {
                    span: self.span,
                    user_ty: None,
                    const_: Const::Val(ConstValue::ZeroSized, self.tcx.types.unit),
                }))]
                .into(),
            );

            stmts.push(self.make_assign(return_place, ok_unit));

            self.block(stmts, TerminatorKind::Return, false);
            return;
        }

        // bb0:
        //  _arg_needs_drop = true;
        //  _return_needs_drop_on_unwind = false;
        //  _component_*_needs_drop = true;
        //  _adt_field_*_needs_drop = false;
        //  _dst_MU_ptr = &raw mut *dst_MU_ref;
        //  _dst_ptr = _dst_MU_ptr as *mut DST;
        //  goto -> bb3 (first bb that does work);

        let mut entry_stmts = vec![];

        // We keep one bool for each initializer element, each destination element,
        // the `Arg`, and the return place to determine if they need to be dropped
        // on failure or unwind.
        // These bools are ignored on success.
        // `return_needs_drop_on_unwind` is only used on an unwind; it is set to `true` after a failure,
        // but is ignored unless failure cleanup unwinds before finishing.

        let arg_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
        entry_stmts.push(self.make_set_bool_stmt(arg_needs_drop, true));

        let return_needs_drop_on_unwind = self.make_place(Mutability::Mut, self.tcx.types.bool);
        entry_stmts.push(self.make_set_bool_stmt(return_needs_drop_on_unwind, false));

        let components_needs_drop: Vec<_> = init_info
            .component_tys
            .iter()
            .map(|_| {
                let init_elem_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
                entry_stmts.push(self.make_set_bool_stmt(init_elem_needs_drop, true));
                init_elem_needs_drop
            })
            .collect();

        let adt_fields_needs_drop: Vec<_> = adt_variant
            .fields
            .iter()
            .map(|_| {
                let elem_needs_drop = self.make_place(Mutability::Mut, self.tcx.types.bool);
                entry_stmts.push(self.make_set_bool_stmt(elem_needs_drop, false));
                elem_needs_drop
            })
            .collect();

        let dst_mu_ptr_ty = Ty::new_mut_ptr(self.tcx, Ty::new_maybe_uninit(self.tcx, dst_ty));
        let dst_mu_ptr = self.make_place(Mutability::Not, dst_mu_ptr_ty);
        // Note that reference-to-raw-ptr casts are translated into &raw mut/const *r, i.e., they are not actually casts.
        entry_stmts.push(self.make_assign(
            dst_mu_ptr,
            Rvalue::RawPtr(
                RawPtrKind::Mut,
                dst_mu_ref.project_deeper(&[PlaceElem::Deref], self.tcx),
            ),
        ));

        let dst_ptr_ty = Ty::new_mut_ptr(self.tcx, dst_ty);
        let dst_ptr = self.make_place(Mutability::Not, dst_ptr_ty);
        entry_stmts.push(self.make_assign(
            dst_ptr,
            Rvalue::Cast(CastKind::PtrToPtr, Operand::Move(dst_mu_ptr), dst_ptr_ty),
        ));
        let dst_place = if adt_def.is_enum() {
            dst_ptr.project_deeper(
                &[PlaceElem::Deref, PlaceElem::Downcast(None, init_info.variant)],
                self.tcx,
            )
        } else {
            dst_ptr.project_deeper(&[PlaceElem::Deref], self.tcx)
        };

        let entry_target = self.block_index_offset(3);
        self.block(entry_stmts, TerminatorKind::Goto { target: entry_target }, false);

        // Failure block
        // bb1:
        //  return_needs_drop_on_unwind = true;
        //  goto -> first failure cleanup bb; (patched after loop)
        let failure_cleanup_target = self.block_index_offset(0);
        self.block(
            vec![self.make_set_bool_stmt(return_needs_drop_on_unwind, true)],
            TerminatorKind::Goto { target: failure_cleanup_target },
            false,
        );

        // bb2 (cleanup):
        //  goto -> first unwind cleanup bb; (patched after loop)
        let unwind_cleanup_target = self.block_index_offset(0);
        self.block(vec![], TerminatorKind::Goto { target: unwind_cleanup_target }, true);

        // Keep track of how many `arg` uses there are remaining, so we don't clone for the last usage.
        let mut remaining_arg_uses = init_info
            .component_infos
            .iter()
            .flat_map(|component_info| component_info.args)
            .filter(|arg| matches!(arg, InitAdtComponentArg::Arg))
            .count();

        // Each component has several blocks:
        // 1. Get the arg for that component (may be zero or more blocks).
        // 2. set component_needs_drop=false, then call `init_once` for that component [return -> keep going, unwind -> bb2]
        // 3. check if `init_once` succeeded [yes -> keep going, no -> bb1]
        // 4. if that component initialized a field, set adt_field_IDX_needs_drop=true, then keep going

        // Keep track of which fields were initialized, so we can fill all others with their default values
        // on success.
        let mut initialized_adt_fields = IndexVec::from_elem(false, &adt_variant.fields);

        for (component_idx, (component_ty, component_info)) in
            std::iter::zip(init_info.component_tys, init_info.component_infos).enumerate()
        {
            let component_field_idx = FieldIdx::new(component_idx);

            // Make the arg for the call

            let (component_arg, component_arg_ty) = self.make_adt_arg_blocks(
                adt_variant,
                adt_args,
                dst_place,
                &mut remaining_arg_uses,
                &component_info,
                arg_needs_drop,
                arg,
                unwind_cleanup_target,
            );

            // Get the Dst MU reference for the call, then
            // set the initializer as moved and do the call
            // component has a field:
            //  _adt_field_ptr = &raw mut DST_PLACE.IDX;
            //  _dst_ptr = _adt_field_ptr as *mut MaybeUninit<ELEM>;
            // component has no field:
            //  _dst_ptr = 1 as *mut MaybeUninit<()>;
            // both:
            //  _dst_ref = &mut *_dst_ptr;
            //  _component_IDX_needs_drop = false;
            //  _return_place = <INITELEM as PinInitOnce<ELEM, Error, Arg>>::init_once(
            //    move this.IDX,
            //    move _adt_field_mu_ref,
            //    move elem_arg,
            //    copy pre_zeroed,
            //  ) [return -> next, unwind -> unwind_cleanup_target];
            let mut stmts = vec![];
            let (component_dst_ty, component_dst_ptr) =
                if let Some(adt_field_idx) = component_info.field {
                    initialized_adt_fields[adt_field_idx] = true;
                    let adt_field_ty = adt_variant.fields[adt_field_idx].ty(self.tcx, adt_args);

                    let mu_adt_field_ty = Ty::new_maybe_uninit(self.tcx, adt_field_ty);

                    //  _adt_field_ptr = &raw mut DST_PLACE.IDX;
                    let adt_field_place = dst_place
                        .project_deeper(&[PlaceElem::Field(adt_field_idx, adt_field_ty)], self.tcx);
                    let adt_field_ptr_ty = Ty::new_mut_ptr(self.tcx, adt_field_ty);
                    let adt_field_ptr = self.make_place(Mutability::Not, adt_field_ptr_ty);
                    stmts.push(self.make_assign(
                        adt_field_ptr,
                        Rvalue::RawPtr(RawPtrKind::Mut, adt_field_place),
                    ));

                    //  _dst_ptr = _adt_field_ptr as *mut MaybeUninit<ELEM>;
                    let mu_adt_field_ptr_ty = Ty::new_mut_ptr(self.tcx, mu_adt_field_ty);
                    let mu_adt_field_ptr = self.make_place(Mutability::Not, mu_adt_field_ptr_ty);
                    stmts.push(self.make_assign(
                        mu_adt_field_ptr,
                        Rvalue::Cast(
                            CastKind::PtrToPtr,
                            Operand::Move(adt_field_ptr),
                            mu_adt_field_ptr_ty,
                        ),
                    ));

                    (adt_field_ty, mu_adt_field_ptr)
                } else {
                    let mu_unit_ty = Ty::new_maybe_uninit(self.tcx, self.tcx.types.unit);
                    let mu_unit_ptr_ty = Ty::new_mut_ptr(self.tcx, mu_unit_ty);
                    let mu_unit_ptr = self.make_place(Mutability::Not, mu_unit_ptr_ty);

                    stmts.push(self.make_assign(
                        mu_unit_ptr,
                        Rvalue::Cast(
                            CastKind::PointerWithExposedProvenance,
                            Operand::const_from_scalar(
                                self.tcx,
                                self.tcx.types.u8,
                                interpret::Scalar::from_u8(0),
                                self.span,
                            ),
                            mu_unit_ptr_ty,
                        ),
                    ));

                    (self.tcx.types.unit, mu_unit_ptr)
                };

            //  _dst_ref = &mut *_dst_ptr;
            let dst_ref = self.make_place(
                Mutability::Not,
                Ty::new_mut_ref(
                    self.tcx,
                    self.tcx.lifetimes.re_erased,
                    Ty::new_maybe_uninit(self.tcx, component_dst_ty),
                ),
            );
            stmts.push(self.make_assign(
                dst_ref,
                Rvalue::Ref(
                    self.tcx.lifetimes.re_erased,
                    BorrowKind::Mut { kind: MutBorrowKind::Default },
                    component_dst_ptr.project_deeper(&[PlaceElem::Deref], self.tcx),
                ),
            ));

            //  _component_IDX_needs_drop = false;
            stmts.push(self.make_assign(
                components_needs_drop[component_idx],
                Rvalue::Use(Operand::const_from_scalar(
                    self.tcx,
                    self.tcx.types.bool,
                    interpret::Scalar::from_bool(false),
                    self.span,
                )),
            ));

            //  _return_place = <INITELEM as PinInitOnce<ELEM, Error, Arg>>::init_once(
            //    move this.IDX,
            //    move _dst_ref,
            //    move elem_arg,
            //    copy pre_zeroed,
            //  ) [return -> next, unwind -> unwind_cleanup_target];
            let target = self.block_index_offset(1);
            let func_ty = Ty::new_fn_def(
                self.tcx,
                init_method_def_id,
                [component_ty, component_dst_ty, error_ty, component_arg_ty],
            );
            let func = Operand::Constant(Box::new(ConstOperand {
                span: self.span,
                user_ty: None,
                const_: Const::zero_sized(func_ty),
            }));
            let args = [
                Operand::Move(this.project_deeper(
                    &[PlaceElem::Field(component_field_idx, component_ty)],
                    self.tcx,
                )),
                Operand::Move(dst_ref),
                component_arg,
                Operand::Copy(pre_zeroed),
            ]
            .map(|arg| Spanned { node: arg, span: self.span });
            self.block(
                stmts,
                TerminatorKind::Call {
                    func,
                    args: args.into(),
                    destination: return_place,
                    target: Some(target),
                    unwind: UnwindAction::Cleanup(unwind_cleanup_target),
                    call_source: CallSource::Normal,
                    fn_span: self.span,
                },
                false,
            );

            // Check if it succeeded
            // bb:
            //  _result_discr = discriminant(_return_place);
            //  // 0 is discriminant of Result::Ok
            //  switchInt [0 -> next, otherwise -> failure_cleanup_target]
            let continue_target = self.block_index_offset(1);
            let result_discr = self.make_place(Mutability::Not, self.tcx.types.isize);
            let get_result_discr_stmt =
                self.make_assign(result_discr, Rvalue::Discriminant(return_place));
            self.block(
                vec![get_result_discr_stmt],
                TerminatorKind::SwitchInt {
                    discr: Operand::Move(result_discr),
                    targets: SwitchTargets::static_if(0, continue_target, failure_cleanup_target),
                },
                false,
            );

            // If this component initialized a field, selt the adt field as initialized.
            // Then go to the next loop iteration (or the return).
            // bb:
            //  _elem_IDX_needs_drop = true;
            //  goto -> next;
            let stmts = if let Some(adt_field_idx) = component_info.field {
                vec![self.make_set_bool_stmt(adt_fields_needs_drop[adt_field_idx.as_usize()], true)]
            } else {
                vec![]
            };
            let target = self.block_index_offset(1);
            self.block(stmts, TerminatorKind::Goto { target }, false);
        }

        // All the component initializers succeeded and `Arg` was moved, so there's no drops to do.
        // Set all remaining fields to their default values, set discriminant (if enum), and return
        let mut stmts = vec![];
        // Fill all remaining fields with their default
        for (field_idx, field) in adt_variant.fields.iter_enumerated() {
            if initialized_adt_fields[field_idx] {
                continue;
            };
            let Some(value_const_did) = field.value else { bug!() };
            let const_ =
                Const::from_unevaluated(self.tcx, value_const_did).instantiate(self.tcx, adt_args);
            let field_ty = const_.ty();
            let op = Operand::Constant(Box::new(ConstOperand {
                span: self.span,
                user_ty: None,
                const_,
            }));

            // (*_dst_ptr).IDX = const CONST;
            let field_dst_place =
                dst_place.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self.tcx);

            stmts.push(self.make_assign(field_dst_place, Rvalue::Use(op)));
        }
        // Set the discriminant (if enum)
        if adt_def.is_enum() {
            stmts.push(self.make_statement(StatementKind::SetDiscriminant {
                place: Box::new(dst_ptr.project_deeper(&[PlaceElem::Deref], self.tcx)),
                variant_index: init_info.variant,
            }));
        }
        // Return
        self.block(stmts, TerminatorKind::Return, false);

        // Now we make the two cleanup loops and patch bb1 and bb2 to point at them

        // Cleanup loop for non-panic failure.
        let real_failure_cleanup_target = self.block_index_offset(0);
        self.blocks[failure_cleanup_target].terminator = Some(Terminator {
            source_info: self.source_info(),
            kind: TerminatorKind::Goto { target: real_failure_cleanup_target },
        });

        // Cleanup component initializers
        for (idx, &component_needs_drop) in components_needs_drop.iter().enumerate() {
            // Check if this component initializer needs to be dropped
            self.make_cleanup_blocks(
                component_needs_drop,
                this.project_deeper(
                    &[PlaceElem::Field(FieldIdx::new(idx), init_info.component_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                false,
            );
        }

        // Cleanup destination fields
        for (idx, &adt_field_needs_drop) in adt_fields_needs_drop.iter().enumerate() {
            // Check if this fields needs to be dropped
            let field_ty = adt_variant.fields[FieldIdx::from_usize(idx)].ty(self.tcx, adt_args);
            self.make_cleanup_blocks(
                adt_field_needs_drop,
                dst_place
                    .project_deeper(&[PlaceElem::Field(FieldIdx::new(idx), field_ty)], self.tcx),
                unwind_cleanup_target,
                false,
            );
        }

        // Cleanup the `Arg`
        self.make_cleanup_blocks(arg_needs_drop, arg, unwind_cleanup_target, false);

        // Done with failure cleanup, `return_place` contains the `Err` from
        // the initializer that failed, return.
        self.block(vec![], TerminatorKind::Return, false);

        // Cleanup loop for unwinds
        let real_unwind_cleanup_target = self.block_index_offset(0);
        self.blocks[unwind_cleanup_target].terminator = Some(Terminator {
            source_info: self.source_info(),
            kind: TerminatorKind::Goto { target: real_unwind_cleanup_target },
        });

        // Cleanup element initializers
        for (idx, &component_needs_drop) in components_needs_drop.iter().enumerate() {
            // Check if this element initializer needs to be dropped
            self.make_cleanup_blocks(
                component_needs_drop,
                this.project_deeper(
                    &[PlaceElem::Field(FieldIdx::new(idx), init_info.component_tys[idx])],
                    self.tcx,
                ),
                unwind_cleanup_target,
                /* is_cleanup */ true,
            );
        }

        // Cleanup destination fields
        for (idx, &adt_field_needs_drop) in adt_fields_needs_drop.iter().enumerate() {
            // Check if this fields needs to be dropped
            let field_ty = adt_variant.fields[FieldIdx::from_usize(idx)].ty(self.tcx, adt_args);
            self.make_cleanup_blocks(
                adt_field_needs_drop,
                dst_place
                    .project_deeper(&[PlaceElem::Field(FieldIdx::new(idx), field_ty)], self.tcx),
                unwind_cleanup_target,
                /* is_cleanup */ true,
            );
        }

        // Cleanup the `Arg`
        self.make_cleanup_blocks(arg_needs_drop, arg, unwind_cleanup_target, true);

        // Cleanup the return place
        self.make_cleanup_blocks(
            return_needs_drop_on_unwind,
            return_place,
            unwind_cleanup_target,
            true,
        );

        // Done with unwind cleanup, resume unwinding
        self.block(vec![], TerminatorKind::UnwindResume, true);
    }

    /// Makes the `ElemArg` for a given component then jumps to the next block (if any blocks were created).
    /// On unwind, cleans up any partial parts of the `ElemArg` then jumps to `cleanup`.
    fn make_adt_arg_blocks(
        &mut self,
        adt_variant: &ty::VariantDef,
        adt_args: ty::GenericArgsRef<'tcx>,
        dst_place: Place<'tcx>,
        remaining_arg_uses: &mut usize,
        component_info: &InitAdtComponentInfo<'tcx>,
        arg_needs_drop: Place<'tcx>,
        arg: Place<'tcx>,
        mut cleanup: BasicBlock,
    ) -> (Operand<'tcx>, Ty<'tcx>) {
        let arg_ty = self.extra.arg_ty;
        let mut mk_arg = |self_: &mut Self| -> (Option<Statement<'tcx>>, Place<'tcx>, Ty<'tcx>) {
            match *remaining_arg_uses {
                0 => bug!("remaining_arg_uses was wrong?"),
                1 => {
                    // This is the last usage of `arg`, so we don't need a cleanup block
                    // clear its drop flag and move out of it.
                    let target = self_.block_index_offset(1);
                    self_.block(
                        vec![self_.make_set_bool_stmt(arg_needs_drop, false)],
                        TerminatorKind::Goto { target },
                        false,
                    );
                    *remaining_arg_uses = 0;
                    (None, arg, arg_ty)
                }
                _ => {
                    // Clone `arg` into a new place, and replace `cleanup` with a block that drops it then jumps to
                    // the old value of `cleanup`.
                    *remaining_arg_uses -= 1;
                    let elem_arg = self_.make_place(Mutability::Not, arg_ty);
                    let new_cleanup = self_.block_index_offset(1);
                    let target = self_.block_index_offset(2);
                    self_.make_clone_call(elem_arg, arg, arg_ty, target, cleanup);
                    self_.block(
                        vec![],
                        TerminatorKind::Drop {
                            place: elem_arg,
                            target: cleanup,
                            unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
                            replace: false,
                            drop: None,
                            async_fut: None,
                        },
                        true,
                    );
                    cleanup = new_cleanup;
                    (None, elem_arg, arg_ty)
                }
            }
        };
        let mk_ptr = |self_: &mut Self,
                      field_idx: FieldIdx|
         -> (Option<Statement<'tcx>>, Place<'tcx>, Ty<'tcx>) {
            let field_ty = adt_variant.fields[field_idx].ty(self_.tcx, adt_args);
            let component_arg_ty = Ty::new_mut_ptr(self_.tcx, field_ty);
            let component_arg = self_.make_place(Mutability::Not, component_arg_ty);
            let stmt = self_.make_assign(
                component_arg,
                Rvalue::RawPtr(
                    RawPtrKind::Mut,
                    dst_place.project_deeper(&[PlaceElem::Field(field_idx, field_ty)], self_.tcx),
                ),
            );
            (Some(stmt), component_arg, component_arg_ty)
        };
        let mut mk_elem_arg = |self_: &mut Self, arg: InitAdtComponentArg| match arg {
            InitAdtComponentArg::Arg => mk_arg(self_),
            InitAdtComponentArg::Ptr(field_idx) => mk_ptr(self_, field_idx),
            InitAdtComponentArg::Ref(_field_idx) => todo!(),
            InitAdtComponentArg::PinRef(_field_idx) => todo!(),
        };

        match component_info.args[..] {
            [] => (
                Operand::Constant(Box::new(ConstOperand {
                    span: self.span,
                    user_ty: None,
                    const_: Const::zero_sized(self.tcx.types.unit),
                })),
                self.tcx.types.unit,
            ),
            [arg] => {
                let (stmt, arg_place, arg_ty) = mk_elem_arg(self, arg);
                if let Some(stmt) = stmt {
                    let target = self.block_index_offset(1);
                    self.block(vec![stmt], TerminatorKind::Goto { target }, false);
                }
                (Operand::Move(arg_place), arg_ty)
            }
            ref args => {
                // The `Ptr`/`Ref`/`PinRef` args can never fail, so they can happen last in one block.
                let mut stmts = vec![];
                let mut ops = IndexVec::new();
                let mut tys = vec![];
                for &arg in args {
                    let (stmt, place, ty) = mk_elem_arg(self, arg);
                    stmts.extend(stmt);
                    ops.push(Operand::Move(place));
                    tys.push(ty);
                }
                let elem_arg_ty = Ty::new_tup(self.tcx, &tys);
                let elem_arg = self.make_place(Mutability::Not, elem_arg_ty);
                stmts.push(
                    self.make_assign(
                        elem_arg,
                        Rvalue::Aggregate(Box::new(AggregateKind::Tuple), ops),
                    ),
                );
                let target = self.block_index_offset(1);
                self.block(stmts, TerminatorKind::Goto { target }, false);
                (Operand::Move(elem_arg), elem_arg_ty)
            }
        }
    }

    fn make_clone_call(
        &mut self,
        dest: Place<'tcx>,
        src: Place<'tcx>,
        ty: Ty<'tcx>,
        next: BasicBlock,
        cleanup: BasicBlock,
    ) {
        let tcx = self.tcx;

        let clone_def_id = self.tcx.require_lang_item(LangItem::CloneFn, self.span);
        // `func == Clone::clone(&ty) -> ty`
        let func_ty = Ty::new_fn_def(tcx, clone_def_id, [ty]);
        let func = Operand::Constant(Box::new(ConstOperand {
            span: self.span,
            user_ty: None,
            const_: Const::zero_sized(func_ty),
        }));

        let ref_loc =
            self.make_place(Mutability::Not, Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, ty));

        // `let ref_loc: &ty = &src;`
        let statement = self
            .make_assign(ref_loc, Rvalue::Ref(tcx.lifetimes.re_erased, BorrowKind::Shared, src));

        // `let loc = Clone::clone(ref_loc);`
        self.block(
            vec![statement],
            TerminatorKind::Call {
                func,
                args: [Spanned { node: Operand::Move(ref_loc), span: DUMMY_SP }].into(),
                destination: dest,
                target: Some(next),
                unwind: UnwindAction::Cleanup(cleanup),
                call_source: CallSource::Normal,
                fn_span: self.span,
            },
            false,
        );
    }

    fn make_cleanup_blocks(
        &mut self,
        needs_drop_flag: Place<'tcx>,
        place: Place<'tcx>,
        unwind_cleanup_target: BasicBlock,
        is_cleanup: bool,
    ) {
        // bbn:
        //  switchInt(copy NEEDS_DROP_FLAG) [true -> bbn+1, otherwise -> bbn+2]
        // bbn+1:
        //  NEEDS_DROP_FLAG = const false;
        //  drop(PLACE) [return -> bbn+2, unwind -> [unwind_cleanup_target during failure, terminate during unwind]]
        // bbn+2: (next thing to drop)

        let drop_target = self.block_index_offset(1);
        let continue_target = self.block_index_offset(2);

        self.block(
            vec![],
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(needs_drop_flag),
                targets: SwitchTargets::static_if(1, drop_target, continue_target),
            },
            is_cleanup,
        );

        self.block(
            vec![self.make_set_bool_stmt(needs_drop_flag, false)],
            TerminatorKind::Drop {
                place,
                target: continue_target,
                unwind: if is_cleanup {
                    UnwindAction::Terminate(UnwindTerminateReason::InCleanup)
                } else {
                    UnwindAction::Cleanup(unwind_cleanup_target)
                },
                replace: false,
                drop: None,
                async_fut: None,
            },
            is_cleanup,
        );
    }
}

/// Builds a "call" shim for `instance`. The shim calls the function specified by `call_kind`,
/// first adjusting its first argument according to `rcvr_adjustment`.
#[instrument(level = "debug", skip(tcx), ret)]
fn build_call_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: ty::InstanceKind<'tcx>,
    rcvr_adjustment: Option<Adjustment>,
    call_kind: CallKind<'tcx>,
) -> Body<'tcx> {
    // `FnPtrShim` contains the fn pointer type that a call shim is being built for - this is used
    // to instantiate into the signature of the shim. It is not necessary for users of this
    // MIR body to perform further instantiations (see `InstanceKind::has_polymorphic_mir_body`).
    let (sig_args, untuple_args) = if let ty::InstanceKind::FnPtrShim(_, ty) = instance {
        let sig = tcx.instantiate_bound_regions_with_erased(ty.fn_sig(tcx));

        let untuple_args = sig.inputs();

        // Create substitutions for the `Self` and `Args` generic parameters of the shim body.
        let arg_tup = Ty::new_tup(tcx, untuple_args);

        (Some([ty.into(), arg_tup.into()]), Some(untuple_args))
    } else {
        (None, None)
    };

    let def_id = instance.def_id();

    let sig = tcx.fn_sig(def_id);
    let sig = sig.map_bound(|sig| tcx.instantiate_bound_regions_with_erased(sig));

    assert_eq!(sig_args.is_some(), !instance.has_polymorphic_mir_body());
    let mut sig = if let Some(sig_args) = sig_args {
        sig.instantiate(tcx, &sig_args)
    } else {
        sig.instantiate_identity()
    };

    if let CallKind::Indirect(fnty) = call_kind {
        // `sig` determines our local decls, and thus the callee type in the `Call` terminator. This
        // can only be an `FnDef` or `FnPtr`, but currently will be `Self` since the types come from
        // the implemented `FnX` trait.

        // Apply the opposite adjustment to the MIR input.
        let mut inputs_and_output = sig.inputs_and_output.to_vec();

        // Initial signature is `fn(&? Self, Args) -> Self::Output` where `Args` is a tuple of the
        // fn arguments. `Self` may be passed via (im)mutable reference or by-value.
        assert_eq!(inputs_and_output.len(), 3);

        // `Self` is always the original fn type `ty`. The MIR call terminator is only defined for
        // `FnDef` and `FnPtr` callees, not the `Self` type param.
        let self_arg = &mut inputs_and_output[0];
        *self_arg = match rcvr_adjustment.unwrap() {
            Adjustment::Identity => fnty,
            Adjustment::Deref { source } => match source {
                DerefSource::ImmRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, fnty),
                DerefSource::MutRef => Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, fnty),
                DerefSource::MutPtr => Ty::new_mut_ptr(tcx, fnty),
            },
            Adjustment::RefMut => bug!("`RefMut` is never used with indirect calls: {instance:?}"),
        };
        sig.inputs_and_output = tcx.mk_type_list(&inputs_and_output);
    }

    // FIXME: Avoid having to adjust the signature both here and in
    // `fn_sig_for_fn_abi`.
    if let ty::InstanceKind::VTableShim(..) = instance {
        // Modify fn(self, ...) to fn(self: *mut Self, ...)
        let mut inputs_and_output = sig.inputs_and_output.to_vec();
        let self_arg = &mut inputs_and_output[0];
        debug_assert!(tcx.generics_of(def_id).has_self && *self_arg == tcx.types.self_param);
        *self_arg = Ty::new_mut_ptr(tcx, *self_arg);
        sig.inputs_and_output = tcx.mk_type_list(&inputs_and_output);
    }

    let span = tcx.def_span(def_id);

    debug!(?sig);

    let mut local_decls = local_decls_for_sig(&sig, span);
    let source_info = SourceInfo::outermost(span);

    let destination = Place::return_place();

    let rcvr_place = || {
        assert!(rcvr_adjustment.is_some());
        Place::from(Local::new(1))
    };
    let mut statements = vec![];

    let rcvr = rcvr_adjustment.map(|rcvr_adjustment| match rcvr_adjustment {
        Adjustment::Identity => Operand::Move(rcvr_place()),
        Adjustment::Deref { source: _ } => Operand::Move(tcx.mk_place_deref(rcvr_place())),
        Adjustment::RefMut => {
            // let rcvr = &mut rcvr;
            let ref_rcvr = local_decls.push(
                LocalDecl::new(
                    Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, sig.inputs()[0]),
                    span,
                )
                .immutable(),
            );
            let borrow_kind = BorrowKind::Mut { kind: MutBorrowKind::Default };
            statements.push(Statement::new(
                source_info,
                StatementKind::Assign(Box::new((
                    Place::from(ref_rcvr),
                    Rvalue::Ref(tcx.lifetimes.re_erased, borrow_kind, rcvr_place()),
                ))),
            ));
            Operand::Move(Place::from(ref_rcvr))
        }
    });

    let (callee, mut args) = match call_kind {
        // `FnPtr` call has no receiver. Args are untupled below.
        CallKind::Indirect(_) => (rcvr.unwrap(), vec![]),

        // `FnDef` call with optional receiver.
        CallKind::Direct(def_id) => {
            let ty = tcx.type_of(def_id).instantiate_identity();
            (
                Operand::Constant(Box::new(ConstOperand {
                    span,
                    user_ty: None,
                    const_: Const::zero_sized(ty),
                })),
                rcvr.into_iter().collect::<Vec<_>>(),
            )
        }
    };

    let mut arg_range = 0..sig.inputs().len();

    // Take the `self` ("receiver") argument out of the range (it's adjusted above).
    if rcvr_adjustment.is_some() {
        arg_range.start += 1;
    }

    // Take the last argument, if we need to untuple it (handled below).
    if untuple_args.is_some() {
        arg_range.end -= 1;
    }

    // Pass all of the non-special arguments directly.
    args.extend(arg_range.map(|i| Operand::Move(Place::from(Local::new(1 + i)))));

    // Untuple the last argument, if we have to.
    if let Some(untuple_args) = untuple_args {
        let tuple_arg = Local::new(1 + (sig.inputs().len() - 1));
        args.extend(untuple_args.iter().enumerate().map(|(i, ity)| {
            Operand::Move(tcx.mk_place_field(Place::from(tuple_arg), FieldIdx::new(i), *ity))
        }));
    }

    let n_blocks = if let Some(Adjustment::RefMut) = rcvr_adjustment { 5 } else { 2 };
    let mut blocks = IndexVec::with_capacity(n_blocks);
    let block = |blocks: &mut IndexVec<_, _>, statements, kind, is_cleanup| {
        blocks.push(BasicBlockData::new_stmts(
            statements,
            Some(Terminator { source_info, kind }),
            is_cleanup,
        ))
    };

    // BB #0
    let args = args.into_iter().map(|a| Spanned { node: a, span: DUMMY_SP }).collect();
    block(
        &mut blocks,
        statements,
        TerminatorKind::Call {
            func: callee,
            args,
            destination,
            target: Some(BasicBlock::new(1)),
            unwind: if let Some(Adjustment::RefMut) = rcvr_adjustment {
                UnwindAction::Cleanup(BasicBlock::new(3))
            } else {
                UnwindAction::Continue
            },
            call_source: CallSource::Misc,
            fn_span: span,
        },
        false,
    );

    if let Some(Adjustment::RefMut) = rcvr_adjustment {
        // BB #1 - drop for Self
        block(
            &mut blocks,
            vec![],
            TerminatorKind::Drop {
                place: rcvr_place(),
                target: BasicBlock::new(2),
                unwind: UnwindAction::Continue,
                replace: false,
                drop: None,
                async_fut: None,
            },
            false,
        );
    }
    // BB #1/#2 - return
    let stmts = vec![];
    block(&mut blocks, stmts, TerminatorKind::Return, false);
    if let Some(Adjustment::RefMut) = rcvr_adjustment {
        // BB #3 - drop if closure panics
        block(
            &mut blocks,
            vec![],
            TerminatorKind::Drop {
                place: rcvr_place(),
                target: BasicBlock::new(4),
                unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
                replace: false,
                drop: None,
                async_fut: None,
            },
            /* is_cleanup */ true,
        );

        // BB #4 - resume
        block(&mut blocks, vec![], TerminatorKind::UnwindResume, true);
    }

    let mut body =
        new_body(MirSource::from_instance(instance), blocks, local_decls, sig.inputs().len(), span);

    if let ExternAbi::RustCall = sig.abi {
        body.spread_arg = Some(Local::new(sig.inputs().len()));
    }

    body
}

pub(super) fn build_adt_ctor(tcx: TyCtxt<'_>, ctor_id: DefId) -> Body<'_> {
    debug_assert!(tcx.is_constructor(ctor_id));

    let typing_env = ty::TypingEnv::post_analysis(tcx, ctor_id);

    // Normalize the sig.
    let sig = tcx
        .fn_sig(ctor_id)
        .instantiate_identity()
        .no_bound_vars()
        .expect("LBR in ADT constructor signature");
    let sig = tcx.normalize_erasing_regions(typing_env, sig);

    let ty::Adt(adt_def, args) = sig.output().kind() else {
        bug!("unexpected type for ADT ctor {:?}", sig.output());
    };

    debug!("build_ctor: ctor_id={:?} sig={:?}", ctor_id, sig);

    let span = tcx.def_span(ctor_id);

    let local_decls = local_decls_for_sig(&sig, span);

    let source_info = SourceInfo::outermost(span);

    let variant_index =
        if adt_def.is_enum() { adt_def.variant_index_with_ctor_id(ctor_id) } else { FIRST_VARIANT };

    // Generate the following MIR:
    //
    // (return as Variant).field0 = arg0;
    // (return as Variant).field1 = arg1;
    //
    // return;
    debug!("build_ctor: variant_index={:?}", variant_index);

    let kind = AggregateKind::Adt(adt_def.did(), variant_index, args, None, None);
    let variant = adt_def.variant(variant_index);
    let statement = Statement::new(
        source_info,
        StatementKind::Assign(Box::new((
            Place::return_place(),
            Rvalue::Aggregate(
                Box::new(kind),
                (0..variant.fields.len())
                    .map(|idx| Operand::Move(Place::from(Local::new(idx + 1))))
                    .collect(),
            ),
        ))),
    );

    let start_block = BasicBlockData::new_stmts(
        vec![statement],
        Some(Terminator { source_info, kind: TerminatorKind::Return }),
        false,
    );

    let source = MirSource::item(ctor_id);
    let mut body = new_body(
        source,
        IndexVec::from_elem_n(start_block, 1),
        local_decls,
        sig.inputs().len(),
        span,
    );
    // A constructor doesn't mention any other items (and we don't run the usual optimization passes
    // so this would otherwise not get filled).
    body.set_mentioned_items(Vec::new());

    crate::pass_manager::dump_mir_for_phase_change(tcx, &body);

    body
}

/// ```ignore (pseudo-impl)
/// impl FnPtr for fn(u32) {
///     fn addr(self) -> usize {
///         self as usize
///     }
/// }
/// ```
fn build_fn_ptr_addr_shim<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId, self_ty: Ty<'tcx>) -> Body<'tcx> {
    assert_matches!(self_ty.kind(), ty::FnPtr(..), "expected fn ptr, found {self_ty}");
    let span = tcx.def_span(def_id);
    let Some(sig) = tcx.fn_sig(def_id).instantiate(tcx, &[self_ty.into()]).no_bound_vars() else {
        span_bug!(span, "FnPtr::addr with bound vars for `{self_ty}`");
    };
    let locals = local_decls_for_sig(&sig, span);

    let source_info = SourceInfo::outermost(span);
    // FIXME: use `expose_provenance` once we figure out whether function pointers have meaningful
    // provenance.
    let rvalue = Rvalue::Cast(
        CastKind::FnPtrToPtr,
        Operand::Move(Place::from(Local::new(1))),
        Ty::new_imm_ptr(tcx, tcx.types.unit),
    );
    let stmt = Statement::new(
        source_info,
        StatementKind::Assign(Box::new((Place::return_place(), rvalue))),
    );
    let statements = vec![stmt];
    let start_block = BasicBlockData::new_stmts(
        statements,
        Some(Terminator { source_info, kind: TerminatorKind::Return }),
        false,
    );
    let source = MirSource::from_instance(ty::InstanceKind::FnPtrAddrShim(def_id, self_ty));
    new_body(source, IndexVec::from_elem_n(start_block, 1), locals, sig.inputs().len(), span)
}

fn build_construct_coroutine_by_move_shim<'tcx>(
    tcx: TyCtxt<'tcx>,
    coroutine_closure_def_id: DefId,
    receiver_by_ref: bool,
) -> Body<'tcx> {
    let mut self_ty = tcx.type_of(coroutine_closure_def_id).instantiate_identity();
    let mut self_local: Place<'tcx> = Local::from_usize(1).into();
    let ty::CoroutineClosure(_, args) = *self_ty.kind() else {
        bug!();
    };

    // We use `&Self` here because we only need to emit an ABI-compatible shim body,
    // rather than match the signature exactly (which might take `&mut self` instead).
    //
    // We adjust the `self_local` to be a deref since we want to copy fields out of
    // a reference to the closure.
    if receiver_by_ref {
        self_local = tcx.mk_place_deref(self_local);
        self_ty = Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, self_ty);
    }

    let poly_sig = args.as_coroutine_closure().coroutine_closure_sig().map_bound(|sig| {
        tcx.mk_fn_sig(
            [self_ty].into_iter().chain(sig.tupled_inputs_ty.tuple_fields()),
            sig.to_coroutine_given_kind_and_upvars(
                tcx,
                args.as_coroutine_closure().parent_args(),
                tcx.coroutine_for_closure(coroutine_closure_def_id),
                ty::ClosureKind::FnOnce,
                tcx.lifetimes.re_erased,
                args.as_coroutine_closure().tupled_upvars_ty(),
                args.as_coroutine_closure().coroutine_captures_by_ref_ty(),
            ),
            sig.c_variadic,
            sig.safety,
            sig.abi,
        )
    });
    let sig = tcx.liberate_late_bound_regions(coroutine_closure_def_id, poly_sig);
    let ty::Coroutine(coroutine_def_id, coroutine_args) = *sig.output().kind() else {
        bug!();
    };

    let span = tcx.def_span(coroutine_closure_def_id);
    let locals = local_decls_for_sig(&sig, span);

    let mut fields = vec![];

    // Move all of the closure args.
    for idx in 1..sig.inputs().len() {
        fields.push(Operand::Move(Local::from_usize(idx + 1).into()));
    }

    for (idx, ty) in args.as_coroutine_closure().upvar_tys().iter().enumerate() {
        if receiver_by_ref {
            // The only situation where it's possible is when we capture immuatable references,
            // since those don't need to be reborrowed with the closure's env lifetime. Since
            // references are always `Copy`, just emit a copy.
            if !matches!(ty.kind(), ty::Ref(_, _, hir::Mutability::Not)) {
                // This copy is only sound if it's a `&T`. This may be
                // reachable e.g. when eagerly computing the `Fn` instance
                // of an async closure that doesn't borrowck.
                tcx.dcx().delayed_bug(format!(
                    "field should be captured by immutable ref if we have \
                    an `Fn` instance, but it was: {ty}"
                ));
            }
            fields.push(Operand::Copy(tcx.mk_place_field(
                self_local,
                FieldIdx::from_usize(idx),
                ty,
            )));
        } else {
            fields.push(Operand::Move(tcx.mk_place_field(
                self_local,
                FieldIdx::from_usize(idx),
                ty,
            )));
        }
    }

    let source_info = SourceInfo::outermost(span);
    let rvalue = Rvalue::Aggregate(
        Box::new(AggregateKind::Coroutine(coroutine_def_id, coroutine_args)),
        IndexVec::from_raw(fields),
    );
    let stmt = Statement::new(
        source_info,
        StatementKind::Assign(Box::new((Place::return_place(), rvalue))),
    );
    let statements = vec![stmt];
    let start_block = BasicBlockData::new_stmts(
        statements,
        Some(Terminator { source_info, kind: TerminatorKind::Return }),
        false,
    );

    let source = MirSource::from_instance(ty::InstanceKind::ConstructCoroutineInClosureShim {
        coroutine_closure_def_id,
        receiver_by_ref,
    });

    let body =
        new_body(source, IndexVec::from_elem_n(start_block, 1), locals, sig.inputs().len(), span);

    let pass_name =
        if receiver_by_ref { "coroutine_closure_by_ref" } else { "coroutine_closure_by_move" };
    if let Some(dumper) = MirDumper::new(tcx, pass_name, &body) {
        dumper.dump_mir(&body);
    }

    body
}

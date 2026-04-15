use std::assert_matches;

use itertools::Itertools as _;
use rustc_abi::{self as abi, BackendRepr, FIRST_VARIANT, FieldIdx, Size};
use rustc_middle::ty::adjustment::PointerCoercion;
use rustc_middle::ty::layout::{HasTyCtxt, LayoutOf, TyAndLayout};
use rustc_middle::ty::{self, Instance, Ty, TyCtxt};
use rustc_middle::{bug, mir, span_bug};
use rustc_session::config::OptLevel;
use tracing::{debug, instrument};

use super::FunctionCx;
use super::operand::{OperandRef, OperandRefBuilder, OperandValue};
use super::place::{PlaceRef, PlaceValue, codegen_tag_value};
use crate::common::TypeKind;
use crate::mir::{AnyPlaceMeta, PlaceMetadata};
use crate::traits::*;
use crate::{MemFlags, base, size_of_val};

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    #[instrument(level = "trace", skip(self, bx))]
    pub(crate) fn codegen_rvalue(
        &mut self,
        bx: &mut Bx,
        dest: PlaceRef<'tcx, Bx::Value>,
        rvalue: &mir::Rvalue<'tcx>,
    ) {
        match *rvalue {
            mir::Rvalue::Use(ref operand) => {
                if let mir::Operand::Constant(const_op) = operand {
                    let val = self.eval_mir_constant(&const_op);
                    if val.all_bytes_uninit(self.cx.tcx()) {
                        return;
                    }
                }
                let cg_operand = self.codegen_operand(bx, operand);
                // Crucially, we do *not* use `OperandValue::Ref` for types with
                // `BackendRepr::Scalar | BackendRepr::ScalarPair`. This ensures we match the MIR
                // semantics regarding when assignment operators allow overlap of LHS and RHS.
                if matches!(
                    cg_operand.layout.backend_repr,
                    BackendRepr::Scalar(..) | BackendRepr::ScalarPair(..),
                ) {
                    debug_assert!(!matches!(cg_operand.val, OperandValue::Ref(..)));
                }
                // FIXME: consider not copying constants through stack. (Fixable by codegen'ing
                // constants into `OperandValue::Ref`; why don’t we do that yet if we don’t?)
                cg_operand.store_with_annotation(bx, dest);
            }

            mir::Rvalue::Cast(mir::CastKind::PtrToPtr, ref source, _) => {
                // The destination could be a thin or single-wide pointer,
                // for which codegen_rvalue_operand can handle it.
                if !bx.cx().is_backend_ref(dest.layout) {
                    // Into-coerce of something small enough for an operand -- just
                    // use the operand path.
                    let temp = self.codegen_rvalue_operand(bx, rvalue);
                    temp.store_with_annotation(bx, dest);
                    return;
                }
                // If the destination is *not* a thin pointer, then this must be a cast between
                // compatible metadata, which is just a transmute.

                let operand = self.codegen_operand(bx, source);
                assert_eq!(dest.layout.size, operand.layout.size);
                self.codegen_transmute(bx, operand, dest);
            }

            mir::Rvalue::Cast(mir::CastKind::PtrMetadataToPtrMetadata, ref source, _) => {
                // The destination could be a thin, single-, or double-wide wide metadata,
                // for which codegen_rvalue_operand can handle it.
                if !bx.cx().is_backend_ref(dest.layout) {
                    // Into-coerce of something small enough for an operand -- just
                    // use the operand path.
                    let temp = self.codegen_rvalue_operand(bx, rvalue);
                    temp.store_with_annotation(bx, dest);
                    return;
                }
                // If the destination is *not* a thin metadata, then this must be a cast between
                // compatible metadata, which is just a transmute.

                let operand = self.codegen_operand(bx, source);
                assert_eq!(dest.layout.size, operand.layout.size);
                self.codegen_transmute(bx, operand, dest);
            }

            mir::Rvalue::Cast(
                mir::CastKind::PointerCoercion(PointerCoercion::Unsize, _),
                ref source,
                _,
            ) => {
                // The destination necessarily contains a pointer metadata,
                // but it could be a nontrivial struct or a multi-wide metadata
                // even if it's a scalar pair, not necessarily just a wide pointer
                // or newtype thereof.
                if bx.cx().is_backend_scalar_pair(dest.layout) {
                    // Into-coerce of something small enough for an operand -- just
                    // use the operand path.
                    let temp = self.codegen_rvalue_operand(bx, rvalue);
                    temp.store_with_annotation(bx, dest);
                    return;
                }

                // Unsize into a non-trivial struct, or a multi-wide pointer.
                let operand = self.codegen_operand(bx, source);
                match operand.val {
                    OperandValue::Pair(..)
                    | OperandValue::Immediate(_)
                    | OperandValue::ZeroSized => {
                        // Unsize from an immediate structure. We don't
                        // really need a temporary alloca here, but
                        // avoiding it would require us to have
                        // `coerce_unsized_into` use `extractvalue` to
                        // index into the struct, and this case isn't
                        // important enough for it.
                        debug!("codegen_rvalue: creating ugly alloca");
                        let scratch = PlaceRef::alloca(bx, operand.layout);
                        scratch.storage_live(bx);
                        operand.store_with_annotation(bx, scratch);
                        base::coerce_unsized_into(bx, scratch, dest);
                        scratch.storage_dead(bx);
                    }
                    OperandValue::Ref(val) => {
                        if val.llextra.has_metadata() {
                            bug!("unsized coercion on an unsized rvalue");
                        }
                        base::coerce_unsized_into(bx, val.with_type(operand.layout), dest);
                    }
                }
            }

            mir::Rvalue::Cast(
                mir::CastKind::Transmute | mir::CastKind::Subtype,
                ref operand,
                _ty,
            ) => {
                let src = self.codegen_operand(bx, operand);
                self.codegen_transmute(bx, src, dest);
            }

            mir::Rvalue::Repeat(ref elem, count) => {
                // Do not generate the loop for zero-sized elements or empty arrays.
                if dest.layout.is_zst() {
                    return;
                }

                // When the element is a const with all bytes uninit, emit a single memset that
                // writes undef to the entire destination.
                if let mir::Operand::Constant(const_op) = elem {
                    let val = self.eval_mir_constant(const_op);
                    if val.all_bytes_uninit(self.cx.tcx()) {
                        let size = bx.const_usize(dest.layout.size.bytes());
                        bx.memset(
                            dest.val.llval,
                            bx.const_undef(bx.type_i8()),
                            size,
                            dest.val.align,
                            MemFlags::empty(),
                        );
                        return;
                    }
                }

                let cg_elem = self.codegen_operand(bx, elem);

                let try_init_all_same = |bx: &mut Bx, v| {
                    let start = dest.val.llval;
                    let size = bx.const_usize(dest.layout.size.bytes());

                    // Use llvm.memset.p0i8.* to initialize all same byte arrays
                    if let Some(int) = bx.cx().const_to_opt_u128(v, false)
                        && let bytes = &int.to_le_bytes()[..cg_elem.layout.size.bytes_usize()]
                        && let Ok(&byte) = bytes.iter().all_equal_value()
                    {
                        let fill = bx.cx().const_u8(byte);
                        bx.memset(start, fill, size, dest.val.align, MemFlags::empty());
                        return true;
                    }

                    // Use llvm.memset.p0i8.* to initialize byte arrays
                    let v = bx.from_immediate(v);
                    if bx.cx().val_ty(v) == bx.cx().type_i8() {
                        bx.memset(start, v, size, dest.val.align, MemFlags::empty());
                        return true;
                    }
                    false
                };

                if let OperandValue::Immediate(v) = cg_elem.val
                    && try_init_all_same(bx, v)
                {
                    return;
                }

                let count = self
                    .monomorphize(count)
                    .try_to_target_usize(bx.tcx())
                    .expect("expected monomorphic const in codegen");

                bx.write_operand_repeatedly(cg_elem, count, dest);
            }

            mir::Rvalue::Aggregate(ref kind, ref operands) => {
                let (variant_index, variant_dest, active_field_index) = match **kind {
                    mir::AggregateKind::Adt(_, variant_index, _, _, active_field_index) => {
                        let variant_dest = dest.project_downcast(bx, variant_index);
                        (variant_index, variant_dest, active_field_index)
                    }
                    _ => (FIRST_VARIANT, dest, None),
                };
                if active_field_index.is_some() {
                    assert_eq!(operands.len(), 1);
                }
                for (i, operand) in operands.iter_enumerated() {
                    let op = self.codegen_operand(bx, operand);
                    // Do not generate stores and GEPis for zero-sized fields.
                    if !op.layout.is_zst() {
                        let field_index = active_field_index.unwrap_or(i);
                        let field = if let mir::AggregateKind::Array(_) = **kind {
                            let llindex = bx.cx().const_usize(field_index.as_u32().into());
                            variant_dest.project_index(Some(self), bx, llindex)
                        } else {
                            variant_dest.project_field(Some(self), bx, field_index.as_usize())
                        };
                        op.store_with_annotation(bx, field);
                    }
                }
                dest.codegen_set_discr(bx, variant_index);
            }

            mir::Rvalue::BinaryOp(mir::BinOp::Offset, box (ref lhs, ref rhs)) => {
                // If this is a thin or single-wide pointer, then codegen_rvalue_operand can handle it.
                if !bx.cx().is_backend_ref(dest.layout) {
                    // Offset of a thin or single-wide pointer small enough for an operand -- just
                    // use the operand path.
                    let temp = self.codegen_rvalue_operand(bx, rvalue);
                    temp.store_with_annotation(bx, dest);
                    return;
                }
                // If the destination is *not* a thin pointer, then this must be an offset
                // of an unsized type

                let lhs = self.codegen_operand(bx, lhs);
                let rhs = self.codegen_operand(bx, rhs);
                let lhs_ty = lhs.layout.ty;
                let rhs_ty = rhs.layout.ty;

                let base_data_ptr = lhs.extract_or_load_field(bx, 0);
                let OperandValue::Immediate(base_data_ptr_val) = base_data_ptr.val else {
                    unreachable!("untyped pointer must be OperandValue::Immediate")
                };
                let meta = lhs.extract_or_load_field(bx, 1);

                let OperandValue::Immediate(rhs) = rhs.val else {
                    unreachable!("Offset second argument must be usize or isize")
                };

                let pointee_ty = lhs_ty
                    .builtin_deref(true)
                    .unwrap_or_else(|| bug!("deref of non-pointer {:?}", lhs_ty));
                let pointee_layout = bx.cx().layout_of(pointee_ty);
                assert!(
                    !pointee_layout.is_sized(),
                    "type with metadata had sized layout? ty = {pointee_ty:?}, layout = {pointee_layout:?}"
                );
                let (pointee_size, _pointee_align) = size_of_val::size_and_align_of_dst(
                    Some(self),
                    bx,
                    pointee_ty,
                    AnyPlaceMeta(Some(meta.expect_sized("pointer metadata must be sized"))),
                );

                // FIXME(more_unsized): make this the right kind of unchecked mul
                let offset = bx.mul(pointee_size, rhs);

                let llty = bx.cx().type_i8();
                let result_data_ptr_val = if !rhs_ty.is_signed() {
                    bx.inbounds_nuw_gep(llty, base_data_ptr_val, &[offset])
                } else {
                    bx.inbounds_gep(llty, base_data_ptr_val, &[offset])
                };

                let result_data_ptr = OperandRef::from_immediate_or_packed_pair(
                    bx,
                    result_data_ptr_val,
                    base_data_ptr.layout,
                );

                let dest_data_ptr = dest.project_field(None, bx, 0);
                let dest_meta = dest.project_field(None, bx, 1);

                result_data_ptr.store_with_annotation(bx, dest_data_ptr);
                meta.store_with_annotation(bx, dest_meta);
            }

            _ => {
                let temp = self.codegen_rvalue_operand(bx, rvalue);
                temp.store_with_annotation(bx, dest);
            }
        }
    }

    /// Transmutes the `src` value to the destination type by writing it to `dst`.
    ///
    /// See also [`Self::codegen_transmute_operand`] for cases that can be done
    /// without needing a pre-allocated place for the destination.
    fn codegen_transmute(
        &mut self,
        bx: &mut Bx,
        src: OperandRef<'tcx, Bx::Value>,
        dst: PlaceRef<'tcx, Bx::Value>,
    ) {
        // The MIR validator enforces no unsized transmutes.
        assert!(src.layout.is_sized());
        assert!(dst.layout.is_sized());

        if src.layout.size != dst.layout.size
            || src.layout.is_uninhabited()
            || dst.layout.is_uninhabited()
        {
            // These cases are all UB to actually hit, so don't emit code for them.
            // (The size mismatches are reachable via `transmute_unchecked`.)
            bx.unreachable_nonterminator();
        } else {
            // Since in this path we have a place anyway, we can store or copy to it,
            // making sure we use the destination place's alignment even if the
            // source would normally have a higher one.
            src.store_with_annotation(bx, dst.val.with_type(src.layout));
        }
    }

    /// Transmutes an `OperandValue` to another `OperandValue`.
    ///
    /// This is supported for all cases where the `cast` type is SSA,
    /// but for non-ZSTs with [`abi::BackendRepr::Memory`] it ICEs.
    pub(crate) fn codegen_transmute_operand(
        &mut self,
        bx: &mut Bx,
        operand: OperandRef<'tcx, Bx::Value>,
        cast: TyAndLayout<'tcx>,
    ) -> OperandValue<'tcx, Bx::Value> {
        if let abi::BackendRepr::Memory { .. } = cast.backend_repr
            && !cast.is_zst()
        {
            span_bug!(self.mir.span, "Use `codegen_transmute` to transmute to {cast:?}");
        }

        // `Layout` is interned, so we can do a cheap check for things that are
        // exactly the same and thus don't need any handling.
        if abi::Layout::eq(&operand.layout.layout, &cast.layout) {
            return operand.val;
        }

        // Check for transmutes that are always UB.
        if operand.layout.size != cast.size
            || operand.layout.is_uninhabited()
            || cast.is_uninhabited()
        {
            bx.unreachable_nonterminator();

            // We still need to return a value of the appropriate type, but
            // it's already UB so do the easiest thing available.
            return OperandValue::poison(bx, cast);
        }

        // To or from pointers takes different methods, so we use this to restrict
        // the SimdVector case to types which can be `bitcast` between each other.
        #[inline]
        fn vector_can_bitcast(x: abi::Scalar) -> bool {
            matches!(
                x,
                abi::Scalar::Initialized {
                    value: abi::Primitive::Int(..) | abi::Primitive::Float(..),
                    ..
                }
            )
        }

        let cx = bx.cx();
        match (operand.val, operand.layout.backend_repr, cast.backend_repr) {
            _ if cast.is_zst() => OperandValue::ZeroSized,
            (OperandValue::Ref(source_place_val), abi::BackendRepr::Memory { .. }, _) => {
                assert!(!source_place_val.llextra.has_metadata());
                // The existing alignment is part of `source_place_val`,
                // so that alignment will be used, not `cast`'s.
                bx.load_operand(source_place_val.with_type(cast)).val
            }
            (
                OperandValue::Immediate(imm),
                abi::BackendRepr::Scalar(from_scalar),
                abi::BackendRepr::Scalar(to_scalar),
            ) if from_scalar.size(cx) == to_scalar.size(cx) => {
                OperandValue::Immediate(transmute_scalar(bx, imm, from_scalar, to_scalar))
            }
            (
                OperandValue::Immediate(imm),
                abi::BackendRepr::SimdVector { element: from_scalar, .. },
                abi::BackendRepr::SimdVector { element: to_scalar, .. },
            ) if vector_can_bitcast(from_scalar) && vector_can_bitcast(to_scalar) => {
                let to_backend_ty = bx.cx().immediate_backend_type(cast);
                OperandValue::Immediate(bx.bitcast(imm, to_backend_ty))
            }
            (
                OperandValue::Pair(imm_a, imm_b),
                abi::BackendRepr::ScalarPair(in_a, in_b),
                abi::BackendRepr::ScalarPair(out_a, out_b),
            ) if in_a.size(cx) == out_a.size(cx) && in_b.size(cx) == out_b.size(cx) => {
                OperandValue::Pair(
                    transmute_scalar(bx, imm_a, in_a, out_a),
                    transmute_scalar(bx, imm_b, in_b, out_b),
                )
            }
            _ => {
                // For any other potentially-tricky cases, make a temporary instead.
                // If anything else wants the target local to be in memory this won't
                // be hit, as `codegen_transmute` will get called directly. Thus this
                // is only for places where everything else wants the operand form,
                // and thus it's not worth making those places get it from memory.
                //
                // Notably, Scalar ⇌ ScalarPair cases go here to avoid padding
                // and endianness issues, as do SimdVector ones to avoid worrying
                // about things like f32x8 ⇌ ptrx4 that would need multiple steps.
                let align = Ord::max(operand.layout.align.abi, cast.align.abi);
                let size = Ord::max(operand.layout.size, cast.size);
                let temp = PlaceValue::alloca(bx, size, align);
                bx.lifetime_start(temp.llval, size);
                operand.store_with_annotation(bx, temp.with_type(operand.layout));
                let val = bx.load_operand(temp.with_type(cast)).val;
                bx.lifetime_end(temp.llval, size);
                val
            }
        }
    }

    /// Cast one of the immediates from an [`OperandValue::Immediate`]
    /// or an [`OperandValue::Pair`] to an immediate of the target type.
    ///
    /// Returns `None` if the cast is not possible.
    fn cast_immediate(
        &self,
        bx: &mut Bx,
        mut imm: Bx::Value,
        from_scalar: abi::Scalar,
        from_backend_ty: Bx::Type,
        to_scalar: abi::Scalar,
        to_backend_ty: Bx::Type,
    ) -> Option<Bx::Value> {
        use abi::Primitive::*;

        // When scalars are passed by value, there's no metadata recording their
        // valid ranges. For example, `char`s are passed as just `i32`, with no
        // way for LLVM to know that they're 0x10FFFF at most. Thus we assume
        // the range of the input value too, not just the output range.
        assume_scalar_range(bx, imm, from_scalar, from_backend_ty, None);

        imm = match (from_scalar.primitive(), to_scalar.primitive()) {
            (Int(_, is_signed), Int(..)) => bx.intcast(imm, to_backend_ty, is_signed),
            (Float(_), Float(_)) => {
                let srcsz = bx.cx().float_width(from_backend_ty);
                let dstsz = bx.cx().float_width(to_backend_ty);
                if dstsz > srcsz {
                    bx.fpext(imm, to_backend_ty)
                } else if srcsz > dstsz {
                    bx.fptrunc(imm, to_backend_ty)
                } else {
                    imm
                }
            }
            (Int(_, is_signed), Float(_)) => {
                if is_signed {
                    bx.sitofp(imm, to_backend_ty)
                } else {
                    bx.uitofp(imm, to_backend_ty)
                }
            }
            (Pointer(..), Pointer(..)) => bx.pointercast(imm, to_backend_ty),
            (Int(_, is_signed), Pointer(..)) => {
                let usize_imm = bx.intcast(imm, bx.cx().type_isize(), is_signed);
                bx.inttoptr(usize_imm, to_backend_ty)
            }
            (Float(_), Int(_, is_signed)) => bx.cast_float_to_int(is_signed, imm, to_backend_ty),
            _ => return None,
        };
        Some(imm)
    }

    pub(crate) fn codegen_rvalue_operand(
        &mut self,
        bx: &mut Bx,
        rvalue: &mir::Rvalue<'tcx>,
    ) -> OperandRef<'tcx, Bx::Value> {
        match *rvalue {
            mir::Rvalue::Cast(ref kind, ref source, mir_cast_ty) => {
                let operand = self.codegen_operand(bx, source);
                debug!("cast operand is {:?}", operand);
                let cast = bx.cx().layout_of(self.monomorphize(mir_cast_ty));

                let val = match *kind {
                    mir::CastKind::PointerExposeProvenance => {
                        assert!(bx.cx().is_backend_immediate(cast));
                        let llptr = operand.immediate();
                        let llcast_ty = bx.cx().immediate_backend_type(cast);
                        let lladdr = bx.ptrtoint(llptr, llcast_ty);
                        OperandValue::Immediate(lladdr)
                    }
                    mir::CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer(_), _) => {
                        match *operand.layout.ty.kind() {
                            ty::FnDef(def_id, args) => {
                                let instance = ty::Instance::resolve_for_fn_ptr(
                                    bx.tcx(),
                                    bx.typing_env(),
                                    def_id,
                                    args,
                                )
                                .unwrap();
                                OperandValue::Immediate(bx.get_fn_addr(instance))
                            }
                            _ => bug!("{} cannot be reified to a fn ptr", operand.layout.ty),
                        }
                    }
                    mir::CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer(_), _) => {
                        match *operand.layout.ty.kind() {
                            ty::Closure(def_id, args) => {
                                let instance = Instance::resolve_closure(
                                    bx.cx().tcx(),
                                    def_id,
                                    args,
                                    ty::ClosureKind::FnOnce,
                                );
                                OperandValue::Immediate(bx.cx().get_fn_addr(instance))
                            }
                            _ => bug!("{} cannot be cast to a fn ptr", operand.layout.ty),
                        }
                    }
                    mir::CastKind::PointerCoercion(PointerCoercion::UnsafeFnPointer, _) => {
                        // This is a no-op at the LLVM level.
                        operand.val
                    }
                    mir::CastKind::PointerCoercion(PointerCoercion::Unsize, _) => {
                        if !bx.cx().is_backend_ref(cast) {
                            return self.codegen_coerce_unsized_into_operand(bx, operand, cast);
                        } else {
                            // I think this should be unreachable. `codegen_rvalue_operand`
                            // is only called for things that can be operands, right?
                            tracing::warn!("FIXME(ptr_metadata_v2): This is unreachable, right?");
                            let scratch = PlaceRef::alloca(bx, operand.layout);
                            let result = PlaceRef::alloca(bx, cast);
                            scratch.storage_live(bx);
                            result.storage_live(bx);
                            operand.val.store(bx, scratch);
                            base::coerce_unsized_into(bx, scratch, result.clone());
                            scratch.storage_dead(bx);
                            return bx.load_operand(result);
                        }
                    }
                    mir::CastKind::PointerCoercion(
                        PointerCoercion::MutToConstPointer | PointerCoercion::ArrayToPointer, _
                    ) => {
                        bug!("{kind:?} is for borrowck, and should never appear in codegen");
                    }
                    mir::CastKind::PtrToPtr => {
                        if cast.layout.size < operand.layout.size {
                            // Cast of wide-ptr to thin-ptr is an extraction of data-ptr
                            assert_eq!(cast.layout.size, bx.data_layout().pointer_size());
                            if let OperandValue::Ref(place_value) = operand.val {
                                let place_ref = PlaceRef{val:place_value, layout:operand.layout};
                                let data_ref = place_ref.project_field(None,bx, 0);
                                bx.load_operand(data_ref).val
                            } else {
                                operand.extract_field(self, bx, 0).val
                            }
                        } else {
                            // Otherwise transmute compatible metadata
                            self.codegen_transmute_operand(bx, operand, cast)
                        }
                    }
                    mir::CastKind::PtrMetadataToPtrMetadata => {
                        if cast.layout.size < operand.layout.size {
                            // Cast of wide-ptr-metadata to thin-ptr-metadata just discards input
                            // and returns zst thin metadata
                            assert_eq!(cast.layout.size, Size::ZERO);
                            OperandValue::ZeroSized
                        } else {
                            // Otherwise transmute compatible metadata
                            self.codegen_transmute_operand(bx, operand, cast)
                        }
                    }
                    | mir::CastKind::IntToInt
                    | mir::CastKind::FloatToInt
                    | mir::CastKind::FloatToFloat
                    | mir::CastKind::IntToFloat
                    | mir::CastKind::FnPtrToPtr
                    // Since int2ptr can have arbitrary integer types as input (so we have to do
                    // sign extension and all that), it is currently best handled in the same code
                    // path as the other integer-to-X casts.
                    | mir::CastKind::PointerWithExposedProvenance => {
                        let imm = operand.immediate();
                        let abi::BackendRepr::Scalar(from_scalar) = operand.layout.backend_repr else {
                            bug!("Found non-scalar for operand {operand:?}");
                        };
                        let from_backend_ty = bx.cx().immediate_backend_type(operand.layout);

                        assert!(bx.cx().is_backend_immediate(cast));
                        let to_backend_ty = bx.cx().immediate_backend_type(cast);
                        if operand.layout.is_uninhabited() {
                            let val = OperandValue::Immediate(bx.cx().const_poison(to_backend_ty));
                            return OperandRef { val, layout: cast, move_annotation: None };
                        }
                        let abi::BackendRepr::Scalar(to_scalar) = cast.layout.backend_repr else {
                            bug!("Found non-scalar for cast {cast:?}");
                        };

                        self.cast_immediate(bx, imm, from_scalar, from_backend_ty, to_scalar, to_backend_ty)
                            .map(OperandValue::Immediate)
                            .unwrap_or_else(|| {
                                bug!("Unsupported cast of {operand:?} to {cast:?}");
                            })
                    }
                    mir::CastKind::Transmute | mir::CastKind::Subtype => {
                        self.codegen_transmute_operand(bx, operand, cast)
                    }
                };
                OperandRef { val, layout: cast, move_annotation: None }
            }

            mir::Rvalue::Ref(_, bk, place) => {
                let mk_ref = move |tcx: TyCtxt<'tcx>, ty: Ty<'tcx>| {
                    Ty::new_ref(tcx, tcx.lifetimes.re_erased, ty, bk.to_mutbl_lossy())
                };
                self.codegen_place_to_pointer(bx, place, mk_ref)
            }

            mir::Rvalue::RawPtr(kind, place) => {
                let mk_ptr = move |tcx: TyCtxt<'tcx>, ty: Ty<'tcx>| {
                    Ty::new_ptr(tcx, ty, kind.to_mutbl_lossy())
                };
                self.codegen_place_to_pointer(bx, place, mk_ptr)
            }

            mir::Rvalue::BinaryOp(op_with_overflow, box (ref lhs, ref rhs))
                if let Some(op) = op_with_overflow.overflowing_to_wrapping() =>
            {
                let lhs = self.codegen_operand(bx, lhs);
                let rhs = self.codegen_operand(bx, rhs);
                let result = self.codegen_scalar_checked_binop(
                    bx,
                    op,
                    lhs.immediate(),
                    rhs.immediate(),
                    lhs.layout.ty,
                );
                let val_ty = op.ty(bx.tcx(), lhs.layout.ty, rhs.layout.ty);
                let operand_ty = Ty::new_tup(bx.tcx(), &[val_ty, bx.tcx().types.bool]);
                OperandRef {
                    val: result,
                    layout: bx.cx().layout_of(operand_ty),
                    move_annotation: None,
                }
            }
            mir::Rvalue::BinaryOp(op, box (ref lhs, ref rhs)) => {
                let lhs = self.codegen_operand(bx, lhs);
                let rhs = self.codegen_operand(bx, rhs);

                let llresult = match (lhs.val, rhs.val) {
                    (OperandValue::Immediate(lhs_val), OperandValue::Immediate(rhs_val)) => self
                        .codegen_scalar_binop(
                            bx,
                            op,
                            lhs_val,
                            rhs_val,
                            lhs.layout.ty,
                            rhs.layout.ty,
                        ),

                    (OperandValue::Pair(lhs_ptr, meta), OperandValue::Immediate(rhs_val)) => {
                        assert_matches!(
                            op,
                            mir::BinOp::Offset,
                            "only binop that should have ScalarPair is Offset with a wide pointer"
                        );
                        let result_ptr = self.codegen_immediate_offset(
                            bx,
                            lhs_ptr,
                            Some(meta),
                            rhs_val,
                            lhs.layout.ty,
                            rhs.layout.ty,
                        );
                        return OperandRef {
                            val: OperandValue::Pair(result_ptr, meta),
                            layout: bx.cx().layout_of(op.ty(
                                bx.tcx(),
                                lhs.layout.ty,
                                rhs.layout.ty,
                            )),
                            move_annotation: None,
                        };
                    }

                    _ => unreachable!(
                        "wide pointer comparisons should be lowered to method calls: {op:?} {lhs:?} {rhs:?}"
                    ),
                };
                OperandRef {
                    val: OperandValue::Immediate(llresult),
                    layout: bx.cx().layout_of(op.ty(bx.tcx(), lhs.layout.ty, rhs.layout.ty)),
                    move_annotation: None,
                }
            }

            mir::Rvalue::UnaryOp(op, ref operand) => {
                let operand = self.codegen_operand(bx, operand);
                let is_float = operand.layout.ty.is_floating_point();
                let (val, layout) = match op {
                    mir::UnOp::Not => {
                        let llval = bx.not(operand.immediate());
                        (OperandValue::Immediate(llval), operand.layout)
                    }
                    mir::UnOp::Neg => {
                        let llval = if is_float {
                            bx.fneg(operand.immediate())
                        } else {
                            bx.neg(operand.immediate())
                        };
                        (OperandValue::Immediate(llval), operand.layout)
                    }
                };
                assert!(
                    val.is_expected_variant_for_type(self.cx, layout),
                    "Made wrong variant {val:?} for type {layout:?}",
                );
                OperandRef { val, layout, move_annotation: None }
            }

            mir::Rvalue::Discriminant(ref place) => {
                let discr_ty = rvalue.ty(self.mir, bx.tcx());
                let discr_ty = self.monomorphize(discr_ty);
                let operand = self.codegen_consume(bx, place.as_ref());
                let discr = operand.codegen_get_discr(self, bx, discr_ty);
                OperandRef {
                    val: OperandValue::Immediate(discr),
                    layout: self.cx.layout_of(discr_ty),
                    move_annotation: None,
                }
            }

            mir::Rvalue::ThreadLocalRef(def_id) => {
                assert!(bx.cx().tcx().is_static(def_id));
                let layout = bx.layout_of(bx.cx().tcx().static_ptr_ty(def_id, bx.typing_env()));
                let static_ = if !def_id.is_local() && bx.cx().tcx().needs_thread_local_shim(def_id)
                {
                    let instance = ty::Instance {
                        def: ty::InstanceKind::ThreadLocalShim(def_id),
                        args: ty::GenericArgs::empty(),
                    };
                    let fn_ptr = bx.get_fn_addr(instance);
                    let fn_abi = bx.fn_abi_of_instance(instance, ty::List::empty());
                    let fn_ty = bx.fn_decl_backend_type(fn_abi);
                    let fn_attrs = if bx.tcx().def_kind(instance.def_id()).has_codegen_attrs() {
                        Some(bx.tcx().codegen_instance_attrs(instance.def))
                    } else {
                        None
                    };
                    bx.call(
                        fn_ty,
                        fn_attrs.as_deref(),
                        Some(fn_abi),
                        fn_ptr,
                        &[],
                        None,
                        Some(instance),
                    )
                } else {
                    bx.get_static(def_id)
                };
                OperandRef { val: OperandValue::Immediate(static_), layout, move_annotation: None }
            }
            mir::Rvalue::Use(ref operand) => self.codegen_operand(bx, operand),
            mir::Rvalue::Repeat(ref elem, len_const) => {
                // All arrays have `BackendRepr::Memory`, so only the ZST cases
                // end up here. Anything else forces the destination local to be
                // `Memory`, and thus ends up handled in `codegen_rvalue` instead.
                let operand = self.codegen_operand(bx, elem);
                let array_ty = Ty::new_array_with_const_len(bx.tcx(), operand.layout.ty, len_const);
                let array_ty = self.monomorphize(array_ty);
                let array_layout = bx.layout_of(array_ty);
                assert!(array_layout.is_zst());
                OperandRef {
                    val: OperandValue::ZeroSized,
                    layout: array_layout,
                    move_annotation: None,
                }
            }
            mir::Rvalue::Aggregate(ref kind, ref fields) => {
                let (variant_index, active_field_index) = match **kind {
                    mir::AggregateKind::Adt(_, variant_index, _, _, active_field_index) => {
                        (variant_index, active_field_index)
                    }
                    _ => (FIRST_VARIANT, None),
                };

                let ty = rvalue.ty(self.mir, self.cx.tcx());
                let ty = self.monomorphize(ty);
                let layout = self.cx.layout_of(ty);

                let mut builder = OperandRefBuilder::new(layout);
                for (field_idx, field) in fields.iter_enumerated() {
                    let op = self.codegen_operand(bx, field);
                    let fi = active_field_index.unwrap_or(field_idx);
                    builder.insert_field(bx, variant_index, fi, op);
                }

                let tag_result = codegen_tag_value(self.cx, variant_index, layout);
                match tag_result {
                    Err(super::place::UninhabitedVariantError) => {
                        // Like codegen_set_discr we use a sound abort, but could
                        // potentially `unreachable` or just return the poison for
                        // more optimizability, if that turns out to be helpful.
                        bx.abort();
                        let val = OperandValue::poison(bx, layout);
                        OperandRef { val, layout, move_annotation: None }
                    }
                    Ok(maybe_tag_value) => {
                        if let Some((tag_field, tag_imm)) = maybe_tag_value {
                            builder.insert_imm(tag_field, tag_imm);
                        }
                        builder.build(bx.cx())
                    }
                }
            }
            mir::Rvalue::WrapUnsafeBinder(ref operand, binder_ty) => {
                let operand = self.codegen_operand(bx, operand);
                let binder_ty = self.monomorphize(binder_ty);
                let layout = bx.cx().layout_of(binder_ty);
                OperandRef { val: operand.val, layout, move_annotation: None }
            }
            mir::Rvalue::CopyForDeref(_) => bug!("`CopyForDeref` in codegen"),
        }
    }

    /// Coerces `src`, which is a reference to a value of type `src_ty`,
    /// to a value of type `dst_ty`, and returns it as an operand.
    ///
    /// Like [`crate::base::coerce_unsized_into`], but for immediate operands instead of places.
    ///
    /// The source and result must both not be `is_backend_ref`.
    pub(crate) fn codegen_coerce_unsized_into_operand(
        &mut self,
        bx: &mut Bx,
        src: OperandRef<'tcx, Bx::Value>,
        dst_layout: TyAndLayout<'tcx>,
    ) -> OperandRef<'tcx, Bx::Value> {
        let src_ty = src.layout.ty;
        let dst_ty = dst_layout.ty;
        debug!("coerce_unsized_into_operand: {src_ty:?} -> {dst_ty:?}");
        match (src_ty.kind(), dst_ty.kind()) {
            (&ty::Ref(..), &ty::Ref(..) | &ty::RawPtr(..)) | (&ty::RawPtr(..), &ty::RawPtr(..)) => {
                let src_data = src.extract_field(self, bx, 0);
                let src_extra = src.extract_field(self, bx, 1);

                let mut dst = OperandRefBuilder::new(dst_layout);

                dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ZERO, src_data);

                let dst_extra_layout = dst_layout.field(bx.cx(), 1);
                let unsized_extra =
                    self.codegen_coerce_unsized_into_operand(bx, src_extra, dst_extra_layout);
                dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ONE, unsized_extra);

                dst.build(bx.cx())
            }
            (&ty::PtrMetadata(src_pointee_ty), &ty::PtrMetadata(dst_pointee_ty)) => {
                match (src_pointee_ty.kind(), dst_pointee_ty.kind()) {
                    (ty::Slice(..), ty::Slice(..)) => {
                        let mut dst = OperandRefBuilder::new(dst_layout);
                        // Unsize the element type, keep the length
                        let src_len = src.extract_field(self, bx, 0);
                        dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ZERO, src_len);

                        let src_elem = src.extract_field(self, bx, 1);
                        let dst_elem_meta_layout = dst_layout.field(bx.cx(), 1);
                        let unsized_elem = self.codegen_coerce_unsized_into_operand(
                            bx,
                            src_elem,
                            dst_elem_meta_layout,
                        );
                        dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ONE, unsized_elem);

                        dst.build(bx.cx())
                    }
                    (ty::Array(..), ty::Array(..)) => {
                        let mut dst = OperandRefBuilder::new(dst_layout);
                        // Unsize the element type
                        let src_elem = src.extract_field(self, bx, 0);
                        let dst_elem_meta_layout = dst_layout.field(bx.cx(), 0);
                        let unsized_elem = self.codegen_coerce_unsized_into_operand(
                            bx,
                            src_elem,
                            dst_elem_meta_layout,
                        );
                        dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ZERO, unsized_elem);

                        dst.build(bx.cx())
                    }
                    (ty::Array(_, len), ty::Slice(..)) => {
                        let cx = bx.cx();
                        // Unsize array to slice, possibly unsize element type
                        let src_len = cx.const_usize(
                            len.try_to_target_usize(cx.tcx())
                                .expect("expected monomorphic const in codegen"),
                        );
                        let src_len = OperandRef {
                            val: OperandValue::Immediate(src_len),
                            layout: bx.layout_of(bx.tcx().types.usize),
                            move_annotation: None,
                        };
                        let mut dst = OperandRefBuilder::new(dst_layout);
                        dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ZERO, src_len);

                        let src_elem = src.extract_field(self, bx, 0);
                        let dst_elem_meta_layout = dst_layout.field(bx.cx(), 1);
                        let elem_op = if src_elem.layout == dst_elem_meta_layout {
                            src_elem
                        } else {
                            self.codegen_coerce_unsized_into_operand(
                                bx,
                                src_elem,
                                dst_elem_meta_layout,
                            )
                        };

                        dst.insert_field(bx, FIRST_VARIANT, FieldIdx::ONE, elem_op);

                        dst.build(bx.cx())
                    }
                    (_, &ty::Dynamic(..)) => {
                        let old_info = match src.val {
                            OperandValue::Immediate(old_info) => Some(old_info),
                            OperandValue::ZeroSized => None,
                            _ => bug!("invalid vtable ptr {src:?}"),
                        };
                        let new_info =
                            base::dyn_unsize_info(bx, src_pointee_ty, dst_pointee_ty, old_info);

                        OperandRef {
                            val: OperandValue::Immediate(new_info),
                            layout: dst_layout,
                            move_annotation: None,
                        }
                    }
                    (&ty::Adt(def_a, _), &ty::Adt(def_b, _)) => {
                        assert_eq!(def_a, def_b); // implies same number of metadata fields

                        let mut dst = OperandRefBuilder::new(dst_layout);

                        for i in def_a.variant(FIRST_VARIANT).fields.indices() {
                            let src_f = src.extract_field(self, bx, i.as_usize());
                            let dst_f_layout = dst_layout.field(bx, i.as_usize());

                            let dst_f_op = if src_f.layout.ty == dst_f_layout.ty {
                                src_f
                            } else {
                                self.codegen_coerce_unsized_into_operand(bx, src_f, dst_f_layout)
                            };
                            dst.insert_field(bx, FIRST_VARIANT, i, dst_f_op);
                        }

                        dst.build(bx.cx())
                    }
                    _ => bug!(
                        "unsized_info: invalid unsizing {:?} -> {:?}",
                        src_pointee_ty,
                        dst_pointee_ty
                    ),
                }
            }

            (&ty::Adt(def_a, _), &ty::Adt(def_b, _)) => {
                assert_eq!(def_a, def_b); // implies same number of fields

                let mut dst = OperandRefBuilder::new(dst_layout);

                for i in def_a.variant(FIRST_VARIANT).fields.indices() {
                    let src_f = src.extract_field(self, bx, i.as_usize());
                    let dst_f_layout = dst_layout.field(bx, i.as_usize());

                    let dst_f_op = if src_f.layout.ty == dst_f_layout.ty {
                        src_f
                    } else {
                        self.codegen_coerce_unsized_into_operand(bx, src_f, dst_f_layout)
                    };
                    dst.insert_field(bx, FIRST_VARIANT, i, dst_f_op);
                }

                dst.build(bx.cx())
            }
            _ => {
                bug!("coerce_unsized_into_operand: invalid coercion {:?} -> {:?}", src_ty, dst_ty,)
            }
        }
    }

    /// Codegen an `Rvalue::RawPtr` or `Rvalue::Ref`
    fn codegen_place_to_pointer(
        &mut self,
        bx: &mut Bx,
        place: mir::Place<'tcx>,
        mk_ptr_ty: impl FnOnce(TyCtxt<'tcx>, Ty<'tcx>) -> Ty<'tcx>,
    ) -> OperandRef<'tcx, Bx::Value> {
        let cg_place = self.codegen_place(bx, place.as_ref());
        let ptr = cg_place.address(bx, mk_ptr_ty);

        // FIXME(ptr_metadata_v2): re-add some form of this check
        // let ty = cg_place.layout.ty;
        // assert!(
        //     if bx.cx().tcx().type_has_metadata(ty, bx.cx().typing_env()) {
        //         matches!(val, OperandValue::Pair(..))
        //     } else {
        //         matches!(val, OperandValue::Immediate(..))
        //     },
        //     "Address of place was unexpectedly {val:?} for pointee type {ty:?}",
        // );

        ptr
    }

    fn codegen_scalar_binop(
        &mut self,
        bx: &mut Bx,
        op: mir::BinOp,
        lhs: Bx::Value,
        rhs: Bx::Value,
        lhs_ty: Ty<'tcx>,
        rhs_ty: Ty<'tcx>,
    ) -> Bx::Value {
        let is_float = lhs_ty.is_floating_point();
        let is_signed = lhs_ty.is_signed();
        match op {
            mir::BinOp::Add => {
                if is_float {
                    bx.fadd(lhs, rhs)
                } else {
                    bx.add(lhs, rhs)
                }
            }
            mir::BinOp::AddUnchecked => {
                if is_signed {
                    bx.unchecked_sadd(lhs, rhs)
                } else {
                    bx.unchecked_uadd(lhs, rhs)
                }
            }
            mir::BinOp::Sub => {
                if is_float {
                    bx.fsub(lhs, rhs)
                } else {
                    bx.sub(lhs, rhs)
                }
            }
            mir::BinOp::SubUnchecked => {
                if is_signed {
                    bx.unchecked_ssub(lhs, rhs)
                } else {
                    bx.unchecked_usub(lhs, rhs)
                }
            }
            mir::BinOp::Mul => {
                if is_float {
                    bx.fmul(lhs, rhs)
                } else {
                    bx.mul(lhs, rhs)
                }
            }
            mir::BinOp::MulUnchecked => {
                if is_signed {
                    bx.unchecked_smul(lhs, rhs)
                } else {
                    bx.unchecked_umul(lhs, rhs)
                }
            }
            mir::BinOp::Div => {
                if is_float {
                    bx.fdiv(lhs, rhs)
                } else if is_signed {
                    bx.sdiv(lhs, rhs)
                } else {
                    bx.udiv(lhs, rhs)
                }
            }
            mir::BinOp::Rem => {
                if is_float {
                    bx.frem(lhs, rhs)
                } else if is_signed {
                    bx.srem(lhs, rhs)
                } else {
                    bx.urem(lhs, rhs)
                }
            }
            mir::BinOp::BitOr => bx.or(lhs, rhs),
            mir::BinOp::BitAnd => bx.and(lhs, rhs),
            mir::BinOp::BitXor => bx.xor(lhs, rhs),
            mir::BinOp::Offset => self.codegen_immediate_offset(bx, lhs, None, rhs, lhs_ty, rhs_ty),
            mir::BinOp::Shl | mir::BinOp::ShlUnchecked => {
                let rhs = base::build_shift_expr_rhs(bx, lhs, rhs, op == mir::BinOp::ShlUnchecked);
                bx.shl(lhs, rhs)
            }
            mir::BinOp::Shr | mir::BinOp::ShrUnchecked => {
                let rhs = base::build_shift_expr_rhs(bx, lhs, rhs, op == mir::BinOp::ShrUnchecked);
                if is_signed { bx.ashr(lhs, rhs) } else { bx.lshr(lhs, rhs) }
            }
            mir::BinOp::Ne
            | mir::BinOp::Lt
            | mir::BinOp::Gt
            | mir::BinOp::Eq
            | mir::BinOp::Le
            | mir::BinOp::Ge => {
                if is_float {
                    bx.fcmp(base::bin_op_to_fcmp_predicate(op), lhs, rhs)
                } else {
                    bx.icmp(base::bin_op_to_icmp_predicate(op, is_signed), lhs, rhs)
                }
            }
            mir::BinOp::Cmp => {
                assert!(!is_float);
                bx.three_way_compare(lhs_ty, lhs, rhs)
            }
            mir::BinOp::AddWithOverflow
            | mir::BinOp::SubWithOverflow
            | mir::BinOp::MulWithOverflow => {
                bug!("{op:?} needs to return a pair, so call codegen_scalar_checked_binop instead")
            }
        }
    }

    /// A `BinOp::Offset` with a `Scalar` or `ScalarPair` pointer operand.
    /// The return value is the ptr value of the offsetted pointer.
    /// The metadata should be `lhs_extra` re-used.
    fn codegen_immediate_offset(
        &mut self,
        bx: &mut Bx,
        lhs_ptr: Bx::Value,
        lhs_extra: Option<Bx::Value>,
        rhs: Bx::Value,
        lhs_ty: Ty<'tcx>,
        rhs_ty: Ty<'tcx>,
    ) -> Bx::Value {
        let pointee_type =
            lhs_ty.builtin_deref(true).unwrap_or_else(|| bug!("deref of non-pointer {:?}", lhs_ty));
        let pointee_layout = bx.cx().layout_of(pointee_type);
        if pointee_layout.is_zst() {
            // `Offset` works in terms of the size of pointee,
            // so offsetting a pointer to ZST is a noop.
            lhs_ptr
        } else if pointee_layout.is_sized() {
            let llty = bx.cx().backend_type(pointee_layout);
            if !rhs_ty.is_signed() {
                bx.inbounds_nuw_gep(llty, lhs_ptr, &[rhs])
            } else {
                bx.inbounds_gep(llty, lhs_ptr, &[rhs])
            }
        } else {
            let meta_ty = Ty::new_ptr_metadata(bx.tcx(), pointee_type);
            let meta_layout = bx.cx().layout_of(meta_ty);
            let meta = AnyPlaceMeta(lhs_extra.map(|meta| {
                OperandRef::from_immediate_or_packed_pair(bx, meta, meta_layout)
                    .expect_sized("pointer metadata must be sized")
            }));
            let (pointee_size, _pointee_align) =
                size_of_val::size_and_align_of_dst(Some(self), bx, pointee_type, meta);

            // FIXME(more_unsized): make this the right kind of unchecked mul
            let offset = bx.mul(pointee_size, rhs);

            let llty = bx.cx().type_i8();
            if !rhs_ty.is_signed() {
                bx.inbounds_nuw_gep(llty, lhs_ptr, &[offset])
            } else {
                bx.inbounds_gep(llty, lhs_ptr, &[offset])
            }
        }
    }

    fn codegen_scalar_checked_binop(
        &mut self,
        bx: &mut Bx,
        op: mir::BinOp,
        lhs: Bx::Value,
        rhs: Bx::Value,
        input_ty: Ty<'tcx>,
    ) -> OperandValue<'tcx, Bx::Value> {
        let (val, of) = match op {
            // These are checked using intrinsics
            mir::BinOp::Add | mir::BinOp::Sub | mir::BinOp::Mul => {
                let oop = match op {
                    mir::BinOp::Add => OverflowOp::Add,
                    mir::BinOp::Sub => OverflowOp::Sub,
                    mir::BinOp::Mul => OverflowOp::Mul,
                    _ => unreachable!(),
                };
                bx.checked_binop(oop, input_ty, lhs, rhs)
            }
            _ => bug!("Operator `{:?}` is not a checkable operator", op),
        };

        OperandValue::Pair(val, of)
    }
}

/// Transmutes a single scalar value `imm` from `from_scalar` to `to_scalar`.
///
/// This is expected to be in *immediate* form, as seen in [`OperandValue::Immediate`]
/// or [`OperandValue::Pair`] (so `i1` for bools, not `i8`, for example).
///
/// ICEs if the passed-in `imm` is not a value of the expected type for
/// `from_scalar`, such as if it's a vector or a pair.
pub(super) fn transmute_scalar<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    mut imm: Bx::Value,
    from_scalar: abi::Scalar,
    to_scalar: abi::Scalar,
) -> Bx::Value {
    assert_eq!(from_scalar.size(bx.cx()), to_scalar.size(bx.cx()));
    let imm_ty = bx.cx().val_ty(imm);
    assert_ne!(
        bx.cx().type_kind(imm_ty),
        TypeKind::Vector,
        "Vector type {imm_ty:?} not allowed in transmute_scalar {from_scalar:?} -> {to_scalar:?}"
    );

    // While optimizations will remove no-op transmutes, they might still be
    // there in debug or things that aren't no-op in MIR because they change
    // the Rust type but not the underlying layout/niche.
    if from_scalar == to_scalar {
        return imm;
    }

    use abi::Primitive::*;
    imm = bx.from_immediate(imm);

    let from_backend_ty = bx.cx().type_from_scalar(from_scalar);
    debug_assert_eq!(bx.cx().val_ty(imm), from_backend_ty);
    let to_backend_ty = bx.cx().type_from_scalar(to_scalar);

    // If we have a scalar, we must already know its range. Either
    //
    // 1) It's a parameter with `range` parameter metadata,
    // 2) It's something we `load`ed with `!range` metadata, or
    // 3) After a transmute we `assume`d the range (see below).
    //
    // That said, last time we tried removing this, it didn't actually help
    // the rustc-perf results, so might as well keep doing it
    // <https://github.com/rust-lang/rust/pull/135610#issuecomment-2599275182>
    assume_scalar_range(bx, imm, from_scalar, from_backend_ty, Some(&to_scalar));

    imm = match (from_scalar.primitive(), to_scalar.primitive()) {
        (Int(..) | Float(_), Int(..) | Float(_)) => bx.bitcast(imm, to_backend_ty),
        (Pointer(..), Pointer(..)) => bx.pointercast(imm, to_backend_ty),
        (Int(..), Pointer(..)) => bx.inttoptr(imm, to_backend_ty),
        (Pointer(..), Int(..)) => {
            // FIXME: this exposes the provenance, which shouldn't be necessary.
            bx.ptrtoint(imm, to_backend_ty)
        }
        (Float(_), Pointer(..)) => {
            let int_imm = bx.bitcast(imm, bx.cx().type_isize());
            bx.inttoptr(int_imm, to_backend_ty)
        }
        (Pointer(..), Float(_)) => {
            // FIXME: this exposes the provenance, which shouldn't be necessary.
            let int_imm = bx.ptrtoint(imm, bx.cx().type_isize());
            bx.bitcast(int_imm, to_backend_ty)
        }
    };

    debug_assert_eq!(bx.cx().val_ty(imm), to_backend_ty);

    // This `assume` remains important for cases like (a conceptual)
    //    transmute::<u32, NonZeroU32>(x) == 0
    // since it's never passed to something with parameter metadata (especially
    // after MIR inlining) so the only way to tell the backend about the
    // constraint that the `transmute` introduced is to `assume` it.
    assume_scalar_range(bx, imm, to_scalar, to_backend_ty, Some(&from_scalar));

    imm = bx.to_immediate_scalar(imm, to_scalar);
    imm
}

/// Emits an `assume` call that `imm`'s value is within the known range of `scalar`.
///
/// If `known` is `Some`, only emits the assume if it's more specific than
/// whatever is already known from the range of *that* scalar.
fn assume_scalar_range<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    imm: Bx::Value,
    scalar: abi::Scalar,
    backend_ty: Bx::Type,
    known: Option<&abi::Scalar>,
) {
    if matches!(bx.cx().sess().opts.optimize, OptLevel::No) {
        return;
    }

    match (scalar, known) {
        (abi::Scalar::Union { .. }, _) => return,
        (_, None) => {
            if scalar.is_always_valid(bx.cx()) {
                return;
            }
        }
        (abi::Scalar::Initialized { valid_range, .. }, Some(known)) => {
            let known_range = known.valid_range(bx.cx());
            if valid_range.contains_range(known_range, scalar.size(bx.cx())) {
                return;
            }
        }
    }

    match scalar.primitive() {
        abi::Primitive::Int(..) => {
            let range = scalar.valid_range(bx.cx());
            bx.assume_integer_range(imm, backend_ty, range);
        }
        abi::Primitive::Pointer(abi::AddressSpace::ZERO)
            if !scalar.valid_range(bx.cx()).contains(0) =>
        {
            bx.assume_nonnull(imm);
        }
        abi::Primitive::Pointer(..) | abi::Primitive::Float(..) => {}
    }
}

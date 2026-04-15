//! Computing the size and alignment of a value.

use std::debug_assert_matches;

use rustc_abi::{Align, FieldIdx, FieldsShape, VariantIdx, WrappingRange};
use rustc_hir::LangItem;
use rustc_middle::bug;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths};
use rustc_middle::ty::{self, Ty};
use rustc_span::DUMMY_SP;
use tracing::trace;

use crate::common::IntPredicate;
use crate::mir::operand::OperandRef;
use crate::mir::place::PlaceRef;
use crate::mir::{AnyPlaceMeta, FunctionCx, PlaceMetadata};
use crate::traits::*;
use crate::{common, meth};

#[derive(Debug)]
enum CalculationResult<V> {
    /// The computation was unchecked, or the result is statically known to be valid.
    /// If only the size or alignment was requested, the other value may be invalid.
    Unchecked { size: V, align: V },
    /// The result is statically known to be invalid.
    Invalid,
    /// The computation was checked, and the result is not statically known to be valid.
    /// If only the size or alignment was requested, the other value may be invalid.
    Checked { valid: V, size: V, align: V },
}

pub fn size_and_align_of_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
) -> (Bx::Value, Bx::Value) {
    match size_and_align_of_dst_impl(fx, bx, t, info, false) {
        CalculationResult::Unchecked { size, align }
        | CalculationResult::Checked { size, align, .. } => (size, align),
        CalculationResult::Invalid => (bx.const_usize(0), bx.const_usize(1)),
    }
}

pub fn checked_size_and_align_of_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
) -> (Bx::Value, Bx::Value, Bx::Value) {
    match size_and_align_of_dst_impl(fx, bx, t, info, true) {
        CalculationResult::Unchecked { size, align } => (bx.const_bool(true), size, align),
        CalculationResult::Checked { valid, size, align } => (valid, size, align),
        CalculationResult::Invalid => (bx.const_bool(false), bx.const_usize(0), bx.const_usize(1)),
    }
}

pub fn field_offset_for_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
    field: FieldIdx,
) -> (Bx::Value, Bx::Value) {
    match layout_of_dst_impl(fx, bx, t, info, false, LayoutComputeGoal::FieldOffset(field)) {
        CalculationResult::Unchecked { size, align }
        | CalculationResult::Checked { size, align, .. } => (size, align),
        CalculationResult::Invalid => (bx.const_usize(0), bx.const_usize(1)),
    }
}

pub fn checked_field_offset_for_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
    field: FieldIdx,
) -> (Bx::Value, Bx::Value, Bx::Value) {
    match layout_of_dst_impl(fx, bx, t, info, true, LayoutComputeGoal::FieldOffset(field)) {
        CalculationResult::Unchecked { size, align } => (bx.const_bool(true), size, align),
        CalculationResult::Checked { valid, size, align } => (valid, size, align),
        CalculationResult::Invalid => (bx.const_bool(false), bx.const_usize(0), bx.const_usize(1)),
    }
}

/// What we are computing
#[derive(Debug, Clone, Copy)]
pub enum LayoutComputeGoal {
    /// We are computing the overall size and alignment of the value.
    OverallLayout,
    /// We are computing the overall size of the value.
    OverallSize,
    /// We are computing the overall alignment of the value.
    OverallAlignment,
    /// We are computing the offset and effective alignment of a field.
    FieldOffset(FieldIdx),
}

fn layout_for_arraylike_impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    checked: bool,
    goal: LayoutComputeGoal,
    elem_layout: CalculationResult<Bx::Value>,
    count: Bx::Value,
) -> CalculationResult<Bx::Value> {
    let (elem_valid, elem_size, elem_align) = match elem_layout {
        CalculationResult::Invalid => return CalculationResult::Invalid,
        CalculationResult::Unchecked { size, align } => (None, size, align),
        CalculationResult::Checked { valid, size, align } => (Some(valid), size, align),
    };
    if matches!(goal, LayoutComputeGoal::FieldOffset(_)) {
        todo!();
    } else if matches!(goal, LayoutComputeGoal::OverallAlignment) {
        // The alignment of an array or slice is the alignment of the element
        return elem_layout;
    }

    let try_to_const =
        |val: Bx::Value| -> Result<u64, Bx::Value> { bx.const_to_opt_uint(val).ok_or(val) };

    let (valid, size, align) = match (try_to_const(count), try_to_const(elem_size)) {
        // Zero-length array/slice, or array/slice of zero-sized elements is always zero-sized.
        // The element layout must still be valid for this to be valid.
        (Ok(0), _) | (_, Ok(0)) => (elem_valid, bx.const_usize(0), elem_align),
        // A 1-length array/slice has the size of its element.
        // We already know if elem_size <= isize::MAX (via `elem_valid`)
        (Ok(1), Err(elem_size)) => (elem_valid, elem_size, elem_align),
        // An array/slice of 1-byte values has size == length,
        // but we still need to check if the length <= isize::MAX.
        (Err(len), Ok(1)) => {
            let size_valid = if !checked {
                None
            } else {
                let isize_max: u64 =
                    bx.data_layout().ptr_sized_integer().signed_max().try_into().unwrap();
                let len_valid = bx.icmp(IntPredicate::IntULE, len, bx.const_usize(isize_max));
                match elem_valid {
                    None => Some(len_valid),
                    Some(elem_valid) => {
                        let size_valid = bx.and(elem_valid, len_valid);
                        Some(size_valid)
                    }
                }
            };
            (size_valid, len, elem_align)
        }
        // Optimize the case where both size and len are known statically.
        (Ok(lhs), Ok(rhs)) => {
            let isize_max: u64 =
                bx.data_layout().ptr_sized_integer().signed_max().try_into().unwrap();
            let (size, overflow) = u64::overflowing_mul(lhs, rhs);
            let overflow = overflow || size > isize_max;
            let valid = if !checked {
                None
            } else if overflow {
                Some(bx.const_bool(false))
            } else {
                elem_valid
            };
            (valid, bx.const_usize(size), elem_align)
        }
        // In unchecked mode, don't do any nontrivial optimizations/checks
        _ if !checked => {
            // In unchecked mode, all sizes must fit into `isize`, so this multiplication cannot
            // wrap -- neither signed nor unsigned.
            let size = bx.unchecked_sumul(count, elem_size);
            (None, size, elem_align)
        }
        _ => {
            let (size, overflow) =
                bx.checked_binop(OverflowOp::Mul, bx.tcx().types.usize, count, elem_size);

            // We don't just care about `usize` overflow, we also check that `size <= isize::MAX as usize`.
            let isize_max: u64 =
                bx.data_layout().ptr_sized_integer().signed_max().try_into().unwrap();

            let valid_1 = bx.not(overflow);
            let valid_2 = bx.icmp(IntPredicate::IntULE, size, bx.const_usize(isize_max));
            let valid = bx.and(valid_1, valid_2);
            let valid = match elem_valid {
                Some(elem_valid) => Some(bx.and(valid, elem_valid)),
                None => Some(valid),
            };
            (valid, size, elem_align)
        }
    };
    match valid {
        Some(valid) => CalculationResult::Checked { valid, size, align },
        None => CalculationResult::Unchecked { size, align },
    }
}

fn size_and_align_of_dst_impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
    checked: bool,
) -> CalculationResult<Bx::Value> {
    layout_of_dst_impl(fx, bx, t, info, checked, LayoutComputeGoal::OverallLayout)
}

fn layout_of_dst_impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    mut fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
    checked: bool,
    goal: LayoutComputeGoal,
) -> CalculationResult<Bx::Value> {
    let layout = bx.layout_of(t);
    trace!(
        "layout_of_dst_impl(ty={}, info={:?}, checked={}, goal={:?}): layout: {:?}",
        t, info, checked, goal, layout
    );
    if matches!(goal, LayoutComputeGoal::OverallLayout | LayoutComputeGoal::OverallSize)
        && layout.is_sized()
    {
        let size = bx.const_usize(layout.size.bytes());
        let align = bx.const_usize(layout.align.bytes());
        return CalculationResult::Unchecked { size, align };
    } else if matches!(goal, LayoutComputeGoal::OverallAlignment) && layout.align_is_exact {
        let size = bx.const_usize(layout.size.bytes());
        let align = bx.const_usize(layout.align.bytes());
        return CalculationResult::Unchecked { size, align };
    } else if let LayoutComputeGoal::FieldOffset(field_idx) = goal
        && let Some(field_offset) = layout.fields.try_exact_offset(field_idx.as_usize())
    {
        let field_layout = layout.field(bx, field_idx.as_usize());
        let field_ty_align = field_layout.layout.align.abi;
        let mut field_align = field_ty_align;
        if let Some(adt_def) = layout.ty.ty_adt_def()
            && let Some(pack) = adt_def.repr().pack
        {
            field_align = Align::min(field_align, pack);
        }
        let offset = bx.const_usize(field_offset.bytes());
        let align = bx.const_usize(field_align.bytes());
        return CalculationResult::Unchecked { size: offset, align };
    }

    // Invariant: all valid sizes (including intermediate sizes) are `<= isize::MAX`.
    // Therefore, we can calculate such that the output is correct if both addends are `<= isize::MAX`,
    // and if either input was invalid, `valid` will have already been set to false so the output doesn't matter.
    let add = |bx: &mut Bx, valid: &mut Option<_>, lhs, rhs| {
        // Because we know that valid sizes are `<= isize::MAX`, and `isize::MAX as usize * 2 <= usize::MAX`,
        // we can do the addition and check overflow afterwards.
        // Note that we still can't use `unchecked_add` unconditionally, since this could be after an overflow
        // in a checked computation.
        let sum = bx.add(lhs, rhs);
        if checked {
            let isize_max = bx
                .const_usize(bx.data_layout().ptr_sized_integer().signed_max().try_into().unwrap());
            let sum_valid = bx.icmp(IntPredicate::IntULE, sum, isize_max);
            *valid = Some(match *valid {
                Some(valid) => bx.and(valid, sum_valid),
                None => sum_valid,
            });
        }
        sum
    };

    let max = |bx: &mut Bx, lhs, rhs| {
        let cmp = bx.icmp(IntPredicate::IntUGT, lhs, rhs);
        bx.select(cmp, lhs, rhs)
    };
    let min = |bx: &mut Bx, lhs, rhs| {
        let cmp = bx.icmp(IntPredicate::IntULT, lhs, rhs);
        bx.select(cmp, lhs, rhs)
    };

    // Invariant: all valid sizes (including intermediate sizes) are `<= isize::MAX`,
    // and all valid alignments are powers of 2.
    // Therefore, we can calculate such that the output is correct if the size and align
    // given are valid, and if either input was invalid, `valid` would have already been set to false so
    // the output doesn't matter.
    let round_up_to_alignment = |bx: &mut Bx, valid: &mut Option<_>, size, alignment| {
        // The bit-magic to round `size` up to a multiple of `alignment` is
        //
        //     `(size + (align-1)) & -align`
        //
        // This is valid even for extreme cases `size = isize::MAX as usize, align = isize::MAX as usize + 1`.
        // We also need to check that the new size is still `<= isize::MAX`. This could happen if it was
        // rounded up to `isize::MAX + 1`. However, this happens if and only if the sum was `> isize::MAX`,
        // so we can just check the addition with our `add` closure that already does that check.
        let one = bx.const_usize(1);
        let addend = bx.sub(alignment, one);
        let sum = add(bx, valid, size, addend);
        let neg = bx.neg(alignment);
        bx.and(sum, neg)
    };

    let try_to_const = |bx: &mut Bx, val: Bx::Value| -> Result<u64, Bx::Value> {
        bx.const_to_opt_uint(val).ok_or(val)
    };

    let mk_clamp_alignment_for_adt = |adt_def: Option<ty::AdtDef<'tcx>>| {
        let packed = adt_def.and_then(|adt_def| adt_def.repr().pack);
        move |bx: &mut Bx, align: Bx::Value| {
            if let Some(packed) = packed {
                if packed.bytes() == 1 {
                    bx.const_usize(1)
                } else if let Ok(align) = try_to_const(bx, align) {
                    bx.const_usize(u64::min(align, packed.bytes()))
                } else {
                    // We have to dynamically compute `min(align, packed)`.
                    let packed = bx.const_usize(packed.bytes());
                    min(bx, align, packed)
                }
            } else {
                align
            }
        }
    };

    match t.kind() {
        ty::Dynamic(..) => {
            // Load size/align from vtable.
            let vtable = info.immediate();
            let size = meth::VirtualIndex::from_index(ty::COMMON_VTABLE_ENTRIES_SIZE)
                .get_usize(bx, vtable, t);
            let align = meth::VirtualIndex::from_index(ty::COMMON_VTABLE_ENTRIES_ALIGN)
                .get_usize(bx, vtable, t);

            // Size is always <= isize::MAX.
            let size_bound = bx.data_layout().ptr_sized_integer().signed_max() as u128;
            bx.range_metadata(size, WrappingRange { start: 0, end: size_bound });
            // Alignment is always a power of two, thus 1..=0x800…000,
            // but also bounded by the maximum we support in type layout.
            let align_bound = Align::max_for_target(bx.data_layout()).bytes().into();
            bx.range_metadata(align, WrappingRange { start: 1, end: align_bound });

            CalculationResult::Unchecked { size, align }
        }
        ty::Array(elem_ty, len) => {
            let len =
                len.try_to_target_usize(bx.tcx()).expect("expected monomorphic const in codegen");
            // Sized arrays were handled above, only unsized arrays reach here
            let meta = info.0.expect("unsized array should have metadata");
            let elem_meta_layout = meta.layout.field(bx.cx(), 0);
            let elem_info = AnyPlaceMeta(Some(OperandRef {
                val: meta.val,
                layout: elem_meta_layout,
                move_annotation: None,
            }));

            let elem_layout = size_and_align_of_dst_impl(fx, bx, *elem_ty, elem_info, checked);

            layout_for_arraylike_impl(bx, checked, goal, elem_layout, bx.const_usize(len))
        }
        ty::Str => {
            let len = info.0.expect("str should have metadata").change_sizedness().immediate();
            let elem_layout =
                CalculationResult::Unchecked { size: bx.const_usize(1), align: bx.const_usize(1) };
            layout_for_arraylike_impl(bx, checked, goal, elem_layout, len)
        }
        ty::Slice(elem_ty) => {
            let meta = info.0.expect("slice should have metadata").change_sizedness();
            let meta_len = meta.extract_or_load_field(bx, 0);
            let meta_elem = meta.extract_or_load_field(bx, 1);
            let len = meta_len.immediate();
            let elem_info =
                AnyPlaceMeta(Some(meta_elem.expect_sized("pointer metadata must be sized")));

            let elem_layout = size_and_align_of_dst_impl(fx, bx, *elem_ty, elem_info, checked);
            layout_for_arraylike_impl(bx, checked, goal, elem_layout, len)
        }
        ty::Foreign(_) => {
            // `extern` type. We cannot compute the size, so panic.
            let msg_str = with_no_visible_paths!({
                with_no_trimmed_paths!({
                    format!("attempted to compute the size or alignment of extern type `{t}`")
                })
            });
            let msg = bx.const_str(&msg_str);

            // Obtain the panic entry point.
            let (fn_abi, llfn, _instance) =
                common::build_langcall(bx, DUMMY_SP, LangItem::PanicNounwind);

            // Generate the call. Cannot use `do_call` since we don't have a MIR terminator so we
            // can't create a `TerminationCodegenHelper`. (But we are in good company, this code is
            // duplicated plenty of times.)
            let fn_ty = bx.fn_decl_backend_type(fn_abi);

            bx.call(
                fn_ty,
                /* fn_attrs */ None,
                Some(fn_abi),
                llfn,
                &[msg.0, msg.1],
                None,
                None,
            );

            CalculationResult::Invalid
        }
        ty::Adt(adt_def, ..) if adt_def.is_unsized_type() => {
            let tcx = bx.tcx();
            // An `unsized type`, possibly with a custom `MetaSized` impl.

            // FIXME: make the methods lang items
            let meta_sized = tcx.require_lang_item(LangItem::MetaSized, DUMMY_SP);
            let alignment_struct_ty = tcx.ty_alignment_struct(DUMMY_SP);
            let size_and_align_tup = Ty::new_tup(tcx, &[tcx.types.usize, alignment_struct_ty]);
            let (trait_def_id, method_to_call, method_ret_ty) = match (goal, checked) {
                (LayoutComputeGoal::FieldOffset(_), _) => bug!("`unsized type`s have no fields"),
                (LayoutComputeGoal::OverallLayout, true) => {
                    (meta_sized, "checked_layout_for_meta", Ty::new_option(tcx, size_and_align_tup))
                }
                (LayoutComputeGoal::OverallLayout, false) => {
                    (meta_sized, "unchecked_layout_for_meta", size_and_align_tup)
                }
                (LayoutComputeGoal::OverallSize, true) => {
                    (meta_sized, "checked_size_for_meta", Ty::new_option(tcx, tcx.types.usize))
                }
                (LayoutComputeGoal::OverallSize, false) => {
                    (meta_sized, "unchecked_size_for_meta", tcx.types.usize)
                }
                (LayoutComputeGoal::OverallAlignment, true) => {
                    (meta_sized, "checked_align_for_meta", Ty::new_option(tcx, alignment_struct_ty))
                }
                (LayoutComputeGoal::OverallAlignment, false) => {
                    (meta_sized, "unchecked_align_for_meta", alignment_struct_ty)
                }
            };
            let method_ret_ty = bx.layout_of(method_ret_ty);

            let method_def_id = tcx
                .associated_items(trait_def_id)
                .filter_by_name_unhygienic(rustc_span::Symbol::intern(method_to_call))
                .next()
                .unwrap()
                .def_id;
            // FIXME: what does `try_resolve` do if the trait isn't impleemnted?
            let instance = ty::Instance::try_resolve(
                tcx,
                bx.typing_env(),
                method_def_id,
                tcx.mk_args(&[t.into()]),
            )
            .unwrap();

            let Some(instance) = instance else {
                // The `unsized type` does not implement the relevant trait. We cannot compute the layout, so panic.
                let msg_str = with_no_visible_paths!({
                    with_no_trimmed_paths!({
                        format!(
                            "attempted to compute the size or alignment of unsized type `{t}` that does not implement `MetaSized`"
                        )
                    })
                });
                let msg = bx.const_str(&msg_str);

                // Obtain the panic entry point.
                let (fn_abi, llfn, _instance) =
                    common::build_langcall(bx, DUMMY_SP, LangItem::PanicNounwind);

                // Generate the call. Cannot use `do_call` since we don't have a MIR terminator so we
                // can't create a `TerminationCodegenHelper`. (But we are in good company, this code is
                // duplicated plenty of times.)
                let fn_ty = bx.fn_decl_backend_type(fn_abi);

                bx.call(
                    fn_ty,
                    /* fn_attrs */ None,
                    Some(fn_abi),
                    llfn,
                    &[msg.0, msg.1],
                    None,
                    None,
                );

                return CalculationResult::Invalid;
            };

            let fx = fx.expect("FunctionCx is required to compute layout of `unsized type`");

            let fn_abi = bx.fn_abi_of_instance(instance, ty::List::empty());
            let llfn = bx.get_fn_addr(instance);

            // Generate the call. Cannot use `do_call` since we don't have a MIR terminator so we
            // can't create a `TerminationCodegenHelper`. (But we are in good company, this code is
            // duplicated plenty of times.)
            let fn_ty = bx.fn_decl_backend_type(fn_abi);

            let mut llargs = Vec::with_capacity(3);

            let ret_place = if fn_abi.ret.is_indirect() {
                let ret_place = PlaceRef::alloca(bx, method_ret_ty);
                llargs.push(ret_place.val.llval);
                Some(ret_place)
            } else {
                None
            };

            if let Some(meta) = info.0 {
                match meta.val {
                    crate::mir::operand::OperandValue::Ref(place_value) => {
                        llargs.push(place_value.llval)
                    }
                    crate::mir::operand::OperandValue::Immediate(meta) => llargs.push(meta),
                    crate::mir::operand::OperandValue::Pair(a, b) => llargs.extend([a, b]),
                    crate::mir::operand::OperandValue::ZeroSized => {}
                }
            }

            let llret =
                bx.call(fn_ty, /* fn_attrs */ None, Some(fn_abi), llfn, &llargs, None, None);

            let ret = if fn_abi.ret.is_indirect() {
                ret_place.unwrap()
            } else {
                let ret_op = OperandRef::from_immediate_or_packed_pair(bx, llret, method_ret_ty);
                let ret_place = PlaceRef::alloca(bx, method_ret_ty);
                ret_op.store_with_annotation(bx, ret_place);
                ret_place
            };

            if checked {
                // 0 -> None -> false
                // 1 -> Some -> true
                let llvalid = bx.load_operand(ret).codegen_get_discr(fx, bx, tcx.types.bool);
                let ret_some_field =
                    ret.project_downcast(bx, VariantIdx::from_usize(1)).project_field(None, bx, 0);

                let (llsize, llalign) = match goal {
                    LayoutComputeGoal::OverallLayout => {
                        let llsize = ret_some_field.project_field(None, bx, 0);
                        let llalign = ret_some_field.project_field(None, bx, 1);
                        let llsize = bx.load_operand(llsize).immediate();
                        let llalign = bx.load_operand(llalign).immediate();
                        (
                            bx.select(llvalid, llsize, bx.const_usize(0)),
                            bx.select(llvalid, llalign, bx.const_usize(1)),
                        )
                    }
                    LayoutComputeGoal::OverallSize => {
                        let llsize = bx.load_operand(ret_some_field).immediate();
                        (bx.select(llvalid, llsize, bx.const_usize(0)), bx.const_usize(1))
                    }
                    LayoutComputeGoal::OverallAlignment => {
                        let llalign = bx.load_operand(ret_some_field).immediate();
                        (bx.const_usize(0), bx.select(llvalid, llalign, bx.const_usize(1)))
                    }
                    LayoutComputeGoal::FieldOffset(_) => unreachable!(),
                };

                CalculationResult::Checked { valid: llvalid, size: llsize, align: llalign }
            } else {
                let (llsize, llalign) = match goal {
                    LayoutComputeGoal::OverallLayout => {
                        let llsize = ret.project_field(None, bx, 0);
                        let llalign = ret.project_field(None, bx, 1);
                        (bx.load_operand(llsize).immediate(), bx.load_operand(llalign).immediate())
                    }
                    LayoutComputeGoal::OverallSize => {
                        (bx.load_operand(ret).immediate(), bx.const_usize(1))
                    }
                    LayoutComputeGoal::OverallAlignment => {
                        (bx.const_usize(0), bx.load_operand(ret).immediate())
                    }
                    LayoutComputeGoal::FieldOffset(_) => unreachable!(),
                };

                CalculationResult::Unchecked { size: llsize, align: llalign }
            }
        }
        ty::Adt(adt_def, ..) if adt_def.is_union() => {
            let FieldsShape::Union(field_count) = layout.fields else {
                bug!("union ({t:?}) had non-Union FieldsShape: {:?}", layout.fields)
            };

            let LayoutComputeGoal::OverallLayout = goal else {
                bug!("all union fields should be at exact offset 0")
            };

            // FIXME(more_unsized): optimize this
            let mut adt_valid = None;
            let mut adt_size = bx.const_usize(0);
            let mut adt_align = bx.const_usize(layout.align.bytes());

            let field_meta = |bx: &mut Bx, i: usize| {
                let Some(meta) = info.0 else { return AnyPlaceMeta(None) };

                let meta = meta.change_sizedness();
                AnyPlaceMeta(Some(
                    meta.extract_or_load_field(bx, i)
                        .expect_sized("pointer metadata must be sized"),
                ))
            };

            for field_idx in 0..field_count.get() {
                let field_ty = layout.field(bx, field_idx);
                let field_meta = field_meta(bx, field_idx);
                match size_and_align_of_dst_impl(
                    fx.as_deref_mut(),
                    bx,
                    field_ty.ty,
                    field_meta,
                    checked,
                ) {
                    CalculationResult::Invalid => return CalculationResult::Invalid,
                    CalculationResult::Unchecked { size: field_size, align: field_align } => {
                        adt_size = max(bx, field_size, adt_size);
                        adt_align = max(bx, field_align, adt_align);
                    }
                    CalculationResult::Checked {
                        valid: field_valid,
                        size: field_size,
                        align: field_align,
                    } => {
                        adt_valid = match adt_valid {
                            Some(adt_valid) => Some(bx.and(adt_valid, field_valid)),
                            None => Some(field_valid),
                        };
                        adt_size = max(bx, field_size, adt_size);
                        adt_align = max(bx, field_align, adt_align);
                    }
                }
            }

            // For packed types, we need to cap the alignment.
            adt_align = mk_clamp_alignment_for_adt(t.ty_adt_def())(bx, adt_align);

            // Round up full size to alignment (includes checking for overflow in checked mode)
            adt_size = round_up_to_alignment(bx, &mut adt_valid, adt_size, adt_align);

            match adt_valid {
                Some(adt_valid) => CalculationResult::Checked {
                    valid: adt_valid,
                    size: adt_size,
                    align: adt_align,
                },
                None => CalculationResult::Unchecked { size: adt_size, align: adt_align },
            }
        }
        ty::Adt(adt_def, ..) if adt_def.is_enum() => {
            unimplemented!("unsized enums")
        }
        ty::Adt(adt_def, ..) if adt_def.is_unsized_type() => {
            unimplemented!("call MetaSized::(un)checked_layout_for_meta")
        }
        ty::Adt(..) | ty::Tuple(..) => {
            let FieldsShape::Arbitrary { in_memory_order, .. } = &layout.fields else {
                bug!(
                    "struct or tuple ({:?}) should have FieldsShape::Arbitrary, not {:?}",
                    t,
                    layout.fields
                )
            };

            // For packed types, we need to cap the alignment.
            let clamp_field_align = mk_clamp_alignment_for_adt(t.ty_adt_def());

            // FIXME(more_unsized): optimize this
            let mut adt_valid = None;
            let mut adt_size = bx.const_usize(0);
            let mut adt_align = bx.const_usize(layout.align.bytes());

            let field_meta = |bx: &mut Bx, i: usize| {
                let Some(meta) = info.0 else { return AnyPlaceMeta(None) };

                let meta = meta.change_sizedness();
                AnyPlaceMeta(Some(
                    meta.extract_or_load_field(bx, i)
                        .expect_sized("pointer metadata must be sized"),
                ))
            };

            for &field_idx in in_memory_order {
                let field_ty = layout.field(bx, field_idx.as_usize());
                let field_meta = field_meta(bx, field_idx.as_usize());
                let (field_size, field_align) = match size_and_align_of_dst_impl(
                    fx.as_deref_mut(),
                    bx,
                    field_ty.ty,
                    field_meta,
                    checked,
                ) {
                    CalculationResult::Invalid => return CalculationResult::Invalid,
                    CalculationResult::Unchecked { size: field_size, align: field_align } => {
                        (field_size, field_align)
                    }
                    CalculationResult::Checked {
                        valid: field_valid,
                        size: field_size,
                        align: field_align,
                    } => {
                        adt_valid = match adt_valid {
                            Some(adt_valid) => Some(bx.and(adt_valid, field_valid)),
                            None => Some(field_valid),
                        };
                        (field_size, field_align)
                    }
                };
                let field_align = clamp_field_align(bx, field_align);
                adt_size = round_up_to_alignment(bx, &mut adt_valid, adt_size, field_align);

                // If this is the field we are looking for, return its offset and effective alignment
                if let LayoutComputeGoal::FieldOffset(goal_field_idx) = goal
                    && goal_field_idx == field_idx
                {
                    match adt_valid {
                        Some(valid) => {
                            return CalculationResult::Checked {
                                valid,
                                size: adt_size,
                                align: field_align,
                            };
                        }
                        None => {
                            return CalculationResult::Unchecked {
                                size: adt_size,
                                align: field_align,
                            };
                        }
                    }
                }

                // Otherwise, keep going.
                adt_size = add(bx, &mut adt_valid, adt_size, field_size);

                // We have to dynamically compute `max(adt_align, field_align)`.
                adt_align = max(bx, field_align, adt_align);
            }

            debug_assert_matches!(goal, LayoutComputeGoal::OverallLayout, "FieldIdx out of range?");

            // Round up full size to alignment (includes checking for overflow in checked mode)
            adt_size = round_up_to_alignment(bx, &mut adt_valid, adt_size, adt_align);

            match adt_valid {
                Some(adt_valid) => CalculationResult::Checked {
                    valid: adt_valid,
                    size: adt_size,
                    align: adt_align,
                },
                None => CalculationResult::Unchecked { size: adt_size, align: adt_align },
            }
        }
        _ => bug!("size_and_align_of_dst: {t} not supported"),
    }
}

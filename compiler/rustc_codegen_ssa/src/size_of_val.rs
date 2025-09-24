//! Computing the size and alignment of a value.

use rustc_abi::{Align, WrappingRange};
use rustc_hir::LangItem;
use rustc_middle::bug;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths};
use rustc_middle::ty::{self, Ty};
use rustc_span::DUMMY_SP;
use tracing::{debug, trace};

use crate::common::IntPredicate;
use crate::mir::operand::{OperandRef, OperandValue};
use crate::mir::place::PlaceRef;
use crate::mir::{AnyPlaceMeta, PlaceMetadata};
use crate::traits::*;
use crate::{common, meth};

#[derive(Debug)]
enum CalculationResult<V> {
    /// The computation was unchecked, or the result is statically known to be valid.
    Unchecked { size: V, align: V },
    /// The result is statically known to be invalid.
    Invalid,
    /// The computation was checked, and the result is not statically known to be valid.
    Checked { valid: V, size: V, align: V },
}

pub fn size_and_align_of_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
) -> (Bx::Value, Bx::Value) {
    match size_and_align_of_dst_impl(bx, t, info, false) {
        CalculationResult::Unchecked { size, align }
        | CalculationResult::Checked { size, align, .. } => (size, align),
        CalculationResult::Invalid => (bx.const_usize(0), bx.const_usize(1)),
    }
}

pub fn checked_size_and_align_of_dst<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
) -> (Bx::Value, Bx::Value, Bx::Value) {
    match size_and_align_of_dst_impl(bx, t, info, true) {
        CalculationResult::Unchecked { size, align } => (bx.const_bool(true), size, align),
        CalculationResult::Checked { valid, size, align } => (valid, size, align),
        CalculationResult::Invalid => (bx.const_bool(false), bx.const_usize(0), bx.const_usize(1)),
    }
}

fn size_and_align_of_arraylike_impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    checked: bool,
    elem_layout: CalculationResult<Bx::Value>,
    count: Bx::Value,
) -> CalculationResult<Bx::Value> {
    let (elem_valid, elem_size, elem_align) = match elem_layout {
        CalculationResult::Invalid => return CalculationResult::Invalid,
        CalculationResult::Unchecked { size, align } => (None, size, align),
        CalculationResult::Checked { valid, size, align } => (Some(valid), size, align),
    };

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
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: AnyPlaceMeta<'tcx, Bx::Value>,
    checked: bool,
) -> CalculationResult<Bx::Value> {
    let layout = bx.layout_of(t);
    trace!("size_and_align_of_dst(ty={}, info={:?}): layout: {:?}", t, info, layout);
    if layout.is_sized() {
        let size = bx.const_usize(layout.size.bytes());
        let align = bx.const_usize(layout.align.bytes());
        return CalculationResult::Unchecked { size, align };
    }

    // Invariant: all valid sizes (including intermediate sizes) are `<= isize::MAX`.
    // Therefore, we can calculate such that the output is correct if both addends are `<= isize::MAX`,
    // and if either input was invalid, `valid` will have already been set to false so the output doesn't matter.
    let add = |bx: &mut Bx, valid: &mut Option<_>, lhs, rhs| {
        // Because we know that valid sizes are `<= isize::MAX`, and `isize::MAX as usize * 2 <= usize::MAX`,
        // we can do the addition and check overflow afterwards.
        // Note that we still can't use `unchecked_add` unconditionally.
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

            let elem_layout = size_and_align_of_dst_impl(bx, *elem_ty, elem_info, checked);

            size_and_align_of_arraylike_impl(bx, checked, elem_layout, bx.const_usize(len))
        }
        ty::Str => {
            let len = info.0.expect("str should have metadata").change_sizedness().immediate();
            let elem_layout =
                CalculationResult::Unchecked { size: bx.const_usize(1), align: bx.const_usize(1) };
            size_and_align_of_arraylike_impl(bx, checked, elem_layout, len)
        }
        ty::Slice(elem_ty) => {
            let meta = info.0.expect("slice should have metadata").change_sizedness();
            let (meta_len, meta_elem) = if let OperandValue::Ref(val) = meta.val {
                let meta_place = PlaceRef { val, layout: meta.layout };
                let meta_len_place = meta_place.project_field(bx, 0);
                let meta_elem_place = meta_place.project_field(bx, 1);
                (bx.load_operand(meta_len_place), bx.load_operand(meta_elem_place))
            } else {
                // FIXME(ptr_metadata_v2): use extract_field here
                (meta.extract_field_simple(bx, 0), meta.extract_field_simple(bx, 1))
            };
            let len = meta_len.immediate();
            let elem_info =
                AnyPlaceMeta(Some(meta_elem.expect_sized("pointer metadata must be sized")));

            let elem_layout = size_and_align_of_dst_impl(bx, *elem_ty, elem_info, checked);
            size_and_align_of_arraylike_impl(bx, checked, elem_layout, len)
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
        ty::Adt(..) | ty::Tuple(..) => {
            // First get the size of all statically known fields.
            // Don't use size_of because it also rounds up to alignment, which we
            // want to avoid, as the unsized field's alignment could be smaller.
            assert!(!t.is_simd());
            debug!("DST {} layout: {:?}", t, layout);

            let i = layout.fields.count() - 1;
            let unsized_offset_unadjusted = layout.fields.offset(i).bytes();
            let sized_align = layout.align.bytes();
            debug!(
                "DST {} offset of dyn field: {}, statically sized align: {}",
                t, unsized_offset_unadjusted, sized_align
            );
            let unsized_offset_unadjusted = bx.const_usize(unsized_offset_unadjusted);
            let sized_align = bx.const_usize(sized_align);

            // Recurse to get the size of the dynamically sized field (must be
            // the last field).
            let field_ty = layout.field(bx, i);
            let field_meta = AnyPlaceMeta(
                if field_ty.is_sized() || field_ty.ty.is_thin(bx.tcx(), bx.typing_env()) {
                    None
                } else {
                    let meta =
                        info.0.expect("non-Thin Adt/Tuple should have metadata").change_sizedness();
                    if let OperandValue::Ref(val) = meta.val {
                        let meta_ref = PlaceRef { val, layout: meta.layout };
                        let field_meta_ref = meta_ref.project_field(bx, i);
                        Some(
                            bx.load_operand(field_meta_ref)
                                .expect_sized("pointer metadata must be sized"),
                        )
                    } else {
                        Some(
                            meta.extract_field_simple(bx, i)
                                .expect_sized("pointer metadata must be sized"),
                        )
                    }
                },
            );
            let (mut valid, unsized_size, mut unsized_align) =
                match size_and_align_of_dst_impl(bx, field_ty.ty, field_meta, checked) {
                    CalculationResult::Unchecked { size, align } => (None, size, align),
                    CalculationResult::Invalid => return CalculationResult::Invalid,
                    CalculationResult::Checked { valid, size, align } => (Some(valid), size, align),
                };

            // # First compute the dynamic alignment

            // For packed types, we need to cap the alignment.
            if let ty::Adt(def, _) = t.kind()
                && let Some(packed) = def.repr().pack
            {
                if packed.bytes() == 1 {
                    // We know this will be capped to 1.
                    unsized_align = bx.const_usize(1);
                } else {
                    // We have to dynamically compute `min(unsized_align, packed)`.
                    let packed = bx.const_usize(packed.bytes());
                    let cmp = bx.icmp(IntPredicate::IntULT, unsized_align, packed);
                    unsized_align = bx.select(cmp, unsized_align, packed);
                }
            }

            // Choose max of two known alignments (combined value must
            // be aligned according to more restrictive of the two).
            let full_align = match (
                bx.const_to_opt_u128(sized_align, false),
                bx.const_to_opt_u128(unsized_align, false),
            ) {
                (Some(sized_align), Some(unsized_align)) => {
                    // If both alignments are constant, (the sized_align should always be), then
                    // pick the correct alignment statically.
                    bx.const_usize(std::cmp::max(sized_align, unsized_align) as u64)
                }
                _ => {
                    let cmp = bx.icmp(IntPredicate::IntUGT, sized_align, unsized_align);
                    bx.select(cmp, sized_align, unsized_align)
                }
            };

            // # Then compute the dynamic size

            // For unions, the size is the max size of the fields, rounded up to the alignment
            if let ty::Adt(def, ..) = t.kind()
                && def.is_union()
            {
                // For now, with only one unsized field, the max of the sizes of the sized fields
                // is just the unsized layout's size.
                let sized_size = bx.const_usize(layout.size.bytes());
                let cmp = bx.icmp(IntPredicate::IntUGT, sized_size, unsized_size);
                let full_size = bx.select(cmp, sized_size, unsized_size);

                let full_size = round_up_to_alignment(bx, &mut valid, full_size, full_align);

                match valid {
                    Some(valid) => {
                        return CalculationResult::Checked {
                            valid,
                            size: full_size,
                            align: full_align,
                        };
                    }
                    None => {
                        return CalculationResult::Unchecked { size: full_size, align: full_align };
                    }
                }
            }

            // The full formula for the size would be:
            // let unsized_offset_adjusted = unsized_offset_unadjusted.align_to(unsized_align);
            // let full_size = (unsized_offset_adjusted + unsized_size).align_to(full_align);
            // However, `unsized_size` is a multiple of `unsized_align`. Therefore, we can
            // equivalently do the `align_to(unsized_align)` *after* adding `unsized_size`:
            //
            // let full_size =
            //     (unsized_offset_unadjusted + unsized_size)
            //     .align_to(unsized_align)
            //     .align_to(full_align);
            //
            // Furthermore, `align >= unsized_align`, and therefore we only need to do:
            // let full_size = (unsized_offset_unadjusted + unsized_size).align_to(full_align);

            let full_size = add(bx, &mut valid, unsized_offset_unadjusted, unsized_size);

            let full_size = round_up_to_alignment(bx, &mut valid, full_size, full_align);

            match valid {
                Some(valid) => {
                    CalculationResult::Checked { valid, size: full_size, align: full_align }
                }
                None => CalculationResult::Unchecked { size: full_size, align: full_align },
            }
        }
        _ => bug!("size_and_align_of_dst: {t} not supported"),
    }
}

//! Computing the size and alignment of a value.

use rustc_abi::{Align, WrappingRange};
use rustc_hir::LangItem;
use rustc_middle::bug;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths};
use rustc_middle::ty::{self, Ty};
use rustc_span::DUMMY_SP;
use tracing::{debug, trace};

use crate::common::IntPredicate;
use crate::traits::*;
use crate::{common, meth};

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
    info: Option<Bx::Value>,
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
    info: Option<Bx::Value>,
) -> (Bx::Value, Bx::Value, Bx::Value) {
    match size_and_align_of_dst_impl(bx, t, info, true) {
        CalculationResult::Unchecked { size, align } => (bx.const_bool(true), size, align),
        CalculationResult::Checked { valid, size, align } => (valid, size, align),
        CalculationResult::Invalid => (bx.const_bool(false), bx.const_usize(0), bx.const_usize(1)),
    }
}

fn size_and_align_of_dst_impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    t: Ty<'tcx>,
    info: Option<Bx::Value>,
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
            let vtable = info.unwrap();
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
        ty::Slice(_) | ty::Str => {
            let unit = layout.field(bx, 0);
            // The info in this case is the length of the str/slice, so the size is that
            // times the unit size.
            if unit.size.bytes() == 0 {
                // If the element size is zero, then the size is zero, and any length is valid.
                let size = bx.const_usize(0);
                let align = bx.const_usize(unit.align.bytes());
                CalculationResult::Unchecked { size, align }
            } else if !checked {
                // In unchecked mode, all slice sizes must fit into `isize`, so this multiplication cannot
                // wrap -- neither signed nor unsigned.
                let size = bx.unchecked_sumul(info.unwrap(), bx.const_usize(unit.size.bytes()));
                let align = bx.const_usize(unit.align.bytes());
                CalculationResult::Unchecked { size, align }
            } else {
                // If we are in checked mode, we need to check if `elem_size * count <= isize::MAX as usize`,
                // but we can't check that after the overflow might occur, so check the equivalent division
                // `count <= isize::MAX as usize / elem_size`, since we know `elem_size > 0` from the above check
                let isize_max = bx.const_usize(
                    bx.data_layout().ptr_sized_integer().signed_max().try_into().unwrap(),
                );
                let elem_size = bx.const_usize(unit.size.bytes());
                let count = info.unwrap();
                let isize_max_div_elem_size = bx.udiv(isize_max, elem_size);
                // The slice is valid if the element
                let valid = bx.icmp(IntPredicate::IntULE, count, isize_max_div_elem_size);
                let size = bx.mul(count, elem_size);
                let align = bx.const_usize(unit.align.bytes());
                CalculationResult::Checked { valid, size, align }
            }
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
            let field_ty = layout.field(bx, i).ty;
            let (mut valid, unsized_size, mut unsized_align) =
                match size_and_align_of_dst_impl(bx, field_ty, info, checked) {
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

//! This file implements "place projections"; basically a symmetric API for 3 types: MPlaceTy, OpTy, PlaceTy.
//!
//! OpTy and PlaceTy generally work by "let's see if we are actually an MPlaceTy, and do something custom if not".
//! For PlaceTy, the custom thing is basically always to call `force_allocation` and then use the MPlaceTy logic anyway.
//! For OpTy, the custom thing on field projections has to be pretty clever (since `Operand::Immediate` can have fields),
//! but for array/slice operations it only has to worry about `Operand::Uninit`. That makes the value part trivial,
//! but we still need to do bounds checking and adjust the layout. To not duplicate that with MPlaceTy, we actually
//! implement the logic on OpTy, and MPlaceTy calls that.

use std::marker::PhantomData;
use std::ops::Range;

use rustc_abi::{self as abi, FieldIdx, OffsetAccuracy, Size, VariantIdx};
use rustc_middle::ty::Ty;
use rustc_middle::ty::layout::TyAndLayout;
use rustc_middle::{bug, mir, span_bug, ty};
use tracing::{debug, instrument};

use super::{
    InterpCx, InterpResult, MPlaceTy, Machine, OpTy, Provenance, Scalar, err_ub, interp_ok,
    throw_ub, throw_unsup,
};
use crate::interpret::ImmTy;
use crate::interpret::eval_context::LayoutComputeSemantics;
use crate::interpret::place::{AnyMemPlaceMeta, MemPlaceMetadata};

/// Describes the constraints placed on offset-projections.
#[derive(Copy, Clone, Debug)]
pub enum OffsetMode {
    /// The offset has to be inbounds, like `ptr::offset`.
    Inbounds,
    /// No constraints, just wrap around the edge of the address space.
    Wrapping,
}

/// A thing that we can project into, and that has a layout.
pub trait Projectable<'tcx, Prov: Provenance>: Sized + std::fmt::Debug {
    /// Get the layout.
    fn layout(&self) -> TyAndLayout<'tcx>;

    /// Get the metadata of a wide value.
    fn meta(&self) -> AnyMemPlaceMeta<'tcx, Prov>;

    /// Get the length of a slice/string/array stored here.
    fn len<M: Machine<'tcx, Provenance = Prov>>(
        &self,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, u64> {
        let layout = self.layout();
        if layout.is_unsized() {
            match layout.ty.kind() {
                // Reachable for arrays of unsized elements
                ty::Array(_, len) => interp_ok(
                    len.try_to_target_usize(*ecx.tcx).expect("expected monomorphic const"),
                ),
                // We need to consult `meta` metadata
                ty::Str => self.meta().scalar(ecx)?.to_target_usize(ecx),
                ty::Slice(..) => {
                    let meta = self.meta().0.unwrap().change_sizedness();
                    let len = ecx.project_simple_field(&meta, FieldIdx::ZERO)?;
                    ecx.read_immediate(&len)?.to_scalar().to_target_usize(ecx)
                }
                _ => bug!("len not supported on unsized type {:?}", layout.ty),
            }
        } else {
            // Go through the layout. There are lots of types that support a length,
            // e.g., SIMD types. (But not all repr(simd) types even have FieldsShape::Array!)
            match layout.fields {
                abi::FieldsShape::Array { count, .. } => interp_ok(count),
                _ => bug!("len not supported on sized type {:?}", layout.ty),
            }
        }
    }

    /// Offset the value by the given amount, replacing the layout and metadata.
    fn offset_with_meta<M: Machine<'tcx, Provenance = Prov>>(
        &self,
        offset: Size,
        mode: OffsetMode,
        meta: AnyMemPlaceMeta<'tcx, Prov>,
        layout: TyAndLayout<'tcx>,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, Self>;

    fn offset<M: Machine<'tcx, Provenance = Prov>>(
        &self,
        offset: Size,
        layout: TyAndLayout<'tcx>,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, Self> {
        assert!(layout.is_sized());
        // We sometimes do pointer arithmetic with this function, disregarding the source type.
        // So we don't check the sizes here.
        self.offset_with_meta(offset, OffsetMode::Inbounds, AnyMemPlaceMeta(None), layout, ecx)
    }

    /// This does an offset-by-zero, which is effectively a transmute. Note however that
    /// not all transmutes are supported by all projectables -- specifically, if this is an
    /// `OpTy` or `ImmTy`, the new layout must have almost the same ABI as the old one
    /// (only changing the `valid_range` is allowed and turning integers into pointers).
    fn transmute<M: Machine<'tcx, Provenance = Prov>>(
        &self,
        layout: TyAndLayout<'tcx>,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, Self> {
        assert!(self.layout().is_sized() && layout.is_sized());
        assert_eq!(self.layout().size, layout.size);
        self.offset_with_meta(Size::ZERO, OffsetMode::Wrapping, AnyMemPlaceMeta(None), layout, ecx)
    }

    /// Convert this to an `OpTy`. This might be an irreversible transformation, but is useful for
    /// reading from this thing. This will never actually do a read from memory!
    fn to_op<M: Machine<'tcx, Provenance = Prov>>(
        &self,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, OpTy<'tcx, M::Provenance>>;
}

/// A type representing iteration over the elements of an array.
pub struct ArrayIterator<'a, 'tcx, Prov: Provenance, P: Projectable<'tcx, Prov>> {
    base: &'a P,
    range: Range<u64>,
    stride: Size,
    field_layout: TyAndLayout<'tcx>,
    _phantom: PhantomData<Prov>, // otherwise it says `Prov` is never used...
}

impl<'a, 'tcx, Prov: Provenance, P: Projectable<'tcx, Prov>> ArrayIterator<'a, 'tcx, Prov, P> {
    /// Should be the same `ecx` on each call, and match the one used to create the iterator.
    pub fn next<M: Machine<'tcx, Provenance = Prov>>(
        &mut self,
        ecx: &InterpCx<'tcx, M>,
    ) -> InterpResult<'tcx, Option<(u64, P)>> {
        let Some(idx) = self.range.next() else { return interp_ok(None) };
        // We use `Wrapping` here since the offset has already been checked when the iterator was created.
        interp_ok(Some((
            idx,
            self.base.offset_with_meta(
                self.stride * idx,
                OffsetMode::Wrapping,
                AnyMemPlaceMeta(None),
                self.field_layout,
                ecx,
            )?,
        )))
    }
}

pub trait MaybeMut<T: ?Sized>: std::ops::Deref<Target = T> {
    fn as_mut(&mut self) -> Option<&mut T>;
    type Reborrow<'a>: MaybeMut<T>
    where
        Self: 'a,
        T: 'a;
    fn reborrow<'a>(&'a mut self) -> Self::Reborrow<'a>
    where
        T: 'a;
}

impl<T: ?Sized> MaybeMut<T> for &T {
    fn as_mut(&mut self) -> Option<&mut T> {
        None
    }

    type Reborrow<'a>
        = &'a T
    where
        Self: 'a,
        T: 'a;

    fn reborrow<'a>(&'a mut self) -> Self::Reborrow<'a>
    where
        T: 'a,
    {
        self
    }
}

impl<T: ?Sized> MaybeMut<T> for &mut T {
    fn as_mut(&mut self) -> Option<&mut T> {
        Some(self)
    }

    type Reborrow<'a>
        = &'a mut T
    where
        Self: 'a,
        T: 'a;

    fn reborrow<'a>(&'a mut self) -> Self::Reborrow<'a>
    where
        T: 'a,
    {
        self
    }
}

// FIXME: Working around https://github.com/rust-lang/rust/issues/54385
impl<'tcx, Prov, M> InterpCx<'tcx, M>
where
    Prov: Provenance,
    M: Machine<'tcx, Provenance = Prov>,
{
    /// Offset a pointer to project to a field of a struct/union. Also return the field's layout.
    /// This supports both struct and array fields, but not slices!
    ///
    /// Takes `impl MaybeMut<Self>` so that it can work with `&Self` in most cases and `&mut Self`
    /// in all others.
    ///
    /// This also works for arrays, but then the `FieldIdx` index type is restricting.
    /// For indexing into arrays, use [`Self::project_index`].
    pub fn project_field_inner<P: Projectable<'tcx, M::Provenance>>(
        mut this: impl MaybeMut<Self>,
        base: &P,
        field: FieldIdx,
    ) -> InterpResult<'tcx, P> {
        // Slices nominally have length 0, so they will panic somewhere in `fields.offset`.
        debug_assert!(
            !matches!(base.layout().ty.kind(), ty::Slice(..)),
            "`field` projection called on a slice -- call `index` projection instead"
        );
        let offset = base.layout().fields.offset(field.as_usize());
        // Computing the layout does normalization, so we get a normalized type out of this
        // even if the field type is non-normalized (possible e.g. via associated types).
        let field_layout = base.layout().field(&*this, field.as_usize());

        let field_meta = match base.meta().0 {
            None => {
                assert!(
                    field_layout.ty.is_thin(*this.tcx, this.typing_env),
                    "non-thin field in thin aggregate"
                );
                AnyMemPlaceMeta(None)
            }
            Some(base_meta) => {
                let base_meta = base_meta.change_sizedness();
                let field_meta = (*this)
                    .project_simple_field(&base_meta, field)?
                    .expect_sized("pointer metadata must be sized");
                AnyMemPlaceMeta(Some(field_meta))
            }
        };

        // FIXME(more_unsized): factor this code out to implement `offset_for_meta!`

        // The simple codepath is used when we don't need to adjust the offset (to
        // the dynamic alignment of the field, or due to it being after an unsized field).
        let is_simple = match offset.accuracy {
            OffsetAccuracy::Exact => true,
            OffsetAccuracy::RoundedUp => offset.guaranteed_zero(),
            OffsetAccuracy::LowerBound => false,
        };
        if is_simple {
            let offset = offset.offset;
            return base.offset_with_meta(
                offset,
                OffsetMode::Inbounds,
                field_meta,
                field_layout,
                &this,
            );
        }

        let Some((offset, _effective_align)) = Self::layout_compute_from_meta_inner(
            this.reborrow(),
            &base.meta(),
            &base.layout(),
            LayoutComputeSemantics::FOR_FIELD_OFFSET,
            crate::interpret::eval_context::LayoutComputeGoal::FieldOffset(field),
        )?
        else {
            // We cannot know the alignment of this field, so we cannot adjust.
            throw_unsup!(UnsizedTypeField)
        };

        base.offset_with_meta(offset, OffsetMode::Inbounds, field_meta, field_layout, &this)
    }

    /// Offset a pointer to project to a field of a struct/union. Unlike `place_field`, this is
    /// always possible without allocating, so it can take `&self`. Also return the field's layout.
    /// This supports both struct and array fields, but not slices!
    ///
    /// This also works for arrays, but then the `FieldIdx` index type is restricting.
    /// For indexing into arrays, use [`Self::project_index`].
    pub fn project_field<P: Projectable<'tcx, M::Provenance>>(
        &mut self,
        base: &P,
        field: FieldIdx,
    ) -> InterpResult<'tcx, P> {
        Self::project_field_inner(self, base, field)
    }

    /// Offset a pointer to project to a field of a struct/union. Unlike `place_field`, this is
    /// always possible without allocating, so it can take `&self`. Also return the field's layout.
    /// This supports both struct and array fields, but not slices!
    ///
    /// This also works for arrays, but then the `FieldIdx` index type is restricting.
    /// For indexing into arrays, use [`Self::project_index`].
    pub fn project_simple_field<P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &P,
        field: FieldIdx,
    ) -> InterpResult<'tcx, P> {
        Self::project_field_inner(self, base, field)
    }

    /// Projects multiple fields at once. See [`Self::project_field`] for details.
    pub fn project_fields<P: Projectable<'tcx, M::Provenance>, const N: usize>(
        &mut self,
        base: &P,
        fields: [FieldIdx; N],
    ) -> InterpResult<'tcx, [P; N]> {
        fields.try_map(|field| self.project_field(base, field))
    }

    /// Projects multiple fields at once. See [`Self::project_field`] for details.
    pub fn project_simple_fields<P: Projectable<'tcx, M::Provenance>, const N: usize>(
        &self,
        base: &P,
        fields: [FieldIdx; N],
    ) -> InterpResult<'tcx, [P; N]> {
        fields.try_map(|field| self.project_simple_field(base, field))
    }

    /// Downcasting to an enum variant.
    pub fn project_downcast<P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &P,
        variant: VariantIdx,
    ) -> InterpResult<'tcx, P> {
        assert!(!base.meta().has_metadata());
        // Downcasts only change the layout.
        // (In particular, no check about whether this is even the active variant -- that's by design,
        // see https://github.com/rust-lang/rust/issues/93688#issuecomment-1032929496.)
        // So we just "offset" by 0.
        let layout = base.layout().for_variant(self, variant);
        // This variant may in fact be uninhabited.
        // See <https://github.com/rust-lang/rust/issues/120337>.

        // This cannot be `transmute` as variants *can* have a smaller size than the entire enum.
        base.offset(Size::ZERO, layout, self)
    }

    /// Compute the offset and field layout for accessing the given index.
    fn project_index_inner<P: Projectable<'tcx, M::Provenance>, I: MaybeMut<Self>>(
        mut this: I,
        base: &P,
        index: u64,
    ) -> InterpResult<'tcx, P> {
        // Not using the layout method because we want to compute on u64
        let (offset, field_layout, field_meta) = match base.layout().fields {
            abi::FieldsShape::Array { stride, count: _ } => {
                // `count` is nonsense for slices, use the dynamic length instead.
                let len = base.len(&*this)?;
                if index >= len {
                    // This can only be reached in ConstProp and non-rustc-MIR.
                    throw_ub!(BoundsCheckFailed { len, index });
                }
                // All fields have the same layout.
                let field_layout = base.layout().field(&*this, 0);

                if field_layout.is_sized() {
                    // With raw slices, `len` can be so big that this *can* overflow.
                    let offset = this
                        .compute_size_in_bytes(stride, index)
                        .ok_or_else(|| err_ub!(PointerArithOverflow))?;
                    (offset, field_layout, AnyMemPlaceMeta(None))
                } else {
                    let field_meta_idx = match base.layout().ty.kind() {
                        ty::Array(..) => FieldIdx::ZERO,
                        ty::Slice(..) => FieldIdx::ONE,
                        _ => bug!("project_index on non-slice non-array unsized type"),
                    };
                    let array_meta = base.meta().0.unwrap().change_sizedness();
                    let field_meta = this
                        .project_simple_field(&array_meta, field_meta_idx)?
                        .expect_sized("pointer metadata must be sized");
                    let field_meta = AnyMemPlaceMeta(Some(field_meta));

                    let (stride, _) = Self::size_and_align_from_meta_inner(
                        this.reborrow(),
                        &field_meta,
                        &field_layout,
                        LayoutComputeSemantics::UNCHECKED_METASIZED_LAYOUT,
                    )?
                    .expect(
                        "size_and_align_from_meta(UNCHECKED_METASIZED_LAYOUT) \
                            should never return None",
                    );

                    // With raw slices, `len` can be so big that this *can* overflow.
                    let offset = this
                        .compute_size_in_bytes(stride, index)
                        .ok_or_else(|| err_ub!(PointerArithOverflow))?;
                    (offset, field_layout, field_meta)
                }
            }
            _ => span_bug!(
                this.cur_span(),
                "`project_index` called on non-array type {:?}",
                base.layout().ty
            ),
        };

        if field_layout.is_sized() {
            base.offset(offset, field_layout, &*this)
        } else {
            base.offset_with_meta(offset, OffsetMode::Inbounds, field_meta, field_layout, &*this)
        }
    }

    pub fn project_index<P: Projectable<'tcx, M::Provenance>>(
        &mut self,
        base: &P,
        index: u64,
    ) -> InterpResult<'tcx, P> {
        Self::project_index_inner(self, base, index)
    }
    pub fn project_simple_index<P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &P,
        index: u64,
    ) -> InterpResult<'tcx, P> {
        Self::project_index_inner(self, base, index)
    }

    /// Converts a repr(simd) value into an array of the right size, such that `project_index`
    /// accesses the SIMD elements. Also returns the number of elements.
    pub fn project_to_simd<P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &P,
    ) -> InterpResult<'tcx, (P, u64)> {
        assert!(base.layout().ty.ty_adt_def().unwrap().repr().simd());
        // SIMD types must be newtypes around arrays, so all we have to do is project to their only field.
        let array = self.project_simple_field(base, FieldIdx::ZERO)?;
        let len = array.len(self)?;
        interp_ok((array, len))
    }

    fn project_constant_index_inner<P: Projectable<'tcx, M::Provenance>>(
        this: impl MaybeMut<Self>,
        base: &P,
        offset: u64,
        min_length: u64,
        from_end: bool,
    ) -> InterpResult<'tcx, P> {
        let n = base.len(&*this)?;
        if n < min_length {
            // This can only be reached in ConstProp and non-rustc-MIR.
            throw_ub!(BoundsCheckFailed { len: min_length, index: n });
        }

        let index = if from_end {
            assert!(0 < offset && offset <= min_length);
            n.checked_sub(offset).unwrap()
        } else {
            assert!(offset < min_length);
            offset
        };

        Self::project_index_inner(this, base, index)
    }

    /// Iterates over all fields of an array. Much more efficient than doing the
    /// same by repeatedly calling `project_index`.
    pub fn project_array_fields<'a, P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &'a P,
    ) -> InterpResult<'tcx, ArrayIterator<'a, 'tcx, M::Provenance, P>> {
        let abi::FieldsShape::Array { stride, .. } = base.layout().fields else {
            span_bug!(
                self.cur_span(),
                "project_array_fields: expected an array layout, got {:#?}",
                base.layout()
            );
        };
        let len = base.len(self)?;
        let field_layout = base.layout().field(self, 0);
        // Ensure that all the offsets are in-bounds once, up-front.
        debug!("project_array_fields: {base:?} {len}");
        base.offset(len * stride, self.layout_of(self.tcx.types.unit).unwrap(), self)?;
        // Create the iterator.
        interp_ok(ArrayIterator {
            base,
            range: 0..len,
            stride,
            field_layout,
            _phantom: PhantomData,
        })
    }

    /// Subslicing
    fn project_subslice<P: Projectable<'tcx, M::Provenance>>(
        &self,
        base: &P,
        from: u64,
        to: u64,
        from_end: bool,
    ) -> InterpResult<'tcx, P> {
        let len = base.len(self)?; // also asserts that we have a type where this makes sense
        let actual_to = if from_end {
            if from.checked_add(to).is_none_or(|to| to > len) {
                // This can only be reached in ConstProp and non-rustc-MIR.
                throw_ub!(BoundsCheckFailed { len, index: from.saturating_add(to) });
            }
            len.checked_sub(to).unwrap()
        } else {
            to
        };

        // Not using layout method because that works with usize, and does not work with slices
        // (that have count 0 in their layout).
        let from_offset = match base.layout().fields {
            abi::FieldsShape::Array { stride, .. } => stride * from, // `Size` multiplication is checked
            _ => {
                span_bug!(
                    self.cur_span(),
                    "unexpected layout of index access: {:#?}",
                    base.layout()
                )
            }
        };

        // Compute meta and new layout
        let inner_len = actual_to.checked_sub(from).unwrap();
        let base_layout = base.layout();
        let (meta, ty) = match base_layout.ty.kind() {
            // It is not nice to match on the type, but that seems to be the only way to
            // implement this.
            ty::Array(elem, _) => {
                let meta = if elem.is_thin(*self.tcx, self.typing_env) {
                    AnyMemPlaceMeta(None)
                } else {
                    // will probably need to change `&self` to `&mut self` and make an alloca
                    // in the general case. Or maybe just `transmute`
                    unimplemented!(
                        "FIXME(ptr_metadata_v2): implement subslice projection for unsized elements"
                    )
                };
                (meta, Ty::new_array(self.tcx.tcx, *elem, inner_len))
            }
            ty::Slice(elem) => {
                let len = Scalar::from_target_usize(inner_len, self);
                let meta_ty = Ty::new_ptr_metadata(*self.tcx, base_layout.ty);
                let meta = if elem.is_thin(*self.tcx, self.typing_env) {
                    let meta = ImmTy::from_scalar(len, self.layout_of(meta_ty)?);
                    let meta = OpTy::from(meta).expect_sized("ptr metadata must be sized");
                    AnyMemPlaceMeta(Some(meta))
                } else {
                    // will probably need to change `&self` to `&mut self` and make an alloca
                    // in the general case
                    unimplemented!(
                        "FIXME(ptr_metadata_v2): implement subslice projection for unsized elements"
                    )
                };
                (meta, base_layout.ty)
            }
            _ => {
                span_bug!(
                    self.cur_span(),
                    "cannot subslice non-array type: `{:?}`",
                    base.layout().ty
                )
            }
        };
        let layout = self.layout_of(ty)?;

        base.offset_with_meta(from_offset, OffsetMode::Inbounds, meta, layout, self)
    }

    fn project_inner<P, I>(
        this: I,
        base: &P,
        proj_elem: mir::PlaceElem<'tcx>,
    ) -> InterpResult<'tcx, P>
    where
        P: Projectable<'tcx, M::Provenance> + From<MPlaceTy<'tcx, M::Provenance>> + std::fmt::Debug,
        I: MaybeMut<Self>,
    {
        use rustc_middle::mir::ProjectionElem::*;
        interp_ok(match proj_elem {
            OpaqueCast(ty) => {
                span_bug!(this.cur_span(), "OpaqueCast({ty}) encountered after borrowck")
            }
            UnwrapUnsafeBinder(target) => base.transmute(this.layout_of(target)?, &*this)?,
            Field(field, _) => Self::project_field_inner(this, base, field)?,
            Downcast(_, variant) => this.project_downcast(base, variant)?,
            Deref => this.deref_pointer(&base.to_op(&*this)?)?.into(),
            Index(local) => {
                let layout = this.layout_of(this.tcx.types.usize)?;
                let n = this.local_to_op(local, Some(layout))?;
                let n = this.read_target_usize(&n)?;
                Self::project_index_inner(this, base, n)?
            }
            ConstantIndex { offset, min_length, from_end } => {
                Self::project_constant_index_inner(this, base, offset, min_length, from_end)?
            }
            Subslice { from, to, from_end } => this.project_subslice(base, from, to, from_end)?,
        })
    }

    /// Applying a general projection
    #[instrument(skip(self), level = "trace")]
    pub fn project<P>(&mut self, base: &P, proj_elem: mir::PlaceElem<'tcx>) -> InterpResult<'tcx, P>
    where
        P: Projectable<'tcx, M::Provenance> + From<MPlaceTy<'tcx, M::Provenance>> + std::fmt::Debug,
    {
        Self::project_inner(self, base, proj_elem)
    }

    /// Applying a general projection
    #[instrument(skip(self), level = "trace")]
    pub fn project_simple<P>(
        &self,
        base: &P,
        proj_elem: mir::PlaceElem<'tcx>,
    ) -> InterpResult<'tcx, P>
    where
        P: Projectable<'tcx, M::Provenance> + From<MPlaceTy<'tcx, M::Provenance>> + std::fmt::Debug,
    {
        Self::project_inner(self, base, proj_elem)
    }
}

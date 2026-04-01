use std::ops::Deref as _;

use rustc_abi::{
    Align, BackendRepr, FieldIdx, FieldsShape, OffsetAccuracy, Size, TagEncoding, VariantIdx,
    Variants,
};
use rustc_middle::mir::PlaceTy;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::ty::layout::{HasTyCtxt, HasTypingEnv, LayoutOf, TyAndLayout};
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_middle::{bug, mir};
use tracing::{debug, instrument};

use super::operand::OperandValue;
use super::{FunctionCx, LocalRef};
use crate::common::IntPredicate;
use crate::mir::operand::OperandRef;
use crate::mir::{AnyPlace, AnyPlaceMeta, PlaceMetadata, PlaceSizedness, SizedPlace};
use crate::size_of_val;
use crate::traits::*;

/// The location and extra runtime properties of the place.
///
/// Typically found in a [`PlaceRef`] or an [`OperandValue::Ref`].
///
/// As a location in memory, this has no specific type. If you want to
/// load or store it using a typed operation, use [`Self::with_type`].
#[derive(Copy, Clone, Debug)]
pub struct PlaceValue<'tcx, V: CodegenObject, Sizedness: PlaceSizedness = AnyPlace> {
    /// A pointer to the contents of the place.
    pub llval: V,

    /// This place's extra data if it is unsized, or `None` if null.
    pub llextra: Sizedness::Metadata<'tcx, V>,

    /// The alignment we know for this place.
    pub align: Align,
}

impl<'tcx, V: CodegenObject, Sizedness: PlaceSizedness> PlaceValue<'tcx, V, Sizedness> {
    /// Constructor for the ordinary case of `Sized` types.
    ///
    /// Sets `llextra` to `None`.
    pub fn new_sized(llval: V, align: Align) -> Self {
        PlaceValue { llval, llextra: Default::default(), align }
    }

    /// Allocates a stack slot in the function for a value
    /// of the specified size and alignment.
    ///
    /// The allocation itself is untyped.
    pub fn alloca<'a, Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        bx: &mut Bx,
        size: Size,
        align: Align,
    ) -> Self {
        let llval = bx.alloca(size, align);
        PlaceValue::new_sized(llval, align)
    }

    /// If this PlaceValue has no metadata, return it as a PlaceValue with SizedPlaceMeta
    pub fn try_to_sized(self) -> Option<PlaceValue<'tcx, V, SizedPlace>> {
        Some(PlaceValue {
            llval: self.llval,
            llextra: self.llextra.try_to_sized()?,
            align: self.align,
        })
    }

    /// If this PlaceValue has no metadata, return it as a PlaceValue with SizedPlaceMeta,
    /// otherwise panic
    pub fn expect_sized(self, msg: &str) -> PlaceValue<'tcx, V, SizedPlace> {
        self.try_to_sized().expect(msg)
    }

    pub fn change_sizedness<NewSizedness: PlaceSizedness>(self) -> PlaceValue<'tcx, V, NewSizedness>
    where
        Sizedness::Metadata<'tcx, V>: Into<NewSizedness::Metadata<'tcx, V>>,
    {
        PlaceValue { llval: self.llval, llextra: self.llextra.into(), align: self.align }
    }

    /// Creates a `PlaceRef` to this location with the given type.
    pub fn with_type(self, layout: TyAndLayout<'tcx>) -> PlaceRef<'tcx, V, Sizedness> {
        assert!(
            layout.is_unsized() || layout.is_uninhabited() || !self.llextra.has_metadata(),
            "Had pointer metadata {:?} for sized type {layout:?}",
            self.llextra,
        );
        PlaceRef { val: self, layout }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct PlaceRef<'tcx, V: CodegenObject, Sizedness: PlaceSizedness = AnyPlace> {
    /// The location and extra runtime properties of the place.
    pub val: PlaceValue<'tcx, V, Sizedness>,

    /// The monomorphized type of this place, including variant information.
    ///
    /// You probably shouldn't use the alignment from this layout;
    /// rather you should use the `.val.align` of the actual place,
    /// which might be different from the type's normal alignment.
    pub layout: TyAndLayout<'tcx>,
}

impl<'tcx, V: CodegenObject, Sizedness: PlaceSizedness> PlaceRef<'tcx, V, Sizedness> {
    pub fn try_to_sized(self) -> Option<PlaceRef<'tcx, V, SizedPlace>> {
        Some(PlaceRef { val: self.val.try_to_sized()?, layout: self.layout })
    }

    #[track_caller]
    pub fn expect_sized(self, msg: &str) -> PlaceRef<'tcx, V, SizedPlace> {
        self.try_to_sized().expect(msg)
    }

    pub fn change_sizedness<NewSizedness: PlaceSizedness>(self) -> PlaceRef<'tcx, V, NewSizedness>
    where
        Sizedness::Metadata<'tcx, V>: Into<NewSizedness::Metadata<'tcx, V>>,
    {
        PlaceRef { val: self.val.change_sizedness(), layout: self.layout }
    }
}

impl<'a, 'tcx, V: CodegenObject> PlaceRef<'tcx, V> {
    pub fn new_sized(llval: V, layout: TyAndLayout<'tcx>) -> Self {
        PlaceRef::new_sized_aligned(llval, layout, layout.align.abi)
    }

    pub fn new_sized_aligned(llval: V, layout: TyAndLayout<'tcx>, align: Align) -> Self {
        assert!(layout.is_sized());
        PlaceValue::new_sized(llval, align).with_type(layout)
    }

    // FIXME(eddyb) pass something else for the name so no work is done
    // unless LLVM IR names are turned on (e.g. for `--emit=llvm-ir`).
    pub fn alloca<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        bx: &mut Bx,
        layout: TyAndLayout<'tcx>,
    ) -> Self {
        if layout.deref().is_scalable_vector() {
            Self::alloca_scalable(bx, layout)
        } else {
            Self::alloca_size(bx, layout.size, layout)
        }
    }

    pub fn alloca_size<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        bx: &mut Bx,
        size: Size,
        layout: TyAndLayout<'tcx>,
    ) -> Self {
        assert!(layout.is_sized(), "tried to statically allocate unsized place");
        PlaceValue::alloca(bx, size, layout.align.abi).with_type(layout)
    }

    /// Returns a place for an indirect reference to an unsized place.
    // FIXME(eddyb) pass something else for the name so no work is done
    // unless LLVM IR names are turned on (e.g. for `--emit=llvm-ir`).
    pub fn alloca_unsized_indirect<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        bx: &mut Bx,
        layout: TyAndLayout<'tcx>,
    ) -> Self {
        assert!(layout.is_unsized(), "tried to allocate indirect place for sized values");
        let ptr_ty = Ty::new_mut_ptr(bx.cx().tcx(), layout.ty);
        let ptr_layout = bx.cx().layout_of(ptr_ty);
        Self::alloca(bx, ptr_layout)
    }

    pub fn len<Bx: BuilderMethods<'a, 'tcx, Value = V>>(&self, bx: &mut Bx) -> V {
        if let FieldsShape::Array { count, .. } = self.layout.fields {
            if self.layout.is_unsized() {
                assert_eq!(count, 0);
                let slice_meta = self.val.llextra.0.unwrap().change_sizedness();
                let slice_len = slice_meta.extract_or_load_field(bx, 0);
                slice_len.immediate()
            } else {
                bx.const_usize(count)
            }
        } else {
            bug!("unexpected layout `{:#?}` in PlaceRef::len", self.layout)
        }
    }

    fn alloca_scalable<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        bx: &mut Bx,
        layout: TyAndLayout<'tcx>,
    ) -> Self {
        PlaceValue::new_sized(bx.alloca_with_ty(layout), layout.align.abi).with_type(layout)
    }
}

impl<'a, 'tcx, V: CodegenObject> PlaceRef<'tcx, V> {
    /// Access a field, at a point when the value's case is known.
    ///
    /// If `fx` is `None`, then attempting to perform a projection that would require
    /// computing the layout of an `unsized type` will ICE.
    pub fn project_field<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        self,
        fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
        bx: &mut Bx,
        ix: usize,
    ) -> Self {
        let field = self.layout.field(bx.cx(), ix);
        let offset = self.layout.fields.offset(ix);

        let field_llextra = if bx.cx().tcx().type_has_metadata(field.ty, bx.cx().typing_env()) {
            debug_assert!(
                self.val.llextra.has_metadata(),
                "field projection from thin container to non-thin field",
            );
            self.val.llextra.map_metadata(|meta| {
                let meta = meta.change_sizedness();
                meta.extract_or_load_field(bx, ix).expect_sized("pointer metadata must be sized")
            })
        } else {
            AnyPlaceMeta(None)
        };

        // The simple codepath is used when we don't need to adjust the offset (to
        // the dynamic alignment of the field, or due to it being after an unsized field).
        let is_simple = match offset.accuracy {
            OffsetAccuracy::Exact => true,
            OffsetAccuracy::RoundedUp => offset.guaranteed_zero(),
            OffsetAccuracy::LowerBound => false,
        };
        if is_simple {
            let offset = offset.offset;
            let effective_field_align = self.val.align.restrict_for_offset(offset);
            let llval = if offset.bytes() == 0 {
                self.val.llval
            } else {
                bx.inbounds_ptradd(self.val.llval, bx.const_usize(offset.bytes()))
            };
            let val = PlaceValue { llval, llextra: field_llextra, align: effective_field_align };
            return val.with_type(field);
        } else if matches!(offset.accuracy, OffsetAccuracy::RoundedUp) {
            // We *only* need to adjust for the dynamic alignment in this case
            // We need to get the pointer manually now.
            // We do this by casting to a `*i8`, then offsetting it by the appropriate amount.
            // We do this instead of, say, simply adjusting the pointer from the result of a GEP
            // because the field may have an arbitrary alignment in the LLVM representation.
            //
            // To demonstrate:
            //
            //     struct Foo<T: ?Sized> {
            //         x: u16,
            //         y: T
            //     }
            //
            // The type `Foo<Foo<Trait>>` is represented in LLVM as `{ u16, { u16, u8 }}`, meaning that
            // the `y` field has 16-bit alignment.
            let offset = offset.offset;
            // This is not necessarily exactly correct, but is a lower-bound.
            // If the field is dynamically aligned higher than it's offset would satisfy,
            // then we will round up the offset which can never *decrease* the effective alignment.
            let effective_field_align = self.val.align.restrict_for_offset(offset);

            let unaligned_offset = bx.cx().const_usize(offset.bytes());

            // Get the alignment of the field
            let (_, mut unsized_align) =
                size_of_val::size_and_align_of_dst(fx, bx, field.ty, field_llextra);

            // For packed types, we need to cap alignment.
            if let ty::Adt(def, _) = self.layout.ty.kind()
                && let Some(packed) = def.repr().pack
            {
                let packed = bx.const_usize(packed.bytes());
                let cmp = bx.icmp(IntPredicate::IntULT, unsized_align, packed);
                unsized_align = bx.select(cmp, unsized_align, packed)
            }

            // Bump the unaligned offset up to the appropriate alignment
            let offset = round_up_const_value_to_alignment(bx, unaligned_offset, unsized_align);

            debug!("struct_field_ptr: DST field offset: {:?}", offset);

            // Adjust pointer.
            let ptr = bx.inbounds_ptradd(self.val.llval, offset);
            let val =
                PlaceValue { llval: ptr, llextra: field_llextra, align: effective_field_align };
            val.with_type(field)
        } else {
            // We need to get the pointer manually now.
            // We do this by casting to a `*i8`, then offsetting it by the appropriate (dynamically computed) amount.
            let (offset, _field_align) = crate::size_of_val::field_offset_for_dst(
                fx,
                bx,
                self.layout.ty,
                self.val.llextra,
                FieldIdx::from_usize(ix),
            );
            // Adjust pointer.
            let ptr = bx.inbounds_ptradd(self.val.llval, offset);
            let val = PlaceValue { llval: ptr, llextra: field_llextra, align: Align::ONE };
            return val.with_type(field);
        }
    }

    /// Sets the discriminant for a new value of the given case of the given
    /// representation.
    pub fn codegen_set_discr<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        &self,
        bx: &mut Bx,
        variant_index: VariantIdx,
    ) {
        match codegen_tag_value(bx.cx(), variant_index, self.layout) {
            Err(UninhabitedVariantError) => {
                // We play it safe by using a well-defined `abort`, but we could go for immediate UB
                // if that turns out to be helpful.
                bx.abort();
            }
            Ok(Some((tag_field, imm))) => {
                let tag_place = self.project_field(None, bx, tag_field.as_usize());
                OperandValue::Immediate(imm).store(bx, tag_place);
            }
            Ok(None) => {}
        }
    }

    /// If `fx` is `None`, then attempting to perform a projection that would require
    /// computing the layout of an `unsized type` will ICE.
    pub fn project_index<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        &self,
        fx: Option<&mut FunctionCx<'a, 'tcx, Bx>>,
        bx: &mut Bx,
        llindex: V,
    ) -> Self {
        let layout = self.layout.field(bx, 0);
        if layout.is_sized() {
            // Statically compute the offset if we can, otherwise just use the element size,
            // as this will yield the lowest alignment.
            let offset = if let Some(llindex) = bx.const_to_opt_uint(llindex) {
                layout.size.checked_mul(llindex, bx).unwrap_or(layout.size)
            } else {
                layout.size
            };

            let llval =
                bx.inbounds_nuw_gep(bx.cx().backend_type(layout), self.val.llval, &[llindex]);
            let align = self.val.align.restrict_for_offset(offset);
            PlaceValue::new_sized(llval, align).with_type(layout)
        } else {
            let (&elem_ty, elem_meta_idx) = match self.layout.ty.kind() {
                ty::Array(elem_ty, _) => (elem_ty, 0),
                ty::Slice(elem_ty) => (elem_ty, 1),
                _ => bug!("project_index on non-slice non-array type"),
            };
            let array_meta = self.val.llextra.0.unwrap().change_sizedness();
            let elem_meta = array_meta.extract_or_load_field(bx, elem_meta_idx);
            let elem_meta =
                AnyPlaceMeta(Some(elem_meta.expect_sized("pointer metadata must be sized")));

            let (elem_size, _elem_align) =
                size_of_val::size_and_align_of_dst(fx, bx, elem_ty, elem_meta);

            let byte_idx = bx.mul(elem_size, llindex);

            let llval = bx.inbounds_nuw_gep(bx.cx().type_i8(), self.val.llval, &[byte_idx]);
            PlaceValue { llval, llextra: elem_meta, align: layout.align.abi }.with_type(layout)
        }
    }

    pub fn project_downcast<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        &self,
        bx: &mut Bx,
        variant_index: VariantIdx,
    ) -> Self {
        let mut downcast = *self;
        downcast.layout = self.layout.for_variant(bx.cx(), variant_index);
        downcast
    }

    pub fn project_type<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        &self,
        bx: &mut Bx,
        ty: Ty<'tcx>,
    ) -> Self {
        let mut downcast = *self;
        downcast.layout = bx.cx().layout_of(ty);
        downcast
    }

    pub fn storage_live<Bx: BuilderMethods<'a, 'tcx, Value = V>>(&self, bx: &mut Bx) {
        bx.lifetime_start(self.val.llval, self.layout.size);
    }

    pub fn storage_dead<Bx: BuilderMethods<'a, 'tcx, Value = V>>(&self, bx: &mut Bx) {
        bx.lifetime_end(self.val.llval, self.layout.size);
    }

    pub fn address<Bx: BuilderMethods<'a, 'tcx, Value = V>>(
        &self,
        bx: &mut Bx,
        mk_ptr_ty: impl FnOnce(TyCtxt<'tcx>, Ty<'tcx>) -> Ty<'tcx>,
    ) -> OperandRef<'tcx, V> {
        let pointer_ty = mk_ptr_ty(bx.tcx(), self.layout.ty);
        let pointer_layout = bx.layout_of(pointer_ty);

        let AnyPlaceMeta(Some(meta)) = self.val.llextra else {
            return OperandRef {
                val: OperandValue::Immediate(self.val.llval),
                layout: pointer_layout,
                move_annotation: None,
            };
        };

        match meta.val {
            OperandValue::ZeroSized => OperandRef {
                val: OperandValue::Immediate(self.val.llval),
                layout: pointer_layout,
                move_annotation: None,
            },
            OperandValue::Immediate(llextra) => OperandRef {
                val: OperandValue::Pair(self.val.llval, llextra),
                layout: pointer_layout,
                move_annotation: None,
            },
            meta_val => {
                let ptr_place = PlaceRef::alloca(bx, pointer_layout);
                let ptr_data = ptr_place.project_field(None, bx, 0);
                let ptr_meta = ptr_place.project_field(None, bx, 1);

                OperandValue::Immediate(self.val.llval).store(bx, ptr_data);
                meta_val.change_sizedness().store(bx, ptr_meta);

                bx.load_operand(ptr_place)
            }
        }
    }
}

impl<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>> FunctionCx<'a, 'tcx, Bx> {
    #[instrument(level = "trace", skip(self, bx))]
    pub fn codegen_place(
        &mut self,
        bx: &mut Bx,
        place_ref: mir::PlaceRef<'tcx>,
    ) -> PlaceRef<'tcx, Bx::Value, AnyPlace> {
        let tcx = self.cx.tcx();

        let mut base = 0;
        let mut cg_base = match self.locals[place_ref.local] {
            LocalRef::Place(place) => place.change_sizedness(),
            LocalRef::UnsizedPlace(place) => bx.load_operand(place).deref(bx),
            LocalRef::Operand(..) => {
                if place_ref.is_indirect_first_projection() {
                    base = 1;
                    let cg_base = self.codegen_consume(
                        bx,
                        mir::PlaceRef { projection: &place_ref.projection[..0], ..place_ref },
                    );
                    cg_base.deref(bx)
                } else {
                    bug!("using operand local {:?} as place", place_ref);
                }
            }
            LocalRef::PendingOperand => {
                bug!("using still-pending operand local {:?} as place", place_ref);
            }
        };
        for elem in place_ref.projection[base..].iter() {
            cg_base = match *elem {
                mir::ProjectionElem::Deref => bx.load_operand(cg_base).deref(bx),
                mir::ProjectionElem::Field(ref field, _) => {
                    cg_base.project_field(Some(self), bx, field.index())
                }
                mir::ProjectionElem::OpaqueCast(ty) => {
                    bug!("encountered OpaqueCast({ty}) in codegen")
                }
                mir::ProjectionElem::UnwrapUnsafeBinder(ty) => {
                    cg_base.project_type(bx, self.monomorphize(ty))
                }
                mir::ProjectionElem::Index(index) => {
                    let index = &mir::Operand::Copy(mir::Place::from(index));
                    let index = self.codegen_operand(bx, index);
                    let llindex = index.immediate();
                    cg_base.project_index(Some(self), bx, llindex)
                }
                mir::ProjectionElem::ConstantIndex { offset, from_end: false, min_length: _ } => {
                    let lloffset = bx.cx().const_usize(offset);
                    cg_base.project_index(Some(self), bx, lloffset)
                }
                mir::ProjectionElem::ConstantIndex { offset, from_end: true, min_length: _ } => {
                    let lloffset = bx.cx().const_usize(offset);
                    let lllen = cg_base.len(bx);
                    let llindex = bx.sub(lllen, lloffset);
                    cg_base.project_index(Some(self), bx, llindex)
                }
                mir::ProjectionElem::Subslice { from, to, from_end } => {
                    let elem_ty = match cg_base.layout.ty.kind() {
                        ty::Array(elem_ty, _) => *elem_ty,
                        ty::Slice(elem_ty) => *elem_ty,
                        _ => unreachable!(),
                    };
                    if !elem_ty.is_thin(bx.tcx(), bx.typing_env()) {
                        unimplemented!(
                            "FIXME(ptr_metadata_v2): implement subslice projection for unsized elements"
                        );
                    }
                    let mut subslice = cg_base
                        .project_index(Some(self), bx, bx.cx().const_usize(from))
                        .change_sizedness();
                    let projected_ty =
                        PlaceTy::from_ty(cg_base.layout.ty).projection_ty(tcx, *elem).ty;
                    subslice.layout = bx.cx().layout_of(self.monomorphize(projected_ty));

                    if subslice.layout.is_unsized() {
                        assert!(from_end, "slice subslices should be `from_end`");
                        let len =
                            bx.sub(cg_base.val.llextra.immediate(), bx.cx().const_usize(from + to));
                        subslice.val.llextra = AnyPlaceMeta(Some(OperandRef {
                            val: OperandValue::Immediate(len),
                            layout: bx
                                .cx()
                                .layout_of(Ty::new_ptr_metadata(bx.tcx(), subslice.layout.ty)),
                            move_annotation: None,
                        }));
                    }

                    subslice
                }
                mir::ProjectionElem::Downcast(_, v) => cg_base.project_downcast(bx, v),
            };
        }
        debug!("codegen_place(place={:?}) => {:?}", place_ref, cg_base);
        cg_base
    }

    pub fn monomorphized_place_ty(&self, place_ref: mir::PlaceRef<'tcx>) -> Ty<'tcx> {
        let tcx = self.cx.tcx();
        let place_ty = place_ref.ty(self.mir, tcx);
        self.monomorphize(place_ty.ty)
    }
}

fn round_up_const_value_to_alignment<'a, 'tcx, Bx: BuilderMethods<'a, 'tcx>>(
    bx: &mut Bx,
    value: Bx::Value,
    align: Bx::Value,
) -> Bx::Value {
    // In pseudo code:
    //
    //     if value & (align - 1) == 0 {
    //         value
    //     } else {
    //         (value & !(align - 1)) + align
    //     }
    //
    // Usually this is written without branches as
    //
    //     (value + align - 1) & !(align - 1)
    //
    // But this formula cannot take advantage of constant `value`. E.g. if `value` is known
    // at compile time to be `1`, this expression should be optimized to `align`. However,
    // optimization only holds if `align` is a power of two. Since the optimizer doesn't know
    // that `align` is a power of two, it cannot perform this optimization.
    //
    // Instead we use
    //
    //     value + (-value & (align - 1))
    //
    // Since `align` is used only once, the expression can be optimized. For `value = 0`
    // its optimized to `0` even in debug mode.
    //
    // NB: The previous version of this code used
    //
    //     (value + align - 1) & -align
    //
    // Even though `-align == !(align - 1)`, LLVM failed to optimize this even for
    // `value = 0`. Bug report: https://bugs.llvm.org/show_bug.cgi?id=48559
    let one = bx.const_usize(1);
    let align_minus_1 = bx.sub(align, one);
    let neg_value = bx.neg(value);
    let offset = bx.and(neg_value, align_minus_1);
    bx.add(value, offset)
}

/// Calculates the value that needs to be stored to mark the discriminant.
///
/// This might be `None` for a `struct` or a niched variant (like `Some(&3)`).
///
/// If it's `Some`, it returns the value to store and the field in which to
/// store it. Note that this value is *not* the same as the discriminant, in
/// general, as it might be a niche value or have a different size.
///
/// It might also be an `Err` because the variant is uninhabited.
pub(super) fn codegen_tag_value<'tcx, V>(
    cx: &impl CodegenMethods<'tcx, Value = V>,
    variant_index: VariantIdx,
    layout: TyAndLayout<'tcx>,
) -> Result<Option<(FieldIdx, V)>, UninhabitedVariantError> {
    // By checking uninhabited-ness first we don't need to worry about types
    // like `(u32, !)` which are single-variant but weird.
    if layout.for_variant(cx, variant_index).is_uninhabited() {
        return Err(UninhabitedVariantError);
    }

    Ok(match layout.variants {
        Variants::Empty => unreachable!("we already handled uninhabited types"),
        Variants::Single { index } => {
            assert_eq!(index, variant_index);
            None
        }

        Variants::Multiple { tag_encoding: TagEncoding::Direct, tag_field, .. } => {
            let discr = layout.ty.discriminant_for_variant(cx.tcx(), variant_index);
            let to = discr.unwrap().val;
            let tag_layout = layout.field(cx, tag_field.as_usize());
            let tag_llty = cx.immediate_backend_type(tag_layout);
            let imm = cx.const_uint_big(tag_llty, to);
            Some((tag_field, imm))
        }
        Variants::Multiple {
            tag_encoding: TagEncoding::Niche { untagged_variant, ref niche_variants, niche_start },
            tag_field,
            ..
        } => {
            if variant_index != untagged_variant {
                let niche_layout = layout.field(cx, tag_field.as_usize());
                let niche_llty = cx.immediate_backend_type(niche_layout);
                let BackendRepr::Scalar(scalar) = niche_layout.backend_repr else {
                    bug!("expected a scalar placeref for the niche");
                };
                // We are supposed to compute `niche_value.wrapping_add(niche_start)` wrapping
                // around the `niche`'s type.
                // The easiest way to do that is to do wrapping arithmetic on `u128` and then
                // masking off any extra bits that occur because we did the arithmetic with too many bits.
                let niche_value = variant_index.as_u32() - niche_variants.start().as_u32();
                let niche_value = (niche_value as u128).wrapping_add(niche_start);
                let niche_value = niche_value & niche_layout.size.unsigned_int_max();

                let niche_llval = cx.scalar_to_backend(
                    Scalar::from_uint(niche_value, niche_layout.size),
                    scalar,
                    niche_llty,
                );
                Some((tag_field, niche_llval))
            } else {
                None
            }
        }
    })
}

#[derive(Debug)]
pub(super) struct UninhabitedVariantError;

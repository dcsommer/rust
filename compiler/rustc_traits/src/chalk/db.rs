//! Provides the `RustIrDatabase` implementation for `chalk-solve`
//!
//! The purpose of the `chalk_solve::RustIrDatabase` is to get data about
//! specific types, such as bounds, where clauses, or fields. This file contains
//! the minimal logic to assemble the types for `chalk-solve` by calling out to
//! either the `TyCtxt` (for information about types) or
//! `crate::chalk::lowering` (to lower rustc types into Chalk types).

use rustc_middle::traits::ChalkRustInterner as RustInterner;
use rustc_middle::ty::subst::{InternalSubsts, Subst, SubstsRef};
use rustc_middle::ty::{self, AssocKind, EarlyBinder, Ty, TyCtxt, TypeFoldable, TypeSuperFoldable};

use rustc_ast::ast;
use rustc_attr as attr;

use rustc_hir::def_id::DefId;

use rustc_span::symbol::sym;

use std::fmt;
use std::sync::Arc;

use crate::chalk::lowering::LowerInto;

pub struct RustIrDatabase<'tcx> {
    pub(crate) interner: RustInterner<'tcx>,
}

impl fmt::Debug for RustIrDatabase<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RustIrDatabase")
    }
}

impl<'tcx> RustIrDatabase<'tcx> {
    fn where_clauses_for(
        &self,
        def_id: DefId,
        bound_vars: SubstsRef<'tcx>,
    ) -> Vec<chalk_ir::QuantifiedWhereClause<RustInterner<'tcx>>> {
        let predicates = self.interner.tcx.predicates_defined_on(def_id).predicates;
        predicates
            .iter()
            .map(|(wc, _)| EarlyBinder(*wc).subst(self.interner.tcx, bound_vars))
            .filter_map(|wc| LowerInto::<
                    Option<chalk_ir::QuantifiedWhereClause<RustInterner<'tcx>>>
                    >::lower_into(wc, self.interner)).collect()
    }

    fn bounds_for<T>(&self, def_id: DefId, bound_vars: SubstsRef<'tcx>) -> Vec<T>
    where
        ty::Predicate<'tcx>: LowerInto<'tcx, std::option::Option<T>>,
    {
        self.interner
            .tcx
            .explicit_item_bounds(def_id)
            .iter()
            .map(|(bound, _)| EarlyBinder(*bound).subst(self.interner.tcx, &bound_vars))
            .filter_map(|bound| LowerInto::<Option<_>>::lower_into(bound, self.interner))
            .collect()
    }
}

impl<'tcx> chalk_solve::RustIrDatabase<RustInterner<'tcx>> for RustIrDatabase<'tcx> {
    fn interner(&self) -> RustInterner<'tcx> {
        self.interner
    }

    fn associated_ty_data(
        &self,
        assoc_type_id: chalk_ir::AssocTypeId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::AssociatedTyDatum<RustInterner<'tcx>>> {
        let def_id = assoc_type_id.0;
        let assoc_item = self.interner.tcx.associated_item(def_id);
        let Some(trait_def_id) = assoc_item.trait_container(self.interner.tcx) else {
            unimplemented!("Not possible??");
        };
        match assoc_item.kind {
            AssocKind::Type => {}
            _ => unimplemented!("Not possible??"),
        }
        let bound_vars = bound_vars_for_item(self.interner.tcx, def_id);
        let binders = binders_for(self.interner, bound_vars);

        let where_clauses = self.where_clauses_for(def_id, bound_vars);
        let bounds = self.bounds_for(def_id, bound_vars);

        Arc::new(chalk_solve::rust_ir::AssociatedTyDatum {
            trait_id: chalk_ir::TraitId(trait_def_id),
            id: assoc_type_id,
            name: (),
            binders: chalk_ir::Binders::new(
                binders,
                chalk_solve::rust_ir::AssociatedTyDatumBound { bounds, where_clauses },
            ),
        })
    }

    fn trait_datum(
        &self,
        trait_id: chalk_ir::TraitId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::TraitDatum<RustInterner<'tcx>>> {
        use chalk_solve::rust_ir::WellKnownTrait::*;

        let def_id = trait_id.0;
        let trait_def = self.interner.tcx.trait_def(def_id);

        let bound_vars = bound_vars_for_item(self.interner.tcx, def_id);
        let binders = binders_for(self.interner, bound_vars);

        let where_clauses = self.where_clauses_for(def_id, bound_vars);

        let associated_ty_ids: Vec<_> = self
            .interner
            .tcx
            .associated_items(def_id)
            .in_definition_order()
            .filter(|i| i.kind == AssocKind::Type)
            .map(|i| chalk_ir::AssocTypeId(i.def_id))
            .collect();

        let lang_items = self.interner.tcx.lang_items();
        let well_known = if lang_items.sized_trait() == Some(def_id) {
            Some(Sized)
        } else if lang_items.copy_trait() == Some(def_id) {
            Some(Copy)
        } else if lang_items.clone_trait() == Some(def_id) {
            Some(Clone)
        } else if lang_items.drop_trait() == Some(def_id) {
            Some(Drop)
        } else if lang_items.fn_trait() == Some(def_id) {
            Some(Fn)
        } else if lang_items.fn_once_trait() == Some(def_id) {
            Some(FnOnce)
        } else if lang_items.fn_mut_trait() == Some(def_id) {
            Some(FnMut)
        } else if lang_items.unsize_trait() == Some(def_id) {
            Some(Unsize)
        } else if lang_items.unpin_trait() == Some(def_id) {
            Some(Unpin)
        } else if lang_items.coerce_unsized_trait() == Some(def_id) {
            Some(CoerceUnsized)
        } else if lang_items.dispatch_from_dyn_trait() == Some(def_id) {
            Some(DispatchFromDyn)
        } else {
            None
        };
        Arc::new(chalk_solve::rust_ir::TraitDatum {
            id: trait_id,
            binders: chalk_ir::Binders::new(
                binders,
                chalk_solve::rust_ir::TraitDatumBound { where_clauses },
            ),
            flags: chalk_solve::rust_ir::TraitFlags {
                auto: trait_def.has_auto_impl,
                marker: trait_def.is_marker,
                upstream: !def_id.is_local(),
                fundamental: self.interner.tcx.has_attr(def_id, sym::fundamental),
                non_enumerable: true,
                coinductive: false,
            },
            associated_ty_ids,
            well_known,
        })
    }

    fn adt_datum(
        &self,
        adt_id: chalk_ir::AdtId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::AdtDatum<RustInterner<'tcx>>> {
        let adt_def = adt_id.0;

        let bound_vars = bound_vars_for_item(self.interner.tcx, adt_def.did());
        let binders = binders_for(self.interner, bound_vars);

        let where_clauses = self.where_clauses_for(adt_def.did(), bound_vars);

        let variants: Vec<_> = adt_def
            .variants()
            .iter()
            .map(|variant| chalk_solve::rust_ir::AdtVariantDatum {
                fields: variant
                    .fields
                    .iter()
                    .map(|field| field.ty(self.interner.tcx, bound_vars).lower_into(self.interner))
                    .collect(),
            })
            .collect();
        Arc::new(chalk_solve::rust_ir::AdtDatum {
            id: adt_id,
            binders: chalk_ir::Binders::new(
                binders,
                chalk_solve::rust_ir::AdtDatumBound { variants, where_clauses },
            ),
            flags: chalk_solve::rust_ir::AdtFlags {
                upstream: !adt_def.did().is_local(),
                fundamental: adt_def.is_fundamental(),
                phantom_data: adt_def.is_phantom_data(),
            },
            kind: match adt_def.adt_kind() {
                ty::AdtKind::Struct => chalk_solve::rust_ir::AdtKind::Struct,
                ty::AdtKind::Union => chalk_solve::rust_ir::AdtKind::Union,
                ty::AdtKind::Enum => chalk_solve::rust_ir::AdtKind::Enum,
            },
        })
    }

    fn adt_repr(
        &self,
        adt_id: chalk_ir::AdtId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::AdtRepr<RustInterner<'tcx>>> {
        let adt_def = adt_id.0;
        let int = |i| chalk_ir::TyKind::Scalar(chalk_ir::Scalar::Int(i)).intern(self.interner);
        let uint = |i| chalk_ir::TyKind::Scalar(chalk_ir::Scalar::Uint(i)).intern(self.interner);
        Arc::new(chalk_solve::rust_ir::AdtRepr {
            c: adt_def.repr().c(),
            packed: adt_def.repr().packed(),
            int: adt_def.repr().int.map(|i| match i {
                attr::IntType::SignedInt(ty) => match ty {
                    ast::IntTy::Isize => int(chalk_ir::IntTy::Isize),
                    ast::IntTy::I8 => int(chalk_ir::IntTy::I8),
                    ast::IntTy::I16 => int(chalk_ir::IntTy::I16),
                    ast::IntTy::I32 => int(chalk_ir::IntTy::I32),
                    ast::IntTy::I64 => int(chalk_ir::IntTy::I64),
                    ast::IntTy::I128 => int(chalk_ir::IntTy::I128),
                },
                attr::IntType::UnsignedInt(ty) => match ty {
                    ast::UintTy::Usize => uint(chalk_ir::UintTy::Usize),
                    ast::UintTy::U8 => uint(chalk_ir::UintTy::U8),
                    ast::UintTy::U16 => uint(chalk_ir::UintTy::U16),
                    ast::UintTy::U32 => uint(chalk_ir::UintTy::U32),
                    ast::UintTy::U64 => uint(chalk_ir::UintTy::U64),
                    ast::UintTy::U128 => uint(chalk_ir::UintTy::U128),
                },
            }),
        })
    }

    fn adt_size_align(
        &self,
        adt_id: chalk_ir::AdtId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::AdtSizeAlign> {
        let tcx = self.interner.tcx;
        let did = adt_id.0.did();

        // Grab the ADT and the param we might need to calculate its layout
        let param_env = tcx.param_env(did);
        let adt_ty = tcx.type_of(did);

        // The ADT is a 1-zst if it's a ZST and its alignment is 1.
        // Mark the ADT as _not_ a 1-zst if there was a layout error.
        let one_zst = if let Ok(layout) = tcx.layout_of(param_env.and(adt_ty)) {
            layout.is_zst() && layout.align.abi.bytes() == 1
        } else {
            false
        };

        Arc::new(chalk_solve::rust_ir::AdtSizeAlign::from_one_zst(one_zst))
    }

    fn fn_def_datum(
        &self,
        fn_def_id: chalk_ir::FnDefId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::FnDefDatum<RustInterner<'tcx>>> {
        let def_id = fn_def_id.0;
        let bound_vars = bound_vars_for_item(self.interner.tcx, def_id);
        let binders = binders_for(self.interner, bound_vars);

        let where_clauses = self.where_clauses_for(def_id, bound_vars);

        let sig = self.interner.tcx.fn_sig(def_id);
        let (inputs_and_output, iobinders, _) = crate::chalk::lowering::collect_bound_vars(
            self.interner,
            self.interner.tcx,
            EarlyBinder(sig.inputs_and_output()).subst(self.interner.tcx, bound_vars),
        );

        let argument_types = inputs_and_output[..inputs_and_output.len() - 1]
            .iter()
            .map(|t| {
                EarlyBinder(*t).subst(self.interner.tcx, &bound_vars).lower_into(self.interner)
            })
            .collect();

        let return_type = EarlyBinder(inputs_and_output[inputs_and_output.len() - 1])
            .subst(self.interner.tcx, &bound_vars)
            .lower_into(self.interner);

        let bound = chalk_solve::rust_ir::FnDefDatumBound {
            inputs_and_output: chalk_ir::Binders::new(
                iobinders,
                chalk_solve::rust_ir::FnDefInputsAndOutputDatum { argument_types, return_type },
            ),
            where_clauses,
        };
        Arc::new(chalk_solve::rust_ir::FnDefDatum {
            id: fn_def_id,
            sig: sig.lower_into(self.interner),
            binders: chalk_ir::Binders::new(binders, bound),
        })
    }

    fn impl_datum(
        &self,
        impl_id: chalk_ir::ImplId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::ImplDatum<RustInterner<'tcx>>> {
        let def_id = impl_id.0;
        let bound_vars = bound_vars_for_item(self.interner.tcx, def_id);
        let binders = binders_for(self.interner, bound_vars);

        let trait_ref = self.interner.tcx.bound_impl_trait_ref(def_id).expect("not an impl");
        let trait_ref = trait_ref.subst(self.interner.tcx, bound_vars);

        let where_clauses = self.where_clauses_for(def_id, bound_vars);

        let value = chalk_solve::rust_ir::ImplDatumBound {
            trait_ref: trait_ref.lower_into(self.interner),
            where_clauses,
        };

        let associated_ty_value_ids: Vec<_> = self
            .interner
            .tcx
            .associated_items(def_id)
            .in_definition_order()
            .filter(|i| i.kind == AssocKind::Type)
            .map(|i| chalk_solve::rust_ir::AssociatedTyValueId(i.def_id))
            .collect();

        Arc::new(chalk_solve::rust_ir::ImplDatum {
            polarity: self.interner.tcx.impl_polarity(def_id).lower_into(self.interner),
            binders: chalk_ir::Binders::new(binders, value),
            impl_type: chalk_solve::rust_ir::ImplType::Local,
            associated_ty_value_ids,
        })
    }

    fn impls_for_trait(
        &self,
        trait_id: chalk_ir::TraitId<RustInterner<'tcx>>,
        parameters: &[chalk_ir::GenericArg<RustInterner<'tcx>>],
        _binders: &chalk_ir::CanonicalVarKinds<RustInterner<'tcx>>,
    ) -> Vec<chalk_ir::ImplId<RustInterner<'tcx>>> {
        let def_id = trait_id.0;

        // FIXME(chalk): use TraitDef::for_each_relevant_impl, but that will
        // require us to be able to interconvert `Ty<'tcx>`, and we're
        // not there yet.

        let all_impls = self.interner.tcx.all_impls(def_id);
        let matched_impls = all_impls.filter(|impl_def_id| {
            use chalk_ir::could_match::CouldMatch;
            let trait_ref = self.interner.tcx.bound_impl_trait_ref(*impl_def_id).unwrap();
            let bound_vars = bound_vars_for_item(self.interner.tcx, *impl_def_id);

            let self_ty = trait_ref.map_bound(|t| t.self_ty());
            let self_ty = self_ty.subst(self.interner.tcx, bound_vars);
            let lowered_ty = self_ty.lower_into(self.interner);

            parameters[0].assert_ty_ref(self.interner).could_match(
                self.interner,
                self.unification_database(),
                &lowered_ty,
            )
        });

        let impls = matched_impls.map(chalk_ir::ImplId).collect();
        impls
    }

    fn impl_provided_for(
        &self,
        auto_trait_id: chalk_ir::TraitId<RustInterner<'tcx>>,
        chalk_ty: &chalk_ir::TyKind<RustInterner<'tcx>>,
    ) -> bool {
        use chalk_ir::Scalar::*;
        use chalk_ir::TyKind::*;

        let trait_def_id = auto_trait_id.0;
        let all_impls = self.interner.tcx.all_impls(trait_def_id);
        for impl_def_id in all_impls {
            let trait_ref = self.interner.tcx.impl_trait_ref(impl_def_id).unwrap();
            let self_ty = trait_ref.self_ty();
            let provides = match (self_ty.kind(), chalk_ty) {
                (&ty::Adt(impl_adt_def, ..), Adt(id, ..)) => impl_adt_def.did() == id.0.did(),
                (_, AssociatedType(_ty_id, ..)) => {
                    // FIXME(chalk): See https://github.com/rust-lang/rust/pull/77152#discussion_r494484774
                    false
                }
                (ty::Bool, Scalar(Bool)) => true,
                (ty::Char, Scalar(Char)) => true,
                (ty::Int(ty1), Scalar(Int(ty2))) => matches!(
                    (ty1, ty2),
                    (ty::IntTy::Isize, chalk_ir::IntTy::Isize)
                        | (ty::IntTy::I8, chalk_ir::IntTy::I8)
                        | (ty::IntTy::I16, chalk_ir::IntTy::I16)
                        | (ty::IntTy::I32, chalk_ir::IntTy::I32)
                        | (ty::IntTy::I64, chalk_ir::IntTy::I64)
                        | (ty::IntTy::I128, chalk_ir::IntTy::I128)
                ),
                (ty::Uint(ty1), Scalar(Uint(ty2))) => matches!(
                    (ty1, ty2),
                    (ty::UintTy::Usize, chalk_ir::UintTy::Usize)
                        | (ty::UintTy::U8, chalk_ir::UintTy::U8)
                        | (ty::UintTy::U16, chalk_ir::UintTy::U16)
                        | (ty::UintTy::U32, chalk_ir::UintTy::U32)
                        | (ty::UintTy::U64, chalk_ir::UintTy::U64)
                        | (ty::UintTy::U128, chalk_ir::UintTy::U128)
                ),
                (ty::Float(ty1), Scalar(Float(ty2))) => matches!(
                    (ty1, ty2),
                    (ty::FloatTy::F32, chalk_ir::FloatTy::F32)
                        | (ty::FloatTy::F64, chalk_ir::FloatTy::F64)
                ),
                (&ty::Tuple(substs), Tuple(len, _)) => substs.len() == *len,
                (&ty::Array(..), Array(..)) => true,
                (&ty::Slice(..), Slice(..)) => true,
                (&ty::RawPtr(type_and_mut), Raw(mutability, _)) => {
                    match (type_and_mut.mutbl, mutability) {
                        (ast::Mutability::Mut, chalk_ir::Mutability::Mut) => true,
                        (ast::Mutability::Mut, chalk_ir::Mutability::Not) => false,
                        (ast::Mutability::Not, chalk_ir::Mutability::Mut) => false,
                        (ast::Mutability::Not, chalk_ir::Mutability::Not) => true,
                    }
                }
                (&ty::Ref(.., mutability1), Ref(mutability2, ..)) => {
                    match (mutability1, mutability2) {
                        (ast::Mutability::Mut, chalk_ir::Mutability::Mut) => true,
                        (ast::Mutability::Mut, chalk_ir::Mutability::Not) => false,
                        (ast::Mutability::Not, chalk_ir::Mutability::Mut) => false,
                        (ast::Mutability::Not, chalk_ir::Mutability::Not) => true,
                    }
                }
                (&ty::Opaque(def_id, ..), OpaqueType(opaque_ty_id, ..)) => def_id == opaque_ty_id.0,
                (&ty::FnDef(def_id, ..), FnDef(fn_def_id, ..)) => def_id == fn_def_id.0,
                (&ty::Str, Str) => true,
                (&ty::Never, Never) => true,
                (&ty::Closure(def_id, ..), Closure(closure_id, _)) => def_id == closure_id.0,
                (&ty::Foreign(def_id), Foreign(foreign_def_id)) => def_id == foreign_def_id.0,
                (&ty::Error(..), Error) => false,
                _ => false,
            };
            if provides {
                return true;
            }
        }
        false
    }

    fn associated_ty_value(
        &self,
        associated_ty_id: chalk_solve::rust_ir::AssociatedTyValueId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::AssociatedTyValue<RustInterner<'tcx>>> {
        let def_id = associated_ty_id.0;
        let assoc_item = self.interner.tcx.associated_item(def_id);
        let impl_id = assoc_item.container_id(self.interner.tcx);
        match assoc_item.kind {
            AssocKind::Type => {}
            _ => unimplemented!("Not possible??"),
        }

        let trait_item_id = assoc_item.trait_item_def_id.expect("assoc_ty with no trait version");
        let bound_vars = bound_vars_for_item(self.interner.tcx, def_id);
        let binders = binders_for(self.interner, bound_vars);
        let ty = self
            .interner
            .tcx
            .bound_type_of(def_id)
            .subst(self.interner.tcx, bound_vars)
            .lower_into(self.interner);

        Arc::new(chalk_solve::rust_ir::AssociatedTyValue {
            impl_id: chalk_ir::ImplId(impl_id),
            associated_ty_id: chalk_ir::AssocTypeId(trait_item_id),
            value: chalk_ir::Binders::new(
                binders,
                chalk_solve::rust_ir::AssociatedTyValueBound { ty },
            ),
        })
    }

    fn custom_clauses(&self) -> Vec<chalk_ir::ProgramClause<RustInterner<'tcx>>> {
        vec![]
    }

    fn local_impls_to_coherence_check(
        &self,
        _trait_id: chalk_ir::TraitId<RustInterner<'tcx>>,
    ) -> Vec<chalk_ir::ImplId<RustInterner<'tcx>>> {
        unimplemented!()
    }

    fn opaque_ty_data(
        &self,
        opaque_ty_id: chalk_ir::OpaqueTyId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::OpaqueTyDatum<RustInterner<'tcx>>> {
        let bound_vars = ty::fold::shift_vars(
            self.interner.tcx,
            bound_vars_for_item(self.interner.tcx, opaque_ty_id.0),
            1,
        );
        let where_clauses = self.where_clauses_for(opaque_ty_id.0, bound_vars);

        let identity_substs = InternalSubsts::identity_for_item(self.interner.tcx, opaque_ty_id.0);

        let bounds =
            self.interner
                .tcx
                .explicit_item_bounds(opaque_ty_id.0)
                .iter()
                .map(|(bound, _)| EarlyBinder(*bound).subst(self.interner.tcx, &bound_vars))
                .map(|bound| {
                    bound.fold_with(&mut ReplaceOpaqueTyFolder {
                        tcx: self.interner.tcx,
                        opaque_ty_id,
                        identity_substs,
                        binder_index: ty::INNERMOST,
                    })
                })
                .filter_map(|bound| {
                    LowerInto::<
                    Option<chalk_ir::QuantifiedWhereClause<RustInterner<'tcx>>>
                >::lower_into(bound, self.interner)
                })
                .collect();

        // Binder for the bound variable representing the concrete impl Trait type.
        let existential_binder = chalk_ir::VariableKinds::from1(
            self.interner,
            chalk_ir::VariableKind::Ty(chalk_ir::TyVariableKind::General),
        );

        let value = chalk_solve::rust_ir::OpaqueTyDatumBound {
            bounds: chalk_ir::Binders::new(existential_binder.clone(), bounds),
            where_clauses: chalk_ir::Binders::new(existential_binder, where_clauses),
        };

        let binders = binders_for(self.interner, bound_vars);
        Arc::new(chalk_solve::rust_ir::OpaqueTyDatum {
            opaque_ty_id,
            bound: chalk_ir::Binders::new(binders, value),
        })
    }

    fn program_clauses_for_env(
        &self,
        environment: &chalk_ir::Environment<RustInterner<'tcx>>,
    ) -> chalk_ir::ProgramClauses<RustInterner<'tcx>> {
        chalk_solve::program_clauses_for_env(self, environment)
    }

    fn well_known_trait_id(
        &self,
        well_known_trait: chalk_solve::rust_ir::WellKnownTrait,
    ) -> Option<chalk_ir::TraitId<RustInterner<'tcx>>> {
        use chalk_solve::rust_ir::WellKnownTrait::*;
        let lang_items = self.interner.tcx.lang_items();
        let def_id = match well_known_trait {
            Sized => lang_items.sized_trait(),
            Copy => lang_items.copy_trait(),
            Clone => lang_items.clone_trait(),
            Drop => lang_items.drop_trait(),
            Fn => lang_items.fn_trait(),
            FnMut => lang_items.fn_mut_trait(),
            FnOnce => lang_items.fn_once_trait(),
            Generator => lang_items.gen_trait(),
            Unsize => lang_items.unsize_trait(),
            Unpin => lang_items.unpin_trait(),
            CoerceUnsized => lang_items.coerce_unsized_trait(),
            DiscriminantKind => lang_items.discriminant_kind_trait(),
            DispatchFromDyn => lang_items.dispatch_from_dyn_trait(),
        };
        def_id.map(chalk_ir::TraitId)
    }

    fn is_object_safe(&self, trait_id: chalk_ir::TraitId<RustInterner<'tcx>>) -> bool {
        self.interner.tcx.is_object_safe(trait_id.0)
    }

    fn hidden_opaque_type(
        &self,
        _id: chalk_ir::OpaqueTyId<RustInterner<'tcx>>,
    ) -> chalk_ir::Ty<RustInterner<'tcx>> {
        // FIXME(chalk): actually get hidden ty
        self.interner
            .tcx
            .mk_ty(ty::Tuple(self.interner.tcx.intern_type_list(&[])))
            .lower_into(self.interner)
    }

    fn closure_kind(
        &self,
        _closure_id: chalk_ir::ClosureId<RustInterner<'tcx>>,
        substs: &chalk_ir::Substitution<RustInterner<'tcx>>,
    ) -> chalk_solve::rust_ir::ClosureKind {
        let kind = &substs.as_slice(self.interner)[substs.len(self.interner) - 3];
        match kind.assert_ty_ref(self.interner).kind(self.interner) {
            chalk_ir::TyKind::Scalar(chalk_ir::Scalar::Int(int_ty)) => match int_ty {
                chalk_ir::IntTy::I8 => chalk_solve::rust_ir::ClosureKind::Fn,
                chalk_ir::IntTy::I16 => chalk_solve::rust_ir::ClosureKind::FnMut,
                chalk_ir::IntTy::I32 => chalk_solve::rust_ir::ClosureKind::FnOnce,
                _ => bug!("bad closure kind"),
            },
            _ => bug!("bad closure kind"),
        }
    }

    fn closure_inputs_and_output(
        &self,
        _closure_id: chalk_ir::ClosureId<RustInterner<'tcx>>,
        substs: &chalk_ir::Substitution<RustInterner<'tcx>>,
    ) -> chalk_ir::Binders<chalk_solve::rust_ir::FnDefInputsAndOutputDatum<RustInterner<'tcx>>>
    {
        let sig = &substs.as_slice(self.interner)[substs.len(self.interner) - 2];
        match sig.assert_ty_ref(self.interner).kind(self.interner) {
            chalk_ir::TyKind::Function(f) => {
                let substitution = f.substitution.0.as_slice(self.interner);
                let return_type = substitution.last().unwrap().assert_ty_ref(self.interner).clone();
                // Closure arguments are tupled
                let argument_tuple = substitution[0].assert_ty_ref(self.interner);
                let argument_types = match argument_tuple.kind(self.interner) {
                    chalk_ir::TyKind::Tuple(_len, substitution) => substitution
                        .iter(self.interner)
                        .map(|arg| arg.assert_ty_ref(self.interner))
                        .cloned()
                        .collect(),
                    _ => bug!("Expecting closure FnSig args to be tupled."),
                };

                chalk_ir::Binders::new(
                    chalk_ir::VariableKinds::from_iter(
                        self.interner,
                        (0..f.num_binders).map(|_| chalk_ir::VariableKind::Lifetime),
                    ),
                    chalk_solve::rust_ir::FnDefInputsAndOutputDatum { argument_types, return_type },
                )
            }
            _ => panic!("Invalid sig."),
        }
    }

    fn closure_upvars(
        &self,
        _closure_id: chalk_ir::ClosureId<RustInterner<'tcx>>,
        substs: &chalk_ir::Substitution<RustInterner<'tcx>>,
    ) -> chalk_ir::Binders<chalk_ir::Ty<RustInterner<'tcx>>> {
        let inputs_and_output = self.closure_inputs_and_output(_closure_id, substs);
        let tuple = substs.as_slice(self.interner).last().unwrap().assert_ty_ref(self.interner);
        inputs_and_output.map_ref(|_| tuple.clone())
    }

    fn closure_fn_substitution(
        &self,
        _closure_id: chalk_ir::ClosureId<RustInterner<'tcx>>,
        substs: &chalk_ir::Substitution<RustInterner<'tcx>>,
    ) -> chalk_ir::Substitution<RustInterner<'tcx>> {
        let substitution = &substs.as_slice(self.interner)[0..substs.len(self.interner) - 3];
        chalk_ir::Substitution::from_iter(self.interner, substitution)
    }

    fn generator_datum(
        &self,
        _generator_id: chalk_ir::GeneratorId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::GeneratorDatum<RustInterner<'tcx>>> {
        unimplemented!()
    }

    fn generator_witness_datum(
        &self,
        _generator_id: chalk_ir::GeneratorId<RustInterner<'tcx>>,
    ) -> Arc<chalk_solve::rust_ir::GeneratorWitnessDatum<RustInterner<'tcx>>> {
        unimplemented!()
    }

    fn unification_database(&self) -> &dyn chalk_ir::UnificationDatabase<RustInterner<'tcx>> {
        self
    }

    fn discriminant_type(
        &self,
        _: chalk_ir::Ty<RustInterner<'tcx>>,
    ) -> chalk_ir::Ty<RustInterner<'tcx>> {
        unimplemented!()
    }
}

impl<'tcx> chalk_ir::UnificationDatabase<RustInterner<'tcx>> for RustIrDatabase<'tcx> {
    fn fn_def_variance(
        &self,
        def_id: chalk_ir::FnDefId<RustInterner<'tcx>>,
    ) -> chalk_ir::Variances<RustInterner<'tcx>> {
        let variances = self.interner.tcx.variances_of(def_id.0);
        chalk_ir::Variances::from_iter(
            self.interner,
            variances.iter().map(|v| v.lower_into(self.interner)),
        )
    }

    fn adt_variance(
        &self,
        adt_id: chalk_ir::AdtId<RustInterner<'tcx>>,
    ) -> chalk_ir::Variances<RustInterner<'tcx>> {
        let variances = self.interner.tcx.variances_of(adt_id.0.did());
        chalk_ir::Variances::from_iter(
            self.interner,
            variances.iter().map(|v| v.lower_into(self.interner)),
        )
    }
}

/// Creates an `InternalSubsts` that maps each generic parameter to a higher-ranked
/// var bound at index `0`. For types, we use a `BoundVar` index equal to
/// the type parameter index. For regions, we use the `BoundRegionKind::BrNamed`
/// variant (which has a `DefId`).
fn bound_vars_for_item<'tcx>(tcx: TyCtxt<'tcx>, def_id: DefId) -> SubstsRef<'tcx> {
    InternalSubsts::for_item(tcx, def_id, |param, substs| match param.kind {
        ty::GenericParamDefKind::Type { .. } => tcx
            .mk_ty(ty::Bound(
                ty::INNERMOST,
                ty::BoundTy {
                    var: ty::BoundVar::from(param.index),
                    kind: ty::BoundTyKind::Param(param.name),
                },
            ))
            .into(),

        ty::GenericParamDefKind::Lifetime => {
            let br = ty::BoundRegion {
                var: ty::BoundVar::from_usize(substs.len()),
                kind: ty::BrAnon(substs.len() as u32),
            };
            tcx.mk_region(ty::ReLateBound(ty::INNERMOST, br)).into()
        }

        ty::GenericParamDefKind::Const { .. } => tcx
            .mk_const(ty::ConstS {
                kind: ty::ConstKind::Bound(ty::INNERMOST, ty::BoundVar::from(param.index)),
                ty: tcx.type_of(param.def_id),
            })
            .into(),
    })
}

fn binders_for<'tcx>(
    interner: RustInterner<'tcx>,
    bound_vars: SubstsRef<'tcx>,
) -> chalk_ir::VariableKinds<RustInterner<'tcx>> {
    chalk_ir::VariableKinds::from_iter(
        interner,
        bound_vars.iter().map(|arg| match arg.unpack() {
            ty::subst::GenericArgKind::Lifetime(_re) => chalk_ir::VariableKind::Lifetime,
            ty::subst::GenericArgKind::Type(_ty) => {
                chalk_ir::VariableKind::Ty(chalk_ir::TyVariableKind::General)
            }
            ty::subst::GenericArgKind::Const(c) => {
                chalk_ir::VariableKind::Const(c.ty().lower_into(interner))
            }
        }),
    )
}

struct ReplaceOpaqueTyFolder<'tcx> {
    tcx: TyCtxt<'tcx>,
    opaque_ty_id: chalk_ir::OpaqueTyId<RustInterner<'tcx>>,
    identity_substs: SubstsRef<'tcx>,
    binder_index: ty::DebruijnIndex,
}

impl<'tcx> ty::TypeFolder<'tcx> for ReplaceOpaqueTyFolder<'tcx> {
    fn tcx<'b>(&'b self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn fold_binder<T: TypeFoldable<'tcx>>(
        &mut self,
        t: ty::Binder<'tcx, T>,
    ) -> ty::Binder<'tcx, T> {
        self.binder_index.shift_in(1);
        let t = t.super_fold_with(self);
        self.binder_index.shift_out(1);
        t
    }

    fn fold_ty(&mut self, ty: Ty<'tcx>) -> Ty<'tcx> {
        if let ty::Opaque(def_id, substs) = *ty.kind() {
            if def_id == self.opaque_ty_id.0 && substs == self.identity_substs {
                return self.tcx.mk_ty(ty::Bound(
                    self.binder_index,
                    ty::BoundTy::from(ty::BoundVar::from_u32(0)),
                ));
            }
        }
        ty
    }
}

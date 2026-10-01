use std::collections::BTreeSet;

use crate::{
    application::{CompositeSync, HistoryAtom, LayerPropertyEdit},
    core::{
        document::Document,
        material::MaterialIndex,
        surface::{LayerId, LayerMaterialMask},
    },
};

/// Renderer-independent meaning of a document mutation that can affect composites.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompositeImpact {
    None,
    LayerTreeStructure,
    MaterialTreesScoped(BTreeSet<MaterialIndex>),
    LayerProps(LayerId),
    Adjustment(LayerId),
    MaterialMask {
        before: LayerMaterialMask,
        after: LayerMaterialMask,
    },
    Multiple(Vec<CompositeImpact>),
}

impl CompositeImpact {
    pub(crate) fn for_history_atom(document: Option<&Document>, atom: &HistoryAtom) -> Self {
        match atom {
            HistoryAtom::LayerTreeEdit { .. } => Self::LayerTreeStructure,
            HistoryAtom::LayerPropertyEdit { edit } => {
                Self::for_layer_property_edit(document, edit)
            }
            HistoryAtom::PixelEdit { .. } | HistoryAtom::SelectionEdit { .. } => Self::None,
            HistoryAtom::MaterialTextureResize { after, .. } => {
                Self::MaterialTreesScoped(BTreeSet::from([after.material_index]))
            }
        }
    }

    pub(crate) fn for_layer_property_edit(
        document: Option<&Document>,
        edit: &LayerPropertyEdit,
    ) -> Self {
        match edit {
            LayerPropertyEdit::Visibility { layer_id, .. }
            | LayerPropertyEdit::Opacity { layer_id, .. } => Self::LayerProps(*layer_id),
            LayerPropertyEdit::Adjustment { layer_id, .. } => Self::Adjustment(*layer_id),
            LayerPropertyEdit::MaterialMask { before, after, .. } => Self::MaterialMask {
                before: before.clone(),
                after: after.clone(),
            },
            LayerPropertyEdit::FillColor { .. }
            | LayerPropertyEdit::EmbeddedImageTransform { .. }
            | LayerPropertyEdit::OpacityMany { .. }
            | LayerPropertyEdit::CompositeModeMany { .. } => Self::LayerTreeStructure,
            LayerPropertyEdit::CompositeMode { layer_id, .. } => {
                match document.map(|document| document.layer_tree.is_group(*layer_id)) {
                    Some(false) => Self::LayerProps(*layer_id),
                    Some(true) | None => Self::LayerTreeStructure,
                }
            }
            LayerPropertyEdit::Rename { .. }
            | LayerPropertyEdit::Lock { .. }
            | LayerPropertyEdit::LockMany { .. } => Self::None,
        }
    }

    pub(crate) fn merge(self, incoming: Self) -> Self {
        match (self, incoming) {
            (Self::None, incoming) => incoming,
            (existing, Self::None) => existing,
            (Self::Multiple(mut existing), Self::Multiple(incoming)) => {
                existing.extend(incoming);
                Self::Multiple(existing)
            }
            (Self::Multiple(mut existing), incoming) => {
                existing.push(incoming);
                Self::Multiple(existing)
            }
            (existing, Self::Multiple(mut incoming)) => {
                incoming.insert(0, existing);
                Self::Multiple(incoming)
            }
            (existing, incoming) => Self::Multiple(vec![existing, incoming]),
        }
    }

    pub(crate) fn into_composite_sync(self, document: Option<&Document>) -> CompositeSync {
        match self {
            Self::None => CompositeSync::None,
            Self::LayerTreeStructure => CompositeSync::MaterialTrees,
            Self::MaterialTreesScoped(material_indices) => {
                CompositeSync::MaterialTreesScoped(material_indices)
            }
            Self::LayerProps(layer_id) => CompositeSync::layer_props(layer_id),
            Self::Adjustment(layer_id) => CompositeSync::adjustment(layer_id),
            Self::MaterialMask { before, after } => {
                material_mask_composite_sync(document, &before, &after)
            }
            Self::Multiple(impacts) => impacts
                .into_iter()
                .fold(CompositeSync::None, |sync, impact| {
                    sync.merge(impact.into_composite_sync(document))
                }),
        }
    }
}

fn material_mask_composite_sync(
    document: Option<&Document>,
    before: &LayerMaterialMask,
    after: &LayerMaterialMask,
) -> CompositeSync {
    let Some(document) = document else {
        return CompositeSync::MaterialTrees;
    };
    let (LayerMaterialMask::Specified(before_ids), LayerMaterialMask::Specified(after_ids)) =
        (before, after)
    else {
        return CompositeSync::MaterialTrees;
    };
    let affected_ids = before_ids
        .union(after_ids)
        .copied()
        .collect::<BTreeSet<_>>();
    CompositeSync::material_trees_scoped(document.materials.iter().enumerate().filter_map(
        |(material_index, material)| {
            affected_ids
                .contains(&material.id)
                .then_some(MaterialIndex(material_index))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::LayerCompositeSettings,
        core::{
            adjustment::AdjustmentKind,
            composite::{GroupCompositeMode, LayerBlendMode},
            document::{MaterialSpec, MeshData},
            material::MaterialIndex,
        },
    };

    fn document_with_group() -> (Document, LayerId, LayerId) {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
                MaterialSpec::new("C", [8, 8]),
            ],
        );
        let raster = document.layer_tree.default_raster_layer().unwrap();
        let group = document
            .layer_tree
            .add_group_above(raster, "Group")
            .unwrap();
        (document, raster, group)
    }

    fn settings() -> LayerCompositeSettings {
        LayerCompositeSettings {
            blend_mode: LayerBlendMode::Normal,
            group_mode: GroupCompositeMode::Isolated,
        }
    }

    #[test]
    fn layer_property_kinds_have_one_invalidation_table() {
        let (document, raster, group) = document_with_group();
        let adjustment = AdjustmentKind::BrightnessContrast.default_adjustment();

        let cases = [
            (
                LayerPropertyEdit::Opacity {
                    layer_id: raster,
                    before: 1.0,
                    after: 0.5,
                },
                CompositeSync::layer_props(raster),
            ),
            (
                LayerPropertyEdit::Visibility {
                    layer_id: raster,
                    before: true,
                    after: false,
                },
                CompositeSync::layer_props(raster),
            ),
            (
                LayerPropertyEdit::Adjustment {
                    layer_id: raster,
                    edit_session: 1,
                    before: adjustment.clone(),
                    after: adjustment,
                },
                CompositeSync::adjustment(raster),
            ),
            (
                LayerPropertyEdit::CompositeMode {
                    layer_id: raster,
                    before: settings(),
                    after: settings(),
                },
                CompositeSync::layer_props(raster),
            ),
            (
                LayerPropertyEdit::CompositeMode {
                    layer_id: group,
                    before: settings(),
                    after: settings(),
                },
                CompositeSync::MaterialTrees,
            ),
            (
                LayerPropertyEdit::Rename {
                    layer_id: raster,
                    before: "Before".to_owned(),
                    after: "After".to_owned(),
                },
                CompositeSync::None,
            ),
            (
                LayerPropertyEdit::Lock {
                    layer_id: raster,
                    before: false,
                    after: true,
                },
                CompositeSync::None,
            ),
        ];

        for (edit, expected) in cases {
            let actual = CompositeImpact::for_layer_property_edit(Some(&document), &edit)
                .into_composite_sync(Some(&document));
            assert_eq!(actual, expected, "unexpected impact for {edit:?}");
        }
    }

    #[test]
    fn material_mask_uses_scoped_union_or_full_tree_sync() {
        let (document, raster, _) = document_with_group();
        let ids = document
            .materials
            .iter()
            .map(|material| material.id)
            .collect::<Vec<_>>();
        let specified = |ids: &[crate::core::material::MaterialId]| {
            LayerMaterialMask::Specified(ids.iter().copied().collect())
        };
        let edit = LayerPropertyEdit::MaterialMask {
            layer_id: raster,
            before: specified(&ids[..2]),
            after: specified(&ids[1..]),
        };
        assert_eq!(
            CompositeImpact::for_layer_property_edit(Some(&document), &edit)
                .into_composite_sync(Some(&document)),
            CompositeSync::material_trees_scoped([
                MaterialIndex(0),
                MaterialIndex(1),
                MaterialIndex(2),
            ])
        );

        let full = LayerPropertyEdit::MaterialMask {
            layer_id: raster,
            before: LayerMaterialMask::Unspecified,
            after: specified(&ids[..1]),
        };
        assert_eq!(
            CompositeImpact::for_layer_property_edit(Some(&document), &full)
                .into_composite_sync(Some(&document)),
            CompositeSync::MaterialTrees
        );
    }

    #[test]
    fn structural_and_multiple_impacts_merge_through_the_same_policy() {
        let (document, raster, group) = document_with_group();
        assert_eq!(
            CompositeImpact::LayerTreeStructure.into_composite_sync(Some(&document)),
            CompositeSync::MaterialTrees
        );
        assert_eq!(
            CompositeImpact::LayerProps(raster)
                .merge(CompositeImpact::LayerProps(group))
                .into_composite_sync(Some(&document)),
            CompositeSync::LayerProps(BTreeSet::from([raster, group]))
        );
        assert_eq!(
            CompositeImpact::LayerProps(raster)
                .merge(CompositeImpact::Adjustment(group))
                .into_composite_sync(Some(&document)),
            CompositeSync::MaterialTrees
        );
    }
}

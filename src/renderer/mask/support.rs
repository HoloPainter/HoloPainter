use std::{error::Error, fmt};

use crate::core::mask::{GeometryMaskSource, MaskSource, ProjectionMaskSource, ShapeMaskSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaskConsumer {
    Paint,
    Selection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaskKind {
    Brush,
    Rectangle,
    Polygon,
    Full,
    MeshGeometry,
    ViewportRectangle,
    ViewportPolygon,
    Flood,
    ExistingSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOperationKind {
    SolidColorPaint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnsupportedMaskError {
    pub(crate) consumer: MaskConsumer,
    pub(crate) mask_kind: MaskKind,
    pub(crate) operation_kind: Option<ApplyOperationKind>,
}

impl UnsupportedMaskError {
    pub(crate) fn new(
        consumer: MaskConsumer,
        mask: &MaskSource,
        operation_kind: Option<ApplyOperationKind>,
    ) -> Self {
        Self {
            consumer,
            mask_kind: mask_kind(mask),
            operation_kind,
        }
    }
}

impl fmt::Display for UnsupportedMaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported {:?} mask {:?}",
            self.consumer, self.mask_kind
        )?;
        if let Some(operation) = self.operation_kind {
            write!(f, " for {operation:?}")?;
        }
        Ok(())
    }
}

impl Error for UnsupportedMaskError {}

pub(crate) fn ensure_supported(
    consumer: MaskConsumer,
    mask: &MaskSource,
    operation_kind: Option<ApplyOperationKind>,
) -> Result<(), UnsupportedMaskError> {
    let supported = match (consumer, mask) {
        (MaskConsumer::Paint, MaskSource::Brush(_)) => false,
        (MaskConsumer::Paint, MaskSource::Shape(ShapeMaskSource::Rectangle(_))) => true,
        (MaskConsumer::Paint, MaskSource::Shape(ShapeMaskSource::Polygon(_))) => true,
        (MaskConsumer::Paint, MaskSource::Full(_)) => true,
        (MaskConsumer::Paint, MaskSource::Geometry(GeometryMaskSource::Mesh(_))) => true,
        (MaskConsumer::Paint, MaskSource::Projection(ProjectionMaskSource::ViewportRect(_))) => {
            true
        }
        (MaskConsumer::Paint, MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(_))) => {
            true
        }
        (MaskConsumer::Paint, MaskSource::Flood(_)) => false,
        (MaskConsumer::Paint, MaskSource::ExistingSelection) => false,
        (MaskConsumer::Selection, MaskSource::Brush(_)) => false,
        (MaskConsumer::Selection, MaskSource::Shape(ShapeMaskSource::Rectangle(_))) => true,
        (MaskConsumer::Selection, MaskSource::Shape(ShapeMaskSource::Polygon(_))) => true,
        (MaskConsumer::Selection, MaskSource::Full(_)) => true,
        (MaskConsumer::Selection, MaskSource::Geometry(GeometryMaskSource::Mesh(_))) => false,
        (
            MaskConsumer::Selection,
            MaskSource::Projection(ProjectionMaskSource::ViewportRect(_)),
        ) => true,
        (
            MaskConsumer::Selection,
            MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(_)),
        ) => true,
        (MaskConsumer::Selection, MaskSource::Flood(_)) => false,
        (MaskConsumer::Selection, MaskSource::ExistingSelection) => false,
    };
    if supported {
        Ok(())
    } else {
        Err(UnsupportedMaskError::new(consumer, mask, operation_kind))
    }
}

fn mask_kind(mask: &MaskSource) -> MaskKind {
    match mask {
        MaskSource::Brush(_) => MaskKind::Brush,
        MaskSource::Shape(ShapeMaskSource::Rectangle(_)) => MaskKind::Rectangle,
        MaskSource::Shape(ShapeMaskSource::Polygon(_)) => MaskKind::Polygon,
        MaskSource::Full(_) => MaskKind::Full,
        MaskSource::Geometry(GeometryMaskSource::Mesh(_)) => MaskKind::MeshGeometry,
        MaskSource::Projection(ProjectionMaskSource::ViewportRect(_)) => {
            MaskKind::ViewportRectangle
        }
        MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(_)) => {
            MaskKind::ViewportPolygon
        }
        MaskSource::Flood(_) => MaskKind::Flood,
        MaskSource::ExistingSelection => MaskKind::ExistingSelection,
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2};

    use super::*;
    use crate::core::{
        document::MeshId,
        mask::{
            BrushMaskParams, BrushMaskSource, FloodMaskSource, FullMaskSource,
            MeshGeometryMaskSource, PolygonMaskSource, RectangleMaskSource,
            ViewportPolygonProjectionMaskSource, ViewportProjection,
            ViewportRectProjectionMaskSource,
        },
        stroke::SurfaceProjectionId,
    };

    fn capability_cases() -> Vec<(MaskSource, bool, bool)> {
        vec![
            (
                MaskSource::Brush(BrushMaskSource::uv(
                    Vec::new(),
                    BrushMaskParams {
                        radius_world: 1.0,
                        hardness: 1.0,
                        effect_base: 1.0,
                    },
                )),
                false,
                false,
            ),
            (
                MaskSource::Shape(ShapeMaskSource::Rectangle(RectangleMaskSource {
                    min_uv: Vec2::ZERO,
                    max_uv: Vec2::ONE,
                    material_index: Some(0),
                })),
                true,
                true,
            ),
            (
                MaskSource::Shape(ShapeMaskSource::Polygon(PolygonMaskSource {
                    points_uv: vec![Vec2::ZERO, Vec2::X, Vec2::Y],
                    material_index: Some(0),
                })),
                true,
                true,
            ),
            (
                MaskSource::Full(FullMaskSource::Material { material_index: 0 }),
                true,
                true,
            ),
            (
                MaskSource::Geometry(GeometryMaskSource::Mesh(MeshGeometryMaskSource {
                    mesh_id: MeshId(0),
                    triangles: Vec::new(),
                })),
                true,
                false,
            ),
            (
                MaskSource::Projection(ProjectionMaskSource::ViewportRect(
                    ViewportRectProjectionMaskSource {
                        min_px: Vec2::ZERO,
                        max_px: Vec2::ONE,
                        viewport_size: [1, 1],
                        projections: vec![ViewportProjection {
                            id: SurfaceProjectionId::Primary,
                            view_proj: Mat4::IDENTITY,
                        }],
                        material_index: None,
                        scene_visibility: Default::default(),
                    },
                )),
                true,
                true,
            ),
            (
                MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(
                    ViewportPolygonProjectionMaskSource {
                        points_px: vec![Vec2::ZERO, Vec2::X, Vec2::Y],
                        viewport_size: [1, 1],
                        projections: vec![ViewportProjection {
                            id: SurfaceProjectionId::Primary,
                            view_proj: Mat4::IDENTITY,
                        }],
                        material_index: None,
                        scene_visibility: Default::default(),
                    },
                )),
                true,
                true,
            ),
            (
                MaskSource::Flood(FloodMaskSource::UvContiguous {
                    seed_uv: [0.5, 0.5],
                    threshold: 0.0,
                }),
                false,
                false,
            ),
            (MaskSource::ExistingSelection, false, false),
        ]
    }

    #[test]
    fn paint_and_selection_have_distinct_exhaustive_capabilities() {
        for (mask, paint_supported, selection_supported) in capability_cases() {
            assert_eq!(
                ensure_supported(
                    MaskConsumer::Paint,
                    &mask,
                    Some(ApplyOperationKind::SolidColorPaint),
                )
                .is_ok(),
                paint_supported,
                "unexpected paint capability for {mask:?}",
            );
            assert_eq!(
                ensure_supported(MaskConsumer::Selection, &mask, None).is_ok(),
                selection_supported,
                "unexpected selection capability for {mask:?}",
            );
        }
    }

    #[test]
    fn unsupported_error_preserves_consumer_mask_and_operation() {
        let mask = MaskSource::ExistingSelection;
        let error = ensure_supported(
            MaskConsumer::Paint,
            &mask,
            Some(ApplyOperationKind::SolidColorPaint),
        )
        .unwrap_err();

        assert_eq!(error.consumer, MaskConsumer::Paint);
        assert_eq!(error.mask_kind, MaskKind::ExistingSelection);
        assert_eq!(
            error.operation_kind,
            Some(ApplyOperationKind::SolidColorPaint)
        );
    }
}

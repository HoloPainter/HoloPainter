use std::collections::HashMap;

use glam::Vec2;

use crate::{
    core::{
        composite::{ApplyParams, TextureCompositeMode},
        selection::ActiveSelection,
        surface::PaintSurfaceId,
    },
    renderer::{ApplyOperation, FillCoverage, FillCoverageDelta},
};

pub(crate) struct FillApplySession {
    pub(crate) operation: ApplyOperation,
    pub(crate) params: ApplyParams<TextureCompositeMode>,
    pub(crate) active_selection: ActiveSelection,
    surfaces: HashMap<PaintSurfaceId, FillSurfaceSession>,
}

pub(crate) struct FillSurfaceSession {
    pub(crate) texture_size: [u32; 2],
    coverage: FillSurfaceCoverage,
}

enum FillSurfaceCoverage {
    Full,
    Triangles(Vec<[Vec2; 3]>),
}

impl FillApplySession {
    pub(crate) fn new(
        operation: ApplyOperation,
        params: ApplyParams<TextureCompositeMode>,
        active_selection: ActiveSelection,
    ) -> Self {
        Self {
            operation,
            params,
            active_selection,
            surfaces: HashMap::new(),
        }
    }

    pub(crate) fn extend(
        &mut self,
        delta: FillCoverageDelta,
        texture_size: [u32; 2],
    ) -> FillSurfaceUpdate {
        let surface = delta.surface;
        let first_touch = !self.surfaces.contains_key(&surface);
        let entry = self
            .surfaces
            .entry(surface)
            .or_insert_with(|| FillSurfaceSession {
                texture_size,
                coverage: FillSurfaceCoverage::Triangles(Vec::new()),
            });
        debug_assert_eq!(entry.texture_size, texture_size);
        entry.extend(delta.coverage);
        FillSurfaceUpdate {
            first_touch,
            coverage: entry.coverage(),
        }
    }

    pub(crate) fn surface_entries(
        &self,
    ) -> impl Iterator<Item = (PaintSurfaceId, &FillSurfaceSession)> {
        self.surfaces
            .iter()
            .map(|(surface, state)| (*surface, state))
    }

    pub(crate) fn surfaces(&self) -> Vec<PaintSurfaceId> {
        self.surfaces.keys().copied().collect()
    }
}

impl FillSurfaceSession {
    fn extend(&mut self, coverage: FillCoverage) {
        match coverage {
            FillCoverage::Full => self.coverage = FillSurfaceCoverage::Full,
            FillCoverage::Triangles(mut triangles) => match &mut self.coverage {
                FillSurfaceCoverage::Full => {}
                FillSurfaceCoverage::Triangles(current) => current.append(&mut triangles),
            },
        }
    }

    pub(crate) fn coverage(&self) -> FillCoverage {
        match &self.coverage {
            FillSurfaceCoverage::Full => FillCoverage::Full,
            FillSurfaceCoverage::Triangles(triangles) => FillCoverage::Triangles(triangles.clone()),
        }
    }

    pub(crate) fn needs_geometry_dilation(&self) -> bool {
        matches!(self.coverage, FillSurfaceCoverage::Triangles(_))
    }
}

pub(crate) struct FillSurfaceUpdate {
    pub(crate) first_touch: bool,
    pub(crate) coverage: FillCoverage,
}

use crate::core::surface::PaintSurfaceRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFilterDomain {
    Color,
    Scalar,
}

impl From<PaintSurfaceRole> for SurfaceFilterDomain {
    fn from(role: PaintSurfaceRole) -> Self {
        match role {
            PaintSurfaceRole::Raster => Self::Color,
            PaintSurfaceRole::LayerMask => Self::Scalar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialBlurOrientation {
    Ignore,
    SimilarNormals,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_surface_roles_map_to_filter_domains() {
        assert_eq!(
            SurfaceFilterDomain::from(PaintSurfaceRole::Raster),
            SurfaceFilterDomain::Color
        );
        assert_eq!(
            SurfaceFilterDomain::from(PaintSurfaceRole::LayerMask),
            SurfaceFilterDomain::Scalar
        );
    }
}

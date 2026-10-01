use glam::Vec2;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EmbeddedImageId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedImageAsset {
    pub id: EmbeddedImageId,
    pub file_name: String,
    pub size: [u32; 2],
    /// Immutable straight-alpha RGBA8 pixels in row-major order.
    pub rgba8: Arc<[u8]>,
}

impl EmbeddedImageAsset {
    pub fn new(
        id: EmbeddedImageId,
        file_name: String,
        size: [u32; 2],
        rgba8: Arc<[u8]>,
    ) -> Option<Self> {
        let expected = pixel_byte_len(size)?;
        (id.0 != 0 && rgba8.len() == expected).then_some(Self {
            id,
            file_name,
            size,
            rgba8,
        })
    }

    pub fn is_valid(&self) -> bool {
        Self::new(
            self.id,
            self.file_name.clone(),
            self.size,
            self.rgba8.clone(),
        )
        .is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmbeddedImageTransform {
    pub center_uv: Vec2,
    pub size_uv: Vec2,
    pub rotation_radians: f32,
}

impl EmbeddedImageTransform {
    pub fn initial(source_size: [u32; 2], material_size: [u32; 2]) -> Option<Self> {
        if source_size.contains(&0) || material_size.contains(&0) {
            return None;
        }
        Some(Self {
            center_uv: Vec2::splat(0.5),
            size_uv: Vec2::new(
                source_size[0] as f32 / material_size[0] as f32,
                source_size[1] as f32 / material_size[1] as f32,
            ),
            rotation_radians: 0.0,
        })
    }

    pub fn is_valid(self) -> bool {
        self.center_uv.is_finite()
            && self.size_uv.is_finite()
            && self.rotation_radians.is_finite()
            && self.size_uv.x != 0.0
            && self.size_uv.y != 0.0
    }
}

pub fn pixel_byte_len(size: [u32; 2]) -> Option<usize> {
    let pixels = usize::try_from(size[0])
        .ok()?
        .checked_mul(usize::try_from(size[1]).ok()?)?;
    pixels.checked_mul(4).filter(|_| !size.contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_transform_preserves_source_pixel_scale() {
        let transform = EmbeddedImageTransform::initial([1024, 512], [2048, 2048]).unwrap();
        assert_eq!(transform.center_uv, Vec2::splat(0.5));
        assert_eq!(transform.size_uv, Vec2::new(0.5, 0.25));
        assert!(transform.is_valid());
    }

    #[test]
    fn asset_rejects_invalid_payload() {
        assert!(
            EmbeddedImageAsset::new(EmbeddedImageId(1), String::new(), [2, 1], Arc::from([0; 7]))
                .is_none()
        );
    }
}

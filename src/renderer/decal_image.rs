use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        decal::{DecalImageAsset, DecalImageId},
        render_report::GpuTextureMetrics,
    },
    renderer::gpu::frame::GpuFrame,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecalImageKey {
    pub(crate) id: DecalImageId,
    pub(crate) size: [u32; 2],
}

impl DecalImageKey {
    fn from_asset(image: &DecalImageAsset) -> Option<Self> {
        let expected_len = (image.size[0] as usize)
            .checked_mul(image.size[1] as usize)
            .and_then(|pixels| pixels.checked_mul(4))?;
        (image.size[0] > 0 && image.size[1] > 0 && image.rgba8.len() == expected_len).then_some(
            Self {
                id: image.id,
                size: image.size,
            },
        )
    }
}

pub(crate) struct DecalImageBinding<'a> {
    pub(crate) key: DecalImageKey,
    pub(crate) view: &'a wgpu::TextureView,
}

pub(crate) struct DecalImageCache {
    cached: Option<CachedDecalImage>,
}

struct CachedDecalImage {
    key: DecalImageKey,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl DecalImageCache {
    pub(crate) fn new() -> Self {
        Self { cached: None }
    }

    pub(crate) fn ensure<'a>(
        &'a mut self,
        frame: &mut GpuFrame,
        device: &wgpu::Device,
        image: &DecalImageAsset,
    ) -> Option<DecalImageBinding<'a>> {
        let key = DecalImageKey::from_asset(image)?;
        if self.cached.as_ref().is_none_or(|cached| cached.key != key) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("decal_image_shared"),
                size: wgpu::Extent3d {
                    width: image.size[0],
                    height: image.size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            frame.write_texture_rgba8(device, &texture, [0, 0], image.size, &image.rgba8);
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.cached = Some(CachedDecalImage {
                key,
                _texture: texture,
                view,
            });
        }
        let cached = self.cached.as_ref()?;
        Some(DecalImageBinding {
            key: cached.key,
            view: &cached.view,
        })
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let Some(cached) = self.cached.as_ref() else {
            return GpuTextureMetrics::default();
        };
        let bytes = (cached.key.size[0] as usize)
            .saturating_mul(cached.key.size[1] as usize)
            .saturating_mul(4);
        let mut metrics = GpuTextureMetrics::default();
        metrics.add_total_bytes(bytes);
        metrics.view_bytes = bytes;
        metrics.view_texture_count = 1;
        metrics
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::core::decal::{DecalImageAsset, DecalImageId};

    use super::DecalImageKey;

    fn image(id: u64, size: [u32; 2], rgba8: Vec<u8>) -> DecalImageAsset {
        DecalImageAsset {
            id: DecalImageId(id),
            file_name: "decal.png".to_owned(),
            size,
            rgba8: Arc::from(rgba8),
        }
    }

    #[test]
    fn image_key_reuses_same_asset_identity_and_size() {
        let first = image(7, [2, 1], vec![255; 8]);
        let same = image(7, [2, 1], vec![0; 8]);

        assert_eq!(
            DecalImageKey::from_asset(&first),
            DecalImageKey::from_asset(&same)
        );
    }

    #[test]
    fn image_key_rejects_invalid_pixel_payloads() {
        assert!(DecalImageKey::from_asset(&image(1, [0, 1], Vec::new())).is_none());
        assert!(DecalImageKey::from_asset(&image(1, [2, 1], vec![0; 7])).is_none());
    }
}

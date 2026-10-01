use anyhow::{Result, anyhow, bail};

use crate::core::surface::PaintSurfaceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Snapshot {
    pub texture_size: [u32; 2],
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub rgba8: Vec<u8>,
}

impl Rgba8Snapshot {
    pub fn new(
        texture_size: [u32; 2],
        origin: [u32; 2],
        size: [u32; 2],
        rgba8: Vec<u8>,
    ) -> Result<Self> {
        validate_rect(texture_size, origin, size)?;
        let expected_len = rgba8_len(size)?;
        if rgba8.len() != expected_len {
            bail!(
                "invalid RGBA payload size: got {}, expected {}",
                rgba8.len(),
                expected_len
            );
        }
        Ok(Self {
            texture_size,
            origin,
            size,
            rgba8,
        })
    }
}

pub fn validate_rect(texture_size: [u32; 2], origin: [u32; 2], size: [u32; 2]) -> Result<()> {
    if size[0] == 0 || size[1] == 0 {
        bail!("snapshot rect size must be > 0");
    }
    let max_x = origin[0]
        .checked_add(size[0])
        .ok_or_else(|| anyhow!("snapshot rect x range overflows"))?;
    let max_y = origin[1]
        .checked_add(size[1])
        .ok_or_else(|| anyhow!("snapshot rect y range overflows"))?;
    if max_x > texture_size[0] || max_y > texture_size[1] {
        bail!(
            "snapshot rect origin {:?} size {:?} is outside texture {:?}",
            origin,
            size,
            texture_size
        );
    }
    Ok(())
}

pub fn rgba8_len(size: [u32; 2]) -> Result<usize> {
    (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("RGBA payload size overflows usize"))
}

pub fn unpremultiply_rgba8(rgba8: &mut [u8]) {
    for pixel in rgba8.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[..3].fill(0);
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_restores_translucent_rgb_and_clears_transparent_rgb() {
        let mut rgba = vec![100, 50, 25, 128, 20, 10, 5, 0];
        unpremultiply_rgba8(&mut rgba);
        assert_eq!(rgba, [199, 100, 50, 128, 0, 0, 0, 0]);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerImageSnapshot {
    pub surface: PaintSurfaceId,
    pub texture_size: [u32; 2],
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MaterialPayload {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum LayerInitialPixels {
    Transparent,
    SolidRgba8([u8; 4]),
}

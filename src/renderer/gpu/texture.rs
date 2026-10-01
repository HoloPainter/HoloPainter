use eframe::egui_wgpu::wgpu;

use crate::core::geometry::RectU32;

pub(crate) fn create_paint_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_rgba_texture(
        device,
        size,
        label,
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
    )
}

pub(crate) fn create_mask_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_r8_texture(
        device,
        size,
        label,
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
    )
}

pub(crate) fn create_composite_output_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

pub(crate) fn create_render_scratch_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn create_rgba_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn create_r8_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

pub(crate) fn create_viewport_stroke_target(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_r8_texture(
        device,
        size,
        label,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
    )
}

pub(crate) fn create_color_target(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

pub(crate) fn create_depth_target_only(
    device: &wgpu::Device,
    size: [u32; 2],
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewport_depth"),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

pub(crate) fn create_depth_targets(
    device: &wgpu::Device,
    viewport_size: [u32; 2],
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let depth_color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth_color"),
        size: wgpu::Extent3d {
            width: viewport_size[0].max(1),
            height: viewport_size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let depth_color_view = depth_color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_z = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth_z"),
        size: wgpu::Extent3d {
            width: viewport_size[0].max(1),
            height: viewport_size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_z_view = depth_z.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_color, depth_color_view, depth_z, depth_z_view)
}

pub(crate) fn copy_a_to_b(
    enc: &mut wgpu::CommandEncoder,
    paint_a: &wgpu::Texture,
    paint_b: &wgpu::Texture,
    size: [u32; 2],
) {
    enc.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: paint_a,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: paint_b,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
}

pub(crate) fn copy_rects_a_to_b(
    enc: &mut wgpu::CommandEncoder,
    paint_a: &wgpu::Texture,
    paint_b: &wgpu::Texture,
    texture_size: [u32; 2],
    rects: &[RectU32],
) {
    for rect in rects {
        debug_assert!(rect.origin[0].saturating_add(rect.size[0]) <= texture_size[0]);
        debug_assert!(rect.origin[1].saturating_add(rect.size[1]) <= texture_size[1]);
        if rect.origin == [0, 0] && rect.size == texture_size {
            copy_a_to_b(enc, paint_a, paint_b, texture_size);
        } else {
            copy_rect_a_to_b(enc, paint_a, paint_b, *rect);
        }
    }
}

pub(crate) fn copy_rect_a_to_b(
    enc: &mut wgpu::CommandEncoder,
    paint_a: &wgpu::Texture,
    paint_b: &wgpu::Texture,
    rect: RectU32,
) {
    if rect.size[0] == 0 || rect.size[1] == 0 {
        return;
    }
    enc.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: paint_a,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.origin[0],
                y: rect.origin[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: paint_b,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.origin[0],
                y: rect.origin[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: rect.size[0],
            height: rect.size[1],
            depth_or_array_layers: 1,
        },
    );
}

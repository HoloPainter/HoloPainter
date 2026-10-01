use std::collections::{HashMap, HashSet};

use super::v1::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectValidationError(pub String);

impl std::fmt::Display for ProjectValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ProjectValidationError {}

fn invalid(message: impl Into<String>) -> ProjectValidationError {
    ProjectValidationError(message.into())
}

pub fn validate_project_v1(
    project: &ProjectV1,
    geometry_len: u64,
) -> Result<(), ProjectValidationError> {
    if project.format_version != 1 {
        return Err(invalid("format_version must equal 1"));
    }
    let project_id = uuid::Uuid::parse_str(&project.project_id)
        .map_err(|_| invalid("project_id is not a canonical UUID"))?;
    if project_id.hyphenated().to_string() != project.project_id {
        return Err(invalid("project_id is not a canonical lowercase UUID"));
    }
    if project.created_with.trim().is_empty() {
        return Err(invalid("created_with must not be empty or whitespace"));
    }
    validate_mesh(project, geometry_len)?;
    let materials = validate_materials(project)?;
    let embedded_images = validate_embedded_images(project)?;
    let layers = validate_layers(project, &materials, &embedded_images)?;
    validate_editor_state(&project.editor_state)?;
    validate_surfaces(project, &materials, &layers)
}

fn validate_editor_state(editor: &ProjectEditorStateV1) -> Result<(), ProjectValidationError> {
    validate_editor_view_state_v1(&editor.view)
}

pub(crate) fn validate_editor_view_state_v1(
    view: &EditorViewStateV1,
) -> Result<(), ProjectValidationError> {
    let camera = view.viewport_3d.camera;
    if !camera.target.into_iter().all(f32::is_finite)
        || !camera.orientation.into_iter().all(f32::is_finite)
        || !camera.distance.is_finite()
        || camera.distance <= 0.0
        || !camera.fov_y_radians.is_finite()
        || !(0.0..std::f32::consts::PI).contains(&camera.fov_y_radians)
        || !camera.orthographic_height.is_finite()
        || camera.orthographic_height <= 0.0
    {
        return Err(invalid("editor camera contains an invalid value"));
    }
    let orientation_length_squared = camera
        .orientation
        .into_iter()
        .map(|component| component * component)
        .sum::<f32>();
    if !orientation_length_squared.is_finite() || orientation_length_squared <= f32::EPSILON {
        return Err(invalid("editor camera orientation is invalid"));
    }

    validate_color(
        "editor_state.view.viewport_3d.background_color",
        view.viewport_3d.background_color,
    )?;
    validate_wireframe(
        "editor_state.view.viewport_3d.wireframe",
        view.viewport_3d.wireframe,
    )?;
    if !view.viewport_3d.surface_mirror.x_plane.is_finite() {
        return Err(invalid(
            "editor_state.view.viewport_3d.surface_mirror.x_plane must be finite",
        ));
    }

    let uv = view.uv.transform;
    if !uv.center_uv.into_iter().all(f32::is_finite)
        || !uv.zoom.is_finite()
        || uv.zoom <= 0.0
        || !uv.rotation_radians.is_finite()
    {
        return Err(invalid("editor UV view contains an invalid value"));
    }
    validate_color(
        "editor_state.view.uv.background_color",
        view.uv.background_color,
    )?;
    validate_wireframe("editor_state.view.uv.wireframe", view.uv.wireframe)?;
    Ok(())
}

fn validate_wireframe(
    name: &str,
    wireframe: WireframeViewStateV1,
) -> Result<(), ProjectValidationError> {
    validate_color(&format!("{name}.color"), wireframe.color)?;
    validate_unit(&format!("{name}.opacity"), wireframe.opacity)
}

fn validate_color(name: &str, color: [f32; 3]) -> Result<(), ProjectValidationError> {
    for (index, component) in color.into_iter().enumerate() {
        validate_unit(&format!("{name}[{index}]"), component)?;
    }
    Ok(())
}

fn validate_unit(name: &str, value: f32) -> Result<(), ProjectValidationError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid(format!("{name} must be finite and within 0..=1")));
    }
    Ok(())
}

fn validate_embedded_images(project: &ProjectV1) -> Result<HashSet<u64>, ProjectValidationError> {
    let mut ids = HashSet::new();
    for image in &project.embedded_images {
        if image.id == 0 || !ids.insert(image.id) {
            return Err(invalid("embedded image IDs must be unique and non-zero"));
        }
        if image.width == 0
            || image.height == 0
            || u64::from(image.width)
                .checked_mul(u64::from(image.height))
                .and_then(|n| n.checked_mul(4))
                .is_none()
        {
            return Err(invalid(format!(
                "embedded image {} dimensions are invalid",
                image.id
            )));
        }
    }
    if project.id_counters.embedded_image < ids.iter().copied().max().unwrap_or(0) {
        return Err(invalid("embedded image ID counter is below a current ID"));
    }
    Ok(ids)
}

fn validate_mesh(project: &ProjectV1, geometry_len: u64) -> Result<(), ProjectValidationError> {
    let mesh = &project.mesh;
    if mesh.vertex_count == 0 || mesh.triangle_count == 0 {
        return Err(invalid("canonical mesh must be non-empty"));
    }
    if mesh.objects.is_empty() || mesh.primitives.is_empty() {
        return Err(invalid("mesh must contain an object and a primitive"));
    }
    if mesh.position.count != mesh.vertex_count
        || mesh.normal.count != mesh.vertex_count
        || mesh.texcoord0.count != mesh.vertex_count
    {
        return Err(invalid("mesh attribute counts must equal vertex_count"));
    }
    let expected_index_count = mesh
        .triangle_count
        .checked_mul(3)
        .ok_or_else(|| invalid("triangle index count overflows u64"))?;
    if mesh.indices.count != expected_index_count {
        return Err(invalid("index count must equal triangle_count * 3"));
    }

    let ranges = [
        geometry_range(
            mesh.position.offset,
            u64::from(mesh.position.count),
            12,
            geometry_len,
        )?,
        geometry_range(
            mesh.normal.offset,
            u64::from(mesh.normal.count),
            12,
            geometry_len,
        )?,
        geometry_range(
            mesh.texcoord0.offset,
            u64::from(mesh.texcoord0.count),
            8,
            geometry_len,
        )?,
        geometry_range(mesh.indices.offset, mesh.indices.count, 4, geometry_len)?,
    ];
    for (index, left) in ranges.iter().enumerate() {
        if left.0 % 4 != 0 {
            return Err(invalid("geometry arrays must be 4-byte aligned"));
        }
        for right in &ranges[index + 1..] {
            if left.0 < right.1 && right.0 < left.1 {
                return Err(invalid("geometry array ranges overlap"));
            }
        }
    }

    let mut object_ids = HashSet::new();
    for object in &mesh.objects {
        if object.id == 0 || !object_ids.insert(object.id) {
            return Err(invalid("mesh object IDs must be unique and non-zero"));
        }
    }
    if project.id_counters.mesh_object < object_ids.iter().copied().max().unwrap_or(0) {
        return Err(invalid("mesh object ID counter is below a current ID"));
    }

    let material_ids = project
        .materials
        .iter()
        .map(|material| material.id)
        .collect::<HashSet<_>>();
    let mut primitive_ranges = Vec::with_capacity(mesh.primitives.len());
    for primitive in &mesh.primitives {
        if !object_ids.contains(&primitive.object_id)
            || !material_ids.contains(&primitive.material_id)
        {
            return Err(invalid(
                "primitive references an unknown object or material",
            ));
        }
        if primitive.first_index % 3 != 0
            || primitive.index_count == 0
            || primitive.index_count % 3 != 0
        {
            return Err(invalid("primitive range is not triangle-aligned"));
        }
        let end = primitive
            .first_index
            .checked_add(primitive.index_count)
            .ok_or_else(|| invalid("primitive range overflows u64"))?;
        if end > mesh.indices.count {
            return Err(invalid("primitive range exceeds the index array"));
        }
        primitive_ranges.push((primitive.first_index, end));
    }
    primitive_ranges.sort_unstable();
    let mut cursor = 0;
    for (start, end) in primitive_ranges {
        if start != cursor {
            return Err(invalid("primitive ranges must partition the index array"));
        }
        cursor = end;
    }
    if cursor != mesh.indices.count {
        return Err(invalid("primitive ranges do not cover the index array"));
    }
    Ok(())
}

fn geometry_range(
    offset: u64,
    count: u64,
    element_size: u64,
    geometry_len: u64,
) -> Result<(u64, u64), ProjectValidationError> {
    let byte_len = count
        .checked_mul(element_size)
        .ok_or_else(|| invalid("geometry byte length overflows u64"))?;
    let end = offset
        .checked_add(byte_len)
        .ok_or_else(|| invalid("geometry range overflows u64"))?;
    if end > geometry_len {
        return Err(invalid("geometry range exceeds geometry.bin"));
    }
    Ok((offset, end))
}

fn validate_materials(project: &ProjectV1) -> Result<HashSet<u64>, ProjectValidationError> {
    let mut ids = HashSet::new();
    for material in &project.materials {
        if material.id == 0 || !ids.insert(material.id) {
            return Err(invalid("material IDs must be unique and non-zero"));
        }
        if material.texture_size.width == 0 || material.texture_size.height == 0 {
            return Err(invalid("material texture dimensions must be non-zero"));
        }
    }
    if project.id_counters.material < ids.iter().copied().max().unwrap_or(0) {
        return Err(invalid("material ID counter is below a current ID"));
    }
    Ok(ids)
}

fn validate_layers<'a>(
    project: &'a ProjectV1,
    materials: &HashSet<u64>,
    embedded_images: &HashSet<u64>,
) -> Result<HashMap<u64, &'a LayerV1>, ProjectValidationError> {
    let mut layers = HashMap::new();
    for layer in &project.layers {
        if layer.id == 0 || layers.insert(layer.id, layer).is_some() {
            return Err(invalid("layer IDs must be unique and non-zero"));
        }
        if !layer.opacity.is_finite() || !(0.0..=1.0).contains(&layer.opacity) {
            return Err(invalid(format!("layer {} has invalid opacity", layer.id)));
        }
        if let Some(mask) = &layer.material_mask {
            let unique = mask.iter().copied().collect::<HashSet<_>>();
            if unique.len() != mask.len() || !unique.is_subset(materials) {
                return Err(invalid(format!(
                    "layer {} has an invalid material mask",
                    layer.id
                )));
            }
        }
        if layer.mask.is_some()
            && matches!(
                layer.content,
                LayerContentV1::Adjustment {
                    adjustment: AdjustmentV1::UvMirror { .. }
                }
            )
        {
            return Err(invalid(format!(
                "UV Mirror layer {} cannot have a layer mask",
                layer.id
            )));
        }
        validate_layer_content(layer, embedded_images)?;
    }
    if project.id_counters.layer < layers.keys().copied().max().unwrap_or(0) {
        return Err(invalid("layer ID counter is below a current ID"));
    }
    let roots = project
        .layers
        .iter()
        .filter(|layer| layer.parent_id.is_none())
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return Err(invalid("layer tree must contain exactly one root"));
    }
    let root = roots[0];
    if root.order != 0
        || root.mask.is_some()
        || root.material_mask.is_some()
        || root.content
            != (LayerContentV1::Group {
                composite_mode: GroupCompositeModeV1::PassThrough,
            })
    {
        return Err(invalid("root layer violates the format v1 contract"));
    }
    if !project
        .layers
        .iter()
        .any(|layer| layer.content == LayerContentV1::Raster)
    {
        return Err(invalid("layer tree must contain a raster layer"));
    }

    let mut sibling_orders = HashSet::new();
    for layer in &project.layers {
        let Some(parent_id) = layer.parent_id else {
            continue;
        };
        let parent = layers
            .get(&parent_id)
            .ok_or_else(|| invalid(format!("layer {} has an unknown parent", layer.id)))?;
        if !matches!(parent.content, LayerContentV1::Group { .. }) {
            return Err(invalid(format!("layer {} parent is not a group", layer.id)));
        }
        if !sibling_orders.insert((parent_id, layer.order)) {
            return Err(invalid("sibling layer order values must be unique"));
        }
        let mut seen = HashSet::new();
        let mut current = Some(layer.id);
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(invalid("layer tree contains a cycle"));
            }
            current = layers.get(&id).and_then(|node| node.parent_id);
        }
    }
    Ok(layers)
}

fn validate_layer_content(
    layer: &LayerV1,
    embedded_images: &HashSet<u64>,
) -> Result<(), ProjectValidationError> {
    match &layer.content {
        LayerContentV1::SolidFill { color } => finite_range(color, 0.0, 1.0, "solid fill")?,
        LayerContentV1::Adjustment { adjustment } => validate_adjustment(adjustment)?,
        LayerContentV1::EmbeddedImage {
            image_id,
            transform,
        } => {
            if !embedded_images.contains(image_id) {
                return Err(invalid(format!(
                    "layer {} references an unknown embedded image",
                    layer.id
                )));
            }
            if !matches!(&layer.material_mask, Some(ids) if ids.len() == 1) {
                return Err(invalid(format!(
                    "embedded image layer {} must target exactly one material",
                    layer.id
                )));
            }
            let values = [
                transform.center_uv[0],
                transform.center_uv[1],
                transform.size_uv[0],
                transform.size_uv[1],
                transform.rotation_radians,
            ];
            if values.iter().any(|value| !value.is_finite()) || transform.size_uv.contains(&0.0) {
                return Err(invalid(format!(
                    "embedded image layer {} has an invalid transform",
                    layer.id
                )));
            }
        }
        LayerContentV1::Raster | LayerContentV1::Group { .. } => {}
    }
    Ok(())
}

fn validate_adjustment(adjustment: &AdjustmentV1) -> Result<(), ProjectValidationError> {
    match adjustment {
        AdjustmentV1::BrightnessContrast {
            brightness,
            contrast,
        } => {
            if !(-150..=150).contains(brightness) || !(-50..=100).contains(contrast) {
                return Err(invalid("brightness/contrast adjustment is invalid"));
            }
            Ok(())
        }
        AdjustmentV1::Levels {
            master,
            red,
            green,
            blue,
        } => {
            for channel in [master, red, green, blue] {
                if !channel.gamma.is_finite()
                    || channel.input_black > 253
                    || channel.input_white < 2
                    || channel.input_black >= channel.input_white
                    || !(0.1..=9.99).contains(&channel.gamma)
                {
                    return Err(invalid("levels adjustment is invalid"));
                }
            }
            Ok(())
        }
        AdjustmentV1::Curves {
            master,
            red,
            green,
            blue,
        } => {
            for points in [master, red, green, blue] {
                if !(2..=19).contains(&points.len())
                    || points.windows(2).any(|pair| pair[0].input >= pair[1].input)
                {
                    return Err(invalid("curve point count or input order is invalid"));
                }
            }
            Ok(())
        }
        AdjustmentV1::HueSaturation {
            hue,
            saturation,
            lightness,
        } => {
            if !(-180..=180).contains(hue)
                || !(-100..=100).contains(saturation)
                || !(-100..=100).contains(lightness)
            {
                return Err(invalid("hue/saturation adjustment is invalid"));
            }
            Ok(())
        }
        AdjustmentV1::Invert => Ok(()),
        AdjustmentV1::GradientMap { stops, .. } => {
            if !(2..=4097).contains(&stops.len())
                || stops
                    .iter()
                    .any(|stop| stop.location > 4096 || !(5..=95).contains(&stop.midpoint))
                || stops
                    .windows(2)
                    .any(|pair| pair[0].location >= pair[1].location)
            {
                return Err(invalid("gradient stop count is invalid"));
            }
            Ok(())
        }
        AdjustmentV1::UvMirror { position, .. } => {
            finite_range(&[*position], 0.0, 1.0, "UV mirror position")
        }
    }
}

fn finite_range(
    values: &[f32],
    min: f32,
    max: f32,
    label: &str,
) -> Result<(), ProjectValidationError> {
    if values
        .iter()
        .any(|value| !value.is_finite() || !(min..=max).contains(value))
    {
        return Err(invalid(format!("{label} values are invalid")));
    }
    Ok(())
}

fn validate_surfaces(
    project: &ProjectV1,
    materials: &HashSet<u64>,
    layers: &HashMap<u64, &LayerV1>,
) -> Result<(), ProjectValidationError> {
    let material_sizes = project
        .materials
        .iter()
        .map(|material| (material.id, material.texture_size))
        .collect::<HashMap<_, _>>();
    let mut actual = HashSet::new();
    for surface in &project.surfaces {
        let identity = (surface.layer_id, surface.material_id, surface.role);
        if !actual.insert(identity) {
            return Err(invalid("surface identities must be unique"));
        }
        let layer = layers
            .get(&surface.layer_id)
            .ok_or_else(|| invalid("surface references an unknown layer"))?;
        if !materials.contains(&surface.material_id) {
            return Err(invalid("surface references an unknown material"));
        }
        let size = material_sizes[&surface.material_id];
        if surface.width != size.width || surface.height != size.height || surface.tile_size != 256
        {
            return Err(invalid("surface dimensions or tile size are invalid"));
        }
        match surface.role {
            SurfaceRoleV1::Raster
                if layer.content == LayerContentV1::Raster
                    && surface.pixel_format == PixelFormatV1::Rgba8Unorm
                    && surface.default_fill == SurfaceDefaultV1::TransparentBlack => {}
            SurfaceRoleV1::LayerMask
                if layer.mask.is_some()
                    && layer.parent_id.is_some()
                    && surface.pixel_format == PixelFormatV1::R8Unorm
                    && matches!(
                        surface.default_fill,
                        SurfaceDefaultV1::MaskHidden | SurfaceDefaultV1::MaskRevealed
                    ) => {}
            _ => {
                return Err(invalid(
                    "surface role metadata is inconsistent with its layer",
                ));
            }
        }
        let cols = surface.width.div_ceil(surface.tile_size);
        let rows = surface.height.div_ceil(surface.tile_size);
        let mut coordinates = HashSet::new();
        for tile in &surface.tiles {
            if tile.x >= cols
                || tile.y >= rows
                || tile.compressed_size == 0
                || !coordinates.insert((tile.x, tile.y))
            {
                return Err(invalid("surface tile metadata is invalid"));
            }
        }
        surface.tiles.iter().try_fold(0_u64, |sum, tile| {
            sum.checked_add(u64::from(tile.compressed_size))
                .ok_or_else(|| invalid("surface blob size overflows u64"))
        })?;
    }

    let mut expected = HashSet::new();
    for layer in &project.layers {
        for material in &project.materials {
            if layer.content == LayerContentV1::Raster {
                expected.insert((layer.id, material.id, SurfaceRoleV1::Raster));
            }
            if layer.parent_id.is_some() && layer.mask.is_some() {
                expected.insert((layer.id, material.id, SurfaceRoleV1::LayerMask));
            }
        }
    }
    if actual != expected {
        return Err(invalid(
            "surface set does not match required layer/material surfaces",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod adjustment_tests {
    use super::*;

    #[test]
    fn curves_allow_arbitrary_ordered_endpoint_inputs() {
        let points = vec![
            CurvePointV1 {
                input: 32,
                output: 0,
            },
            CurvePointV1 {
                input: 128,
                output: 170,
            },
            CurvePointV1 {
                input: 220,
                output: 255,
            },
        ];
        let adjustment = AdjustmentV1::Curves {
            master: points.clone(),
            red: points.clone(),
            green: points.clone(),
            blue: points,
        };
        validate_adjustment(&adjustment).unwrap();
    }
}

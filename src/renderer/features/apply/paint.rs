use crate::{
    core::composite::{ApplyParams, TextureCompositeMode as CoreTextureCompositeMode},
    renderer::features::apply::{
        operation::ApplyOperation,
        types::{UvCompositeUniform, UvTextureCompositeMode},
    },
};

pub(crate) fn composite_uniform_from_apply_operation(
    operation: &ApplyOperation,
    params: ApplyParams<CoreTextureCompositeMode>,
) -> UvCompositeUniform {
    let composite_mode = match params.composite {
        CoreTextureCompositeMode::SourceOver => UvTextureCompositeMode::SourceOver,
        CoreTextureCompositeMode::DestinationOut => UvTextureCompositeMode::DestinationOut,
        CoreTextureCompositeMode::Clear => UvTextureCompositeMode::Clear,
        CoreTextureCompositeMode::Multiply => UvTextureCompositeMode::Multiply,
    };
    match operation {
        ApplyOperation::SolidColorPaint { color } => {
            let rgb = if params.composite == CoreTextureCompositeMode::DestinationOut {
                [0.0; 3]
            } else {
                *color
            };
            UvCompositeUniform {
                paint_rgb_opacity: [rgb[0], rgb[1], rgb[2], params.opacity],
                composite_mode: composite_mode as u32,
                selection_enabled: 0,
                _pad0: [0, 0],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::composite::{ApplyParams, TextureCompositeMode};

    use super::{ApplyOperation, composite_uniform_from_apply_operation};

    #[test]
    fn source_over_packs_rgb_and_opacity_without_color_alpha() {
        let uniform = composite_uniform_from_apply_operation(
            &ApplyOperation::SolidColorPaint {
                color: [0.2, 0.4, 0.6],
            },
            ApplyParams::new(0.25, TextureCompositeMode::SourceOver),
        );

        assert_eq!(uniform.paint_rgb_opacity, [0.2, 0.4, 0.6, 0.25]);
    }

    #[test]
    fn destination_out_uses_only_opacity() {
        let uniform = composite_uniform_from_apply_operation(
            &ApplyOperation::SolidColorPaint {
                color: [0.2, 0.4, 0.6],
            },
            ApplyParams::new(0.25, TextureCompositeMode::DestinationOut),
        );

        assert_eq!(uniform.paint_rgb_opacity, [0.0, 0.0, 0.0, 0.25]);
    }
}

use crate::core::{stroke_preset::StrokeOp, tool_operation::ToolOperation};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStrokeStyle {
    pub stroke_op: StrokeOp,
    pub operation: ToolOperation,
}

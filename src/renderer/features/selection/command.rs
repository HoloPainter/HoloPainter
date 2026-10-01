use crate::core::{
    composite::{ApplyParams, SelectionCompositeMode},
    mask::MaskSource,
    selection::ActiveSelection,
};

#[derive(Debug, Clone)]
pub enum SelectionEditCommand {
    Update {
        mask: MaskSource,
        params: ApplyParams<SelectionCompositeMode>,
        active_selection: ActiveSelection,
    },
}

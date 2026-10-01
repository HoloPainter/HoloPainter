use crate::core::selection::ActiveSelection;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SelectionDamage {
    MaskChanged { active_selection: ActiveSelection },
    TilesUploaded { active_selection: ActiveSelection },
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct SelectionDamageSet {
    entries: Vec<SelectionDamage>,
}

impl SelectionDamageSet {
    pub(crate) fn mask_changed(&mut self, active_selection: ActiveSelection) {
        self.entries
            .push(SelectionDamage::MaskChanged { active_selection });
    }

    pub(crate) fn tiles_uploaded(&mut self, active_selection: ActiveSelection) {
        self.entries
            .push(SelectionDamage::TilesUploaded { active_selection });
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

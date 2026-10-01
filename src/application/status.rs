use crate::core::composite::SelectionCompositeMode;
use crate::localization::Localization;

pub(crate) const fn selection_status_key(operation: SelectionCompositeMode) -> &'static str {
    match operation {
        SelectionCompositeMode::Replace => "status-selection-replace",
        SelectionCompositeMode::Add => "status-selection-add",
        SelectionCompositeMode::Subtract => "status-selection-subtract",
        SelectionCompositeMode::Intersect => "status-selection-intersect",
        SelectionCompositeMode::Difference => "status-selection-difference",
        SelectionCompositeMode::Clear => "status-selection-clear",
        SelectionCompositeMode::Invert => "status-selection-invert",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusMessage {
    Localized {
        key: &'static str,
        args: Vec<(&'static str, String)>,
    },
    Raw(String),
}

impl StatusMessage {
    pub fn localized(key: &'static str) -> Self {
        Self::Localized {
            key,
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, name: &'static str, value: impl ToString) -> Self {
        match &mut self {
            Self::Localized { args, .. } => args.push((name, value.to_string())),
            Self::Raw(_) => debug_assert!(false, "arguments require a localized status message"),
        }
        self
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Self::Raw(text.into())
    }

    pub fn format(&self, l10n: &Localization) -> String {
        match self {
            Self::Localized { key, args } => {
                let mut fluent_args = fluent::FluentArgs::new();
                for (name, value) in args {
                    fluent_args.set(*name, value.as_str());
                }
                l10n.format(key, Some(&fluent_args))
            }
            Self::Raw(text) => text.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, pattern: &str) -> bool {
        self.test_english_text().contains(pattern)
    }

    #[cfg(test)]
    fn test_english_text(&self) -> String {
        Localization::with_system_locale(
            crate::settings::LanguagePreference::English,
            Some("en-US"),
        )
        .format_status(self)
    }
}

#[cfg(test)]
impl PartialEq<str> for StatusMessage {
    fn eq(&self, other: &str) -> bool {
        self.test_english_text() == other
    }
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self::localized("status-ready")
    }
}

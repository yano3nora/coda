//! In-memory keybinding model.

use crate::input::KeyEvent;

use super::{EditorAction, predicate::ContextPredicate};

/// Origin of a binding. Higher-priority sources win during resolution.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Source {
    Rescue,
    User,
    Imported,
    Default,
}

impl Source {
    pub(crate) const fn priority(self) -> u8 {
        match self {
            Self::Rescue => 3,
            Self::User => 2,
            Self::Imported => 1,
            Self::Default => 0,
        }
    }
}

/// A key sequence mapped to an editor action under an optional context predicate.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Binding {
    pub keys: Vec<KeyEvent>,
    pub action: EditorAction,
    pub when: Option<ContextPredicate>,
    pub source: Source,
}

impl Binding {
    pub fn new(
        keys: Vec<KeyEvent>,
        action: EditorAction,
        when: Option<ContextPredicate>,
        source: Source,
    ) -> Self {
        Self {
            keys,
            action,
            when,
            source,
        }
    }

    pub(crate) fn term_count(&self) -> usize {
        self.when
            .as_ref()
            .map(ContextPredicate::term_count)
            .unwrap_or(0)
    }
}

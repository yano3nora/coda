//! Editor context flags used by `when` predicates.

/// Snapshot of editor/UI state that may affect keybinding resolution.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EditorContext {
    pub editor_focus: bool,
    pub text_input_focus: bool,
    pub has_selection: bool,
    pub has_multiple_selections: bool,
    pub is_readonly: bool,
    pub search_visible: bool,
    pub replace_visible: bool,
    pub command_palette_visible: bool,
    pub list_focus: bool,
    /// Reserved for future completion UI; false in MVP unless tests/app set it.
    pub suggest_visible: bool,
    /// Reserved for future quick-open UI; false in MVP unless tests/app set it.
    pub quick_open_visible: bool,
    pub tab_focus: bool,
    pub split_focus: bool,
}

impl Default for EditorContext {
    fn default() -> Self {
        Self {
            editor_focus: true,
            text_input_focus: true,
            has_selection: false,
            has_multiple_selections: false,
            is_readonly: false,
            search_visible: false,
            replace_visible: false,
            command_palette_visible: false,
            list_focus: false,
            suggest_visible: false,
            quick_open_visible: false,
            tab_focus: false,
            split_focus: false,
        }
    }
}

impl EditorContext {
    /// Looks up a canonical SPEC-0002 field name.
    pub const fn get(self, name: &str) -> Option<bool> {
        match name.as_bytes() {
            b"editorFocus" => Some(self.editor_focus),
            b"textInputFocus" => Some(self.text_input_focus),
            b"hasSelection" => Some(self.has_selection),
            b"hasMultipleSelections" => Some(self.has_multiple_selections),
            b"isReadonly" => Some(self.is_readonly),
            b"searchVisible" => Some(self.search_visible),
            b"replaceVisible" => Some(self.replace_visible),
            b"commandPaletteVisible" => Some(self.command_palette_visible),
            b"listFocus" => Some(self.list_focus),
            b"suggestVisible" => Some(self.suggest_visible),
            b"quickOpenVisible" => Some(self.quick_open_visible),
            b"tabFocus" => Some(self.tab_focus),
            b"splitFocus" => Some(self.split_focus),
            _ => None,
        }
    }
}

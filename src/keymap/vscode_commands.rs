//! VS Code command name → internal action mapping (SPEC-0004 import scope).
//!
//! This table is the single source of truth shared by the VS Code importer and
//! by error messages that suggest internal names when users paste VS Code
//! command names into `bindings.json`.

use super::EditorAction;

/// Maps a VS Code command name to the internal action, when supported.
pub fn action_for_vscode_command(command: &str) -> Option<EditorAction> {
    let action = match command {
        // Cursor movement
        "cursorDown" => EditorAction::CursorDown,
        "cursorUp" => EditorAction::CursorUp,
        "cursorLeft" => EditorAction::CursorLeft,
        "cursorRight" => EditorAction::CursorRight,
        "cursorWordLeft" => EditorAction::CursorWordLeft,
        "cursorWordRight" => EditorAction::CursorWordRight,
        "cursorLineStart" | "cursorHome" => EditorAction::CursorLineStart,
        "cursorLineEnd" | "cursorEnd" => EditorAction::CursorLineEnd,
        "cursorPageDown" => EditorAction::CursorPageDown,
        "cursorPageUp" => EditorAction::CursorPageUp,
        // Selection movement
        "cursorDownSelect" => EditorAction::SelectionDown,
        "cursorUpSelect" => EditorAction::SelectionUp,
        "cursorLeftSelect" => EditorAction::SelectionLeft,
        "cursorRightSelect" => EditorAction::SelectionRight,
        "cursorWordLeftSelect" => EditorAction::SelectionWordLeft,
        "cursorWordRightSelect" => EditorAction::SelectionWordRight,
        // Editing
        "editor.action.insertLineAfter" => EditorAction::EditInsertLineAfter,
        "editor.action.insertLineBefore" => EditorAction::EditInsertLineBefore,
        "editor.action.moveLinesUpAction" => EditorAction::EditMoveLinesUp,
        "editor.action.moveLinesDownAction" => EditorAction::EditMoveLinesDown,
        "editor.action.selectAll" => EditorAction::SelectionAll,
        "undo" => EditorAction::EditUndo,
        "redo" => EditorAction::EditRedo,
        // Search
        "actions.find" => EditorAction::SearchOpen,
        "editor.action.startFindReplaceAction" => EditorAction::ReplaceOpen,
        "editor.action.nextMatchFindAction" => EditorAction::SearchNext,
        "editor.action.previousMatchFindAction" => EditorAction::SearchPrevious,
        // Buffers / Views
        "workbench.action.files.newUntitledFile" => EditorAction::BufferNew,
        "workbench.action.files.save" => EditorAction::FileSave,
        "workbench.action.splitEditor" => EditorAction::ViewSplitVertical,
        "workbench.action.focusNextGroup" => EditorAction::ViewFocusNextSplit,
        "workbench.action.focusPreviousGroup" => EditorAction::ViewFocusPreviousSplit,
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::action_for_vscode_command;
    use crate::keymap::EditorAction;

    #[test]
    fn maps_spec_0004_commands_and_rejects_unknown() {
        assert_eq!(
            action_for_vscode_command("cursorDown"),
            Some(EditorAction::CursorDown)
        );
        assert_eq!(
            action_for_vscode_command("editor.action.selectAll"),
            Some(EditorAction::SelectionAll)
        );
        assert_eq!(action_for_vscode_command("editor.action.rename"), None);
    }
}

//! Generic single-line input overlay (currently used only by Save As).
//!
//! Follows the same "visible ⇒ consume editing keys directly" pattern as
//! `search_overlay`: while open, text-editing keys are handled here instead
//! of going through the editor keymap. Unlike the search overlay, this
//! overlay is fully modal — every key while visible is swallowed (at worst
//! reported as [`PromptOutcome::Continue`]) rather than falling through to
//! the resolver, since a stray keystroke landing in the editor mid-filename
//! entry would be surprising.

use crate::{
    input::{Key, KeyEvent, Modifiers},
    ui::{Screen, Style},
};

/// What the current prompt session will do with its submitted input. Only
/// `SaveAs` exists today; the enum exists so future prompt use sites reuse
/// this overlay instead of growing a new one (TASK-260712 Gate 1).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PromptPurpose {
    SaveAs,
}

/// Result of feeding one key to the overlay.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PromptOutcome {
    /// Key was consumed (input edited, or ignored); overlay stays open.
    Continue,
    /// Enter: caller should act on `input` for the current `purpose`.
    Submit,
    /// Esc: caller should close the overlay and discard the input.
    Cancel,
}

#[derive(Debug, Clone, Default)]
pub struct PromptOverlay {
    pub visible: bool,
    pub purpose: Option<PromptPurpose>,
    pub label: String,
    pub input: String,
}

impl PromptOverlay {
    /// Opens the prompt with `initial` pre-filled (e.g. the document's
    /// current path for Save As).
    pub fn open(
        &mut self,
        purpose: PromptPurpose,
        label: impl Into<String>,
        initial: impl Into<String>,
    ) {
        self.visible = true;
        self.purpose = Some(purpose);
        self.label = label.into();
        self.input = initial.into();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.purpose = None;
        self.label.clear();
        self.input.clear();
    }

    /// Appends bracketed-paste text. The caller (event_loop's
    /// `handle_paste_input`) strips newlines first, matching the
    /// palette/search overlays.
    pub fn paste_text(&mut self, text: &str) {
        self.input.push_str(text);
    }

    pub fn handle_key(&mut self, event: &KeyEvent) -> PromptOutcome {
        match &event.key {
            Key::Esc if event.modifiers == Modifiers::none() => PromptOutcome::Cancel,
            Key::Enter if event.modifiers == Modifiers::none() => PromptOutcome::Submit,
            Key::Backspace if event.modifiers == Modifiers::none() => {
                self.input.pop();
                PromptOutcome::Continue
            }
            Key::Char(character)
                if !event.modifiers.contains_ctrl()
                    && !event.modifiers.contains_alt()
                    && !event.modifiers.contains_super() =>
            {
                self.input.push(*character);
                PromptOutcome::Continue
            }
            _ => PromptOutcome::Continue,
        }
    }
}

pub fn draw_prompt_overlay(screen: &mut Screen, overlay: &PromptOverlay) {
    if !overlay.visible || screen.width() == 0 || screen.height() == 0 {
        return;
    }

    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();
    let width = usize::from(screen.width());

    let line = format!("{} {}", overlay.label, overlay.input);
    let clipped = line.chars().take(width).collect::<String>();
    let padded = format!("{clipped:<width$}");
    screen.put_str(0, 0, &padded, dim);
    // Redraw the label in normal style over the dim background fill, same
    // trick as `search_overlay::draw_search_overlay`.
    screen.put_str(0, 0, &overlay.label, normal);

    let cursor_x = overlay.label.chars().count() + 1 + overlay.input.chars().count();
    screen.set_cursor(cursor_x.min(width.saturating_sub(1)) as u16, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(inner: Key) -> KeyEvent {
        KeyEvent::plain(inner)
    }

    #[test]
    fn open_prefills_input_and_sets_purpose_and_label() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "/tmp/existing.txt");

        assert!(prompt.visible);
        assert_eq!(prompt.purpose, Some(PromptPurpose::SaveAs));
        assert_eq!(prompt.label, "Save As:");
        assert_eq!(prompt.input, "/tmp/existing.txt");
    }

    #[test]
    fn typing_and_backspace_edit_the_input_and_stay_open() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "");

        assert_eq!(
            prompt.handle_key(&key(Key::Char('a'))),
            PromptOutcome::Continue
        );
        assert_eq!(
            prompt.handle_key(&key(Key::Char('b'))),
            PromptOutcome::Continue
        );
        assert_eq!(prompt.input, "ab");

        assert_eq!(
            prompt.handle_key(&key(Key::Backspace)),
            PromptOutcome::Continue
        );
        assert_eq!(prompt.input, "a");
        assert!(prompt.visible, "backspace must not close the overlay");
    }

    #[test]
    fn esc_reports_cancel_without_mutating_input() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "draft.txt");

        assert_eq!(prompt.handle_key(&key(Key::Esc)), PromptOutcome::Cancel);
        assert_eq!(
            prompt.input, "draft.txt",
            "the overlay itself does not clear input on Cancel; the caller closes it"
        );
    }

    #[test]
    fn enter_reports_submit_without_mutating_input() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "draft.txt");

        assert_eq!(prompt.handle_key(&key(Key::Enter)), PromptOutcome::Submit);
        assert_eq!(prompt.input, "draft.txt");
    }

    #[test]
    fn close_clears_all_state() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "draft.txt");
        prompt.close();

        assert!(!prompt.visible);
        assert_eq!(prompt.purpose, None);
        assert!(prompt.input.is_empty());
        assert!(prompt.label.is_empty());
    }

    #[test]
    fn paste_text_appends_to_input() {
        let mut prompt = PromptOverlay::default();
        prompt.open(PromptPurpose::SaveAs, "Save As:", "a");
        prompt.paste_text("bc");
        assert_eq!(prompt.input, "abc");
    }
}

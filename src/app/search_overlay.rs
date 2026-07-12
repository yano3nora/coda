//! Search/replace overlay state and direct key handling.
//!
//! The overlay follows the command-palette pattern: while visible, text editing
//! keys are consumed directly here instead of going through the editor keymap.

use crate::{
    core::{editor::EditorCore, position::Position, search},
    input::{Key, KeyEvent, Modifiers},
    ui::{Screen, Style},
};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum SearchFocus {
    #[default]
    Search,
    Replace,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOverlay {
    pub visible: bool,
    pub query: String,
    pub replace_text: String,
    pub case_sensitive: bool,
    pub replace_mode: bool,
    pub focus: SearchFocus,
    pub current: Option<usize>,
    matches: Vec<(Position, Position)>,
}

impl SearchOverlay {
    pub fn open(&mut self, replace_mode: bool, editor: &mut EditorCore) {
        self.visible = true;
        self.replace_mode = replace_mode;
        self.focus = SearchFocus::Search;
        self.refresh_from_cursor(editor);
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn context_flags(&self) -> (bool, bool) {
        (self.visible, self.visible && self.replace_mode)
    }

    pub fn next(&mut self, editor: &mut EditorCore) {
        self.move_current(editor, 1);
    }

    pub fn previous(&mut self, editor: &mut EditorCore) {
        self.move_current(editor, -1);
    }

    pub fn next_from_cursor(&mut self, editor: &mut EditorCore) {
        self.matches = search::find_matches(&editor.buffer, &self.query, self.case_sensitive);
        self.current = search::next_match_from(&self.matches, editor.cursor);
        self.select_current(editor);
    }

    pub fn previous_from_cursor(&mut self, editor: &mut EditorCore) {
        self.matches = search::find_matches(&editor.buffer, &self.query, self.case_sensitive);
        self.current = search::previous_match_from(&self.matches, editor.cursor);
        self.select_current(editor);
    }

    pub fn replace_current(&mut self, editor: &mut EditorCore) {
        let Some(index) = self.current else {
            return;
        };
        let Some((start, end)) = self.matches.get(index).copied() else {
            return;
        };
        let replacement = self.replace_text.clone();
        editor.replace_ranges(&[(start, end, replacement.as_str())]);
        self.refresh_from_cursor(editor);
    }

    pub fn replace_all(&mut self, editor: &mut EditorCore) {
        if self.matches.is_empty() {
            return;
        }
        let replacement = self.replace_text.clone();
        let replacements = self
            .matches
            .iter()
            .map(|(start, end)| (*start, *end, replacement.as_str()))
            .collect::<Vec<_>>();
        editor.replace_ranges(&replacements);
        self.refresh_from_cursor(editor);
    }

    pub fn paste_text(&mut self, text: &str, editor: &mut EditorCore) {
        match self.focus {
            SearchFocus::Search => {
                self.query.push_str(text);
                self.refresh_from_cursor(editor);
            }
            SearchFocus::Replace => self.replace_text.push_str(text),
        }
    }

    pub fn handle_key(&mut self, event: &KeyEvent, editor: &mut EditorCore) -> bool {
        match &event.key {
            Key::Esc if event.modifiers == Modifiers::none() => {
                self.close();
                true
            }
            Key::Tab if event.modifiers == Modifiers::none() && self.replace_mode => {
                self.focus = match self.focus {
                    SearchFocus::Search => SearchFocus::Replace,
                    SearchFocus::Replace => SearchFocus::Search,
                };
                true
            }
            Key::Backspace if event.modifiers == Modifiers::none() => {
                match self.focus {
                    SearchFocus::Search => {
                        self.query.pop();
                        self.refresh_from_cursor(editor);
                    }
                    SearchFocus::Replace => {
                        self.replace_text.pop();
                    }
                }
                true
            }
            Key::Enter if event.modifiers == Modifiers::shift() => {
                self.previous(editor);
                true
            }
            Key::Enter if event.modifiers == Modifiers::ctrl().with_alt() => {
                self.replace_all(editor);
                true
            }
            Key::Enter
                if event.modifiers == Modifiers::ctrl()
                    || event.modifiers == Modifiers::super_key() =>
            {
                self.replace_current(editor);
                true
            }
            Key::Enter if event.modifiers == Modifiers::none() => {
                self.next(editor);
                true
            }
            Key::Char('c') if event.modifiers == Modifiers::alt() => {
                self.case_sensitive = !self.case_sensitive;
                self.refresh_from_cursor(editor);
                true
            }
            Key::Char(character)
                if !event.modifiers.contains_ctrl()
                    && !event.modifiers.contains_alt()
                    && !event.modifiers.contains_super() =>
            {
                match self.focus {
                    SearchFocus::Search => {
                        self.query.push(*character);
                        self.refresh_from_cursor(editor);
                    }
                    SearchFocus::Replace => self.replace_text.push(*character),
                }
                true
            }
            _ => false,
        }
    }

    fn refresh_from_cursor(&mut self, editor: &mut EditorCore) {
        self.matches = search::find_matches(&editor.buffer, &self.query, self.case_sensitive);
        self.current = search::next_match_from(&self.matches, editor.cursor);
        self.select_current(editor);
    }

    fn move_current(&mut self, editor: &mut EditorCore, delta: isize) {
        if self.matches.is_empty() {
            self.current = None;
            return;
        }
        let current = self.current.unwrap_or(0) as isize;
        self.current = Some((current + delta).rem_euclid(self.matches.len() as isize) as usize);
        self.select_current(editor);
    }

    fn select_current(&self, editor: &mut EditorCore) {
        if let Some(index) = self.current
            && let Some((start, end)) = self.matches.get(index).copied()
        {
            editor.select_range(start, end);
        }
    }
}

pub fn draw_search_overlay(screen: &mut Screen, overlay: &SearchOverlay) {
    if !overlay.visible || screen.width() == 0 || screen.height() == 0 {
        return;
    }

    let normal = Style::default();
    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let width = usize::from(screen.width());
    let total = overlay.match_count();
    let count = overlay
        .current
        .map(|index| format!("{}/{}", index + 1, total))
        .unwrap_or_else(|| "no matches".to_string());
    let case = if overlay.case_sensitive { "on" } else { "off" };
    let first = format!("Find: {} [Aa:{case}] {count}", overlay.query);
    draw_bar_line(screen, 0, &first, width, dim);

    if overlay.replace_mode && screen.height() > 1 {
        let second = format!("Replace: {}", overlay.replace_text);
        draw_bar_line(screen, 1, &second, width, dim);
    }

    let (cursor_x, cursor_y) = match overlay.focus {
        SearchFocus::Search => ("Find: ".chars().count() + overlay.query.chars().count(), 0),
        SearchFocus::Replace => (
            "Replace: ".chars().count() + overlay.replace_text.chars().count(),
            1,
        ),
    };
    if cursor_y < usize::from(screen.height()) {
        screen.set_cursor(
            cursor_x.min(width.saturating_sub(1)) as u16,
            cursor_y as u16,
        );
    }

    // Put the focused prompt in normal style after the dim background fill so
    // focus is visible without introducing another UI state machine.
    match overlay.focus {
        SearchFocus::Search => screen.put_str(0, 0, "Find:", normal),
        SearchFocus::Replace if overlay.replace_mode && screen.height() > 1 => {
            screen.put_str(0, 1, "Replace:", normal)
        }
        SearchFocus::Replace => 0,
    };
}

fn draw_bar_line(screen: &mut Screen, y: u16, text: &str, width: usize, style: Style) {
    if y >= screen.height() {
        return;
    }
    let clipped = text.chars().take(width).collect::<String>();
    let padded = format!("{clipped:<width$}");
    screen.put_str(0, y, &padded, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::TextBuffer;

    fn editor(text: &str) -> EditorCore {
        EditorCore::new(TextBuffer::from_bytes(text.as_bytes()).unwrap().0)
    }

    #[test]
    fn overlay_input_backspace_tab_and_case_toggle_update_state() {
        let mut editor = editor("Foo foo");
        let mut overlay = SearchOverlay::default();
        overlay.open(true, &mut editor);

        assert!(overlay.handle_key(&KeyEvent::plain(Key::Char('f')), &mut editor));
        assert!(overlay.handle_key(&KeyEvent::plain(Key::Char('o')), &mut editor));
        assert_eq!(overlay.query, "fo");
        assert_eq!(overlay.match_count(), 2);

        assert!(overlay.handle_key(&KeyEvent::plain(Key::Tab), &mut editor));
        assert_eq!(overlay.focus, SearchFocus::Replace);
        assert!(overlay.handle_key(&KeyEvent::plain(Key::Char('x')), &mut editor));
        assert_eq!(overlay.replace_text, "x");
        assert!(overlay.handle_key(&KeyEvent::plain(Key::Backspace), &mut editor));
        assert!(overlay.replace_text.is_empty());

        assert!(overlay.handle_key(
            &KeyEvent::new(Key::Char('c'), Modifiers::alt()),
            &mut editor
        ));
        assert!(overlay.case_sensitive);
        assert_eq!(overlay.match_count(), 1);
    }

    #[test]
    fn query_change_selects_first_match_at_or_after_cursor() {
        let mut editor = editor("foo bar foo");
        editor.cursor = Position::new(0, 4);
        let mut overlay = SearchOverlay::default();
        overlay.open(false, &mut editor);
        for character in "foo".chars() {
            overlay.handle_key(&KeyEvent::plain(Key::Char(character)), &mut editor);
        }
        assert_eq!(overlay.current, Some(1));
        assert_eq!(
            editor.selection.unwrap().range(),
            (Position::new(0, 8), Position::new(0, 11))
        );
    }

    #[test]
    fn context_flags_reflect_visibility_and_replace_mode() {
        let mut editor = editor("abc");
        let mut overlay = SearchOverlay::default();
        assert_eq!(overlay.context_flags(), (false, false));
        overlay.open(true, &mut editor);
        assert_eq!(overlay.context_flags(), (true, true));
        overlay.close();
        assert_eq!(overlay.context_flags(), (false, false));
    }
}

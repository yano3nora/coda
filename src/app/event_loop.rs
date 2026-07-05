//! Terminal event loop integrating input, resolver, editor core, and renderer.

use std::{
    io::{self, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use libc::{POLLIN, STDIN_FILENO, pollfd};

use crate::{
    core::editor::{EditorCore, Motion},
    highlight::{HighlightCache, HighlightEngine, ThemeChoice},
    input::{
        BracketedPasteGuard, InputEvent, Key, KeyEvent, KeyboardProtocolGuard, Modifiers,
        RawModeGuard, drain_input_events, flush_pending_escape,
    },
    keymap::{EditorAction, EditorContext, ResolveResult, Resolver},
    ui::{AltScreenGuard, Screen, render_diff, render_full, take_pending_resize, terminal_size},
};

use super::{
    clipboard, default_bindings,
    editor_view::{EditorView, StatusLine},
    file,
    palette::{CommandPalette, filter_actions},
    search_overlay::{SearchOverlay, draw_search_overlay},
};

const SEQUENCE_TIMEOUT: Duration = Duration::from_millis(800);
const IDLE_POLL_MS: i32 = 100;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum QuitDecision {
    Continue,
    Quit,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct QuitGuard {
    warned: bool,
}

impl QuitGuard {
    pub fn request_quit(&mut self, modified: bool) -> QuitDecision {
        if !modified || self.warned {
            QuitDecision::Quit
        } else {
            self.warned = true;
            QuitDecision::Continue
        }
    }

    fn reset(&mut self) {
        self.warned = false;
    }
}

pub struct EventLoop {
    path: PathBuf,
    editor: EditorCore,
    resolver: Resolver,
    view: EditorView,
    highlight_engine: HighlightEngine,
    highlight_cache: HighlightCache,
    palette: CommandPalette,
    search: SearchOverlay,
    saved_snapshot: Vec<u8>,
    message: String,
    clipboard: String,
    pending_terminal_write: Vec<u8>,
    pending_keys: Vec<KeyEvent>,
    pending_since: Option<Instant>,
    quit_guard: QuitGuard,
}

impl EventLoop {
    pub fn open(
        path: PathBuf,
        mut warnings: Vec<String>,
        user_bindings: Vec<crate::keymap::Binding>,
        theme: ThemeChoice,
    ) -> Result<Self, file::LoadError> {
        let (buffer, load_info) = file::open(&path)?;
        if load_info.is_new {
            warnings.push("new file".to_string());
        }
        if load_info.mixed_line_endings {
            warnings.push("mixed line endings".to_string());
        }
        let saved_snapshot = buffer.to_bytes();
        let mut bindings = default_bindings::bindings();
        bindings.extend(user_bindings);
        Ok(Self {
            path,
            editor: EditorCore::new(buffer),
            resolver: Resolver::new(bindings),
            view: EditorView::default(),
            highlight_engine: HighlightEngine::new(theme),
            highlight_cache: HighlightCache::default(),
            palette: CommandPalette::default(),
            search: SearchOverlay::default(),
            saved_snapshot,
            message: warnings.join("; "),
            clipboard: String::new(),
            pending_terminal_write: Vec::new(),
            pending_keys: Vec::new(),
            pending_since: None,
            quit_guard: QuitGuard::default(),
        })
    }

    pub fn run(mut self) -> io::Result<()> {
        let _raw = RawModeGuard::enable_stdin()?;
        let stdout = io::stdout().lock();
        // Alternate screen FIRST, keyboard protocol SECOND: kitty tracks the
        // keyboard mode stack separately for the main and alternate screens,
        // so pushing before entering the alt screen leaves the editor screen
        // in legacy mode (Ctrl+J arrives as 0x0a = Enter). Drop order (reverse
        // declaration) pops the protocol while still on the alt screen.
        let mut alt = AltScreenGuard::enter(stdout)?;
        let _keyboard = KeyboardProtocolGuard::push(alt.writer_mut())?;
        let _paste = BracketedPasteGuard::enable(alt.writer_mut())?;
        let mut stdin = io::stdin().lock();
        let mut byte_buffer = Vec::new();
        let (width, height) = terminal_size().unwrap_or((80, 24));
        let mut prev = Screen::new(width, height);
        let mut first_render = true;

        loop {
            let mut next = Screen::new(prev.width(), prev.height());
            self.draw(&mut next);
            if first_render {
                render_full(&next, alt.writer_mut())?;
                first_render = false;
            } else {
                render_diff(&prev, &next, alt.writer_mut())?;
            }
            alt.writer_mut().flush()?;
            prev = next;

            let timeout = self.poll_timeout_ms();
            if poll_stdin(STDIN_FILENO, timeout)? {
                let mut chunk = [0_u8; 256];
                let read = stdin.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                byte_buffer.extend_from_slice(&chunk[..read]);
                for event in drain_input_events(&mut byte_buffer) {
                    if self.handle_input_event(event) == QuitDecision::Quit {
                        return Ok(());
                    }
                }
                self.flush_terminal_writes(alt.writer_mut())?;
            } else if let Some(event) = flush_pending_escape(&mut byte_buffer) {
                if self.handle_key(event) == QuitDecision::Quit {
                    return Ok(());
                }
                self.flush_terminal_writes(alt.writer_mut())?;
            }

            if self.handle_sequence_timeout() == QuitDecision::Quit {
                return Ok(());
            }

            if take_pending_resize()
                && let Some((width, height)) = terminal_size()
            {
                prev.resize(width, height);
                first_render = true;
            }
        }
        Ok(())
    }

    fn draw(&mut self, screen: &mut Screen) {
        let pending = self.pending_label();
        let filename = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("[No Name]")
            .to_string();
        let editor_rows = screen.height().saturating_sub(1) as usize;
        let highlights = self.highlight_cache.spans_for(
            &self.editor.buffer,
            self.view.top_line..self.view.top_line + editor_rows,
            &self.highlight_engine,
            self.highlight_engine.syntax_for_path(&self.path),
        );
        self.view.draw(
            &self.editor,
            screen,
            &highlights,
            StatusLine {
                filename: &filename,
                modified: self.is_modified(),
                message: &self.message,
                pending: &pending,
            },
        );
        let items = filter_actions(&self.palette.query, self.resolver.bindings());
        draw_search_overlay(screen, &self.search);
        super::palette::draw_palette(screen, &self.palette, &items);
    }

    fn handle_input_event(&mut self, event: InputEvent) -> QuitDecision {
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Paste(text) => self.handle_paste_input(&text),
        }
    }

    fn handle_paste_input(&mut self, text: &str) -> QuitDecision {
        let sanitized = text.replace('\n', "");
        if self.palette.visible {
            self.palette.push_text(&sanitized);
        } else if self.search.visible {
            self.search.paste_text(&sanitized, &mut self.editor);
        } else {
            self.editor.insert_text(text);
            self.editor.commit_group();
            self.quit_guard.reset();
        }
        QuitDecision::Continue
    }

    fn flush_terminal_writes(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if self.pending_terminal_write.is_empty() {
            return Ok(());
        }
        writer.write_all(&self.pending_terminal_write)?;
        writer.flush()?;
        self.pending_terminal_write.clear();
        Ok(())
    }

    fn handle_key(&mut self, event: KeyEvent) -> QuitDecision {
        if event == KeyEvent::plain(Key::F(1)) {
            self.toggle_palette();
            return QuitDecision::Continue;
        }

        if self.palette.visible
            && let Some(decision) = self.handle_palette_key(&event)
        {
            return decision;
        }

        if self.search.visible && self.search.handle_key(&event, &mut self.editor) {
            return QuitDecision::Continue;
        }

        self.pending_keys.push(event.clone());
        let was_sequence_retry = self.pending_keys.len() > 1;
        match self.resolver.resolve(&self.pending_keys, &self.context()) {
            ResolveResult::Matched(action) => {
                self.clear_pending();
                self.dispatch(action)
            }
            ResolveResult::Pending { candidates, .. } => {
                self.pending_since = Some(Instant::now());
                self.message = format!("pending: {}", format_candidates(&candidates));
                QuitDecision::Continue
            }
            ResolveResult::NoMatch => {
                self.pending_keys.clear();
                self.pending_since = None;
                if was_sequence_retry {
                    self.handle_key(event)
                } else {
                    self.handle_text_input(event)
                }
            }
        }
    }

    /// Returns `Some(decision)` when the palette consumed the key. Actions
    /// executed from the palette (e.g. app.quit) must propagate their quit
    /// decision to the run loop, so this cannot collapse to a bool.
    fn handle_palette_key(&mut self, event: &KeyEvent) -> Option<QuitDecision> {
        match &event.key {
            Key::Esc if event.modifiers == Modifiers::none() => {
                self.palette.close();
                Some(QuitDecision::Continue)
            }
            Key::Up if event.modifiers == Modifiers::none() => {
                let count = filter_actions(&self.palette.query, self.resolver.bindings()).len();
                self.palette.move_selection(-1, count);
                Some(QuitDecision::Continue)
            }
            Key::Down if event.modifiers == Modifiers::none() => {
                let count = filter_actions(&self.palette.query, self.resolver.bindings()).len();
                self.palette.move_selection(1, count);
                Some(QuitDecision::Continue)
            }
            Key::Enter if event.modifiers == Modifiers::none() => {
                let items = filter_actions(&self.palette.query, self.resolver.bindings());
                if let Some(action) = self.palette.selected_action(&items) {
                    self.palette.close();
                    return Some(self.dispatch(action));
                }
                Some(QuitDecision::Continue)
            }
            Key::Backspace if event.modifiers == Modifiers::none() => {
                self.palette.backspace();
                Some(QuitDecision::Continue)
            }
            Key::Char(character)
                if !event.modifiers.contains_ctrl()
                    && !event.modifiers.contains_alt()
                    && !event.modifiers.contains_super() =>
            {
                self.palette.push_char(*character);
                Some(QuitDecision::Continue)
            }
            _ => None,
        }
    }

    fn handle_text_input(&mut self, event: KeyEvent) -> QuitDecision {
        if !self.context().text_input_focus {
            return QuitDecision::Continue;
        }
        match event.key {
            Key::Char(character)
                if !event.modifiers.contains_ctrl()
                    && !event.modifiers.contains_alt()
                    && !event.modifiers.contains_super() =>
            {
                self.editor.insert_text(&character.to_string());
                self.quit_guard.reset();
            }
            Key::Enter if event.modifiers == Modifiers::none() => self.editor.insert_text("\n"),
            Key::Tab if event.modifiers == Modifiers::none() => self.editor.insert_text("\t"),
            _ => {}
        }
        QuitDecision::Continue
    }

    fn dispatch(&mut self, action: EditorAction) -> QuitDecision {
        self.message.clear();
        match action {
            EditorAction::CursorUp => self.editor.move_cursor(Motion::Up, false),
            EditorAction::CursorDown => self.editor.move_cursor(Motion::Down, false),
            EditorAction::CursorLeft => self.editor.move_cursor(Motion::Left, false),
            EditorAction::CursorRight => self.editor.move_cursor(Motion::Right, false),
            EditorAction::CursorWordLeft => self.editor.move_cursor(Motion::WordLeft, false),
            EditorAction::CursorWordRight => self.editor.move_cursor(Motion::WordRight, false),
            EditorAction::CursorLineStart => self.editor.move_cursor(Motion::LineStart, false),
            EditorAction::CursorLineEnd => self.editor.move_cursor(Motion::LineEnd, false),
            EditorAction::CursorBufferStart => self.editor.move_cursor(Motion::BufferStart, false),
            EditorAction::CursorBufferEnd => self.editor.move_cursor(Motion::BufferEnd, false),
            EditorAction::CursorPageUp => {
                self.editor.move_cursor(Motion::PageUp { rows: 10 }, false)
            }
            EditorAction::CursorPageDown => self
                .editor
                .move_cursor(Motion::PageDown { rows: 10 }, false),
            EditorAction::SelectionUp => self.editor.move_cursor(Motion::Up, true),
            EditorAction::SelectionDown => self.editor.move_cursor(Motion::Down, true),
            EditorAction::SelectionLeft => self.editor.move_cursor(Motion::Left, true),
            EditorAction::SelectionRight => self.editor.move_cursor(Motion::Right, true),
            EditorAction::SelectionWordLeft => self.editor.move_cursor(Motion::WordLeft, true),
            EditorAction::SelectionWordRight => self.editor.move_cursor(Motion::WordRight, true),
            EditorAction::SelectionLineStart => self.editor.move_cursor(Motion::LineStart, true),
            EditorAction::SelectionLineEnd => self.editor.move_cursor(Motion::LineEnd, true),
            EditorAction::SelectionBufferStart => {
                self.editor.move_cursor(Motion::BufferStart, true)
            }
            EditorAction::SelectionBufferEnd => self.editor.move_cursor(Motion::BufferEnd, true),
            EditorAction::SelectionPageUp => {
                self.editor.move_cursor(Motion::PageUp { rows: 10 }, true)
            }
            EditorAction::SelectionPageDown => {
                self.editor.move_cursor(Motion::PageDown { rows: 10 }, true)
            }
            EditorAction::SelectionAll => self.editor.select_all(),
            EditorAction::EditBackspace => self.editor.backspace(),
            EditorAction::EditDelete => self.editor.delete_forward(),
            EditorAction::EditDeleteWordLeft => self.editor.delete_word_left(),
            EditorAction::EditDeleteToLineStart => self.editor.delete_to_line_start(),
            EditorAction::EditInsertLineAfter => self.editor.insert_line_after(),
            EditorAction::EditInsertLineBefore => self.editor.insert_line_before(),
            EditorAction::EditMoveLinesUp => self.editor.move_lines_up(),
            EditorAction::EditMoveLinesDown => self.editor.move_lines_down(),
            EditorAction::EditCopy => {
                if let Some(text) = self.editor.copy_text() {
                    self.copy_to_clipboards(text, "copied");
                }
            }
            EditorAction::EditCut => {
                if let Some(text) = self.editor.cut() {
                    self.copy_to_clipboards(text, "cut");
                    self.quit_guard.reset();
                }
            }
            EditorAction::EditPaste => {
                if !self.clipboard.is_empty() {
                    let text = self.clipboard.clone();
                    self.editor.insert_text(&text);
                    self.editor.commit_group();
                    self.quit_guard.reset();
                }
            }
            EditorAction::EditUndo => {
                if !self.editor.undo() {
                    self.message = "nothing to undo".to_string();
                }
            }
            EditorAction::EditRedo => {
                if !self.editor.redo() {
                    self.message = "nothing to redo".to_string();
                }
            }
            EditorAction::FileSave => match file::save(&self.path, &self.editor.buffer) {
                Ok(()) => {
                    self.saved_snapshot = self.editor.buffer.to_bytes();
                    self.message = "saved".to_string();
                    self.quit_guard.reset();
                }
                Err(error) => self.message = format!("save failed: {error}"),
            },
            EditorAction::PaletteOpen => {
                self.search.close();
                self.palette.open();
            }
            EditorAction::SearchOpen => {
                self.palette.close();
                self.search.open(false, &mut self.editor);
            }
            EditorAction::ReplaceOpen => {
                self.palette.close();
                self.search.open(true, &mut self.editor);
            }
            EditorAction::SearchNext => {
                if self.search.query.is_empty() {
                    self.search.open(false, &mut self.editor);
                } else if self.search.visible {
                    self.search.next(&mut self.editor);
                } else {
                    self.search.next_from_cursor(&mut self.editor);
                }
            }
            EditorAction::SearchPrevious => {
                if self.search.query.is_empty() {
                    self.search.open(false, &mut self.editor);
                } else if self.search.visible {
                    self.search.previous(&mut self.editor);
                } else {
                    self.search.previous_from_cursor(&mut self.editor);
                }
            }
            EditorAction::ReplaceNext => {
                if self.search.query.is_empty() {
                    self.search.open(true, &mut self.editor);
                } else {
                    if !self.search.visible {
                        self.search.next_from_cursor(&mut self.editor);
                    }
                    self.search.replace_current(&mut self.editor);
                }
            }
            EditorAction::ReplaceAll => {
                if self.search.query.is_empty() {
                    self.search.open(true, &mut self.editor);
                } else {
                    if !self.search.visible {
                        self.search.next_from_cursor(&mut self.editor);
                    }
                    self.search.replace_all(&mut self.editor);
                }
            }
            EditorAction::AppQuit => match self.quit_guard.request_quit(self.is_modified()) {
                QuitDecision::Quit => return QuitDecision::Quit,
                QuitDecision::Continue => {
                    self.message = "unsaved changes; press quit again to exit".to_string();
                }
            },
            other => self.message = format!("{other}: not implemented yet"),
        }
        QuitDecision::Continue
    }

    fn copy_to_clipboards(&mut self, text: String, status: &str) {
        self.clipboard = text;
        if let Some(sequence) = clipboard::osc52_copy_sequence(&self.clipboard) {
            self.pending_terminal_write.extend(sequence);
            self.message = status.to_string();
        } else {
            self.message = format!("{status}; OSC 52 skipped (>1MB)");
        }
    }

    fn handle_sequence_timeout(&mut self) -> QuitDecision {
        let Some(started) = self.pending_since else {
            return QuitDecision::Continue;
        };
        if started.elapsed() < SEQUENCE_TIMEOUT {
            return QuitDecision::Continue;
        }
        let result = self.resolver.resolve(&self.pending_keys, &self.context());
        self.clear_pending();
        match result {
            ResolveResult::Pending {
                exact: Some(action),
                ..
            } => self.dispatch(action),
            _ => QuitDecision::Continue,
        }
    }

    fn context(&self) -> EditorContext {
        EditorContext {
            editor_focus: !self.palette.visible && !self.search.visible,
            text_input_focus: !self.palette.visible && !self.search.visible,
            has_selection: self
                .editor
                .selection
                .is_some_and(|selection| !selection.is_empty()),
            search_visible: self.search.context_flags().0,
            replace_visible: self.search.context_flags().1,
            command_palette_visible: self.palette.visible,
            list_focus: self.palette.visible,
            ..EditorContext::default()
        }
    }

    fn toggle_palette(&mut self) {
        if self.palette.visible {
            self.palette.close();
        } else {
            self.search.close();
            self.palette.open();
        }
    }

    fn is_modified(&self) -> bool {
        self.editor.buffer.to_bytes() != self.saved_snapshot
    }

    fn clear_pending(&mut self) {
        self.pending_keys.clear();
        self.pending_since = None;
    }

    fn pending_label(&self) -> String {
        self.pending_keys
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn poll_timeout_ms(&self) -> i32 {
        if let Some(started) = self.pending_since {
            let elapsed = started.elapsed();
            if elapsed >= SEQUENCE_TIMEOUT {
                0
            } else {
                (SEQUENCE_TIMEOUT - elapsed)
                    .as_millis()
                    .min(i32::MAX as u128) as i32
            }
        } else {
            IDLE_POLL_MS
        }
    }
}

fn poll_stdin(fd: i32, timeout_ms: i32) -> io::Result<bool> {
    let mut fds = [pollfd {
        fd,
        events: POLLIN,
        revents: 0,
    }];
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result > 0 && fds[0].revents & POLLIN != 0)
    }
}

fn format_candidates(candidates: &[(Vec<KeyEvent>, EditorAction)]) -> String {
    candidates
        .iter()
        .map(|(keys, action)| {
            format!(
                "{} -> {action}",
                keys.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{EventLoop, QuitDecision, QuitGuard};
    use crate::{
        highlight::ThemeChoice,
        input::{InputEvent, Key, KeyEvent},
        keymap::EditorAction,
    };

    fn buffer_text(event_loop: &EventLoop) -> String {
        String::from_utf8(event_loop.editor.buffer.to_bytes()).unwrap()
    }

    #[test]
    fn palette_app_quit_propagates_quit_decision_when_clean() {
        let path = std::env::temp_dir().join("coda-test-palette-quit.txt");
        std::fs::write(&path, b"abc\n").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert_eq!(
            event_loop.handle_key(KeyEvent::plain(Key::F(1))),
            QuitDecision::Continue,
            "F1 opens the palette"
        );
        for character in "quit".chars() {
            event_loop.handle_key(KeyEvent::plain(Key::Char(character)));
        }
        assert_eq!(
            event_loop.handle_key(KeyEvent::plain(Key::Enter)),
            QuitDecision::Quit,
            "palette app.quit on a clean buffer must quit"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bracketed_paste_input_inserts_text_without_key_resolution() {
        let path = std::env::temp_dir().join("coda-test-paste-input.txt");
        std::fs::write(&path, b"").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert_eq!(
            event_loop.handle_input_event(InputEvent::Paste("a\x1b[Ab".to_string())),
            QuitDecision::Continue
        );

        assert_eq!(buffer_text(&event_loop), "a\x1b[Ab");
        assert!(event_loop.editor.undo(), "whole paste is one undo group");
        assert_eq!(buffer_text(&event_loop), "");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn paste_into_overlays_strips_newlines() {
        let path = std::env::temp_dir().join("coda-test-paste-overlay.txt");
        std::fs::write(&path, b"abc\n").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.handle_key(KeyEvent::plain(Key::F(1)));
        event_loop.handle_input_event(InputEvent::Paste("a\nb".to_string()));
        assert_eq!(event_loop.palette.query, "ab");

        event_loop.palette.close();
        event_loop.dispatch(EditorAction::SearchOpen);
        event_loop.handle_input_event(InputEvent::Paste("x\ny".to_string()));
        assert_eq!(event_loop.search.query, "xy");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn copy_cut_and_internal_paste_update_clipboard_and_osc52_queue() {
        let path = std::env::temp_dir().join("coda-test-clipboard-actions.txt");
        std::fs::write(&path, b"foo\nbar").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.dispatch(EditorAction::EditCopy);
        assert_eq!(event_loop.clipboard, "foo\n");
        assert_eq!(event_loop.pending_terminal_write, b"\x1b]52;c;Zm9vCg==\x07");
        assert_eq!(event_loop.message, "copied");
        event_loop.pending_terminal_write.clear();

        event_loop.dispatch(EditorAction::EditCut);
        assert_eq!(event_loop.clipboard, "foo\n");
        assert_eq!(buffer_text(&event_loop), "bar");
        assert_eq!(event_loop.message, "cut");

        event_loop.dispatch(EditorAction::EditPaste);
        assert_eq!(buffer_text(&event_loop), "foo\nbar");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn quit_guard_requires_second_quit_when_modified() {
        let mut guard = QuitGuard::default();
        assert_eq!(guard.request_quit(true), QuitDecision::Continue);
        assert_eq!(guard.request_quit(true), QuitDecision::Quit);
    }

    #[test]
    fn quit_guard_allows_immediate_quit_when_clean() {
        let mut guard = QuitGuard::default();
        assert_eq!(guard.request_quit(false), QuitDecision::Quit);
    }

    #[test]
    fn context_reflects_search_and_replace_overlay_focus() {
        let path = std::env::temp_dir().join("coda-test-search-context.txt");
        std::fs::write(&path, b"abc\n").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.dispatch(crate::keymap::EditorAction::ReplaceOpen);
        let context = event_loop.context();
        assert!(!context.editor_focus);
        assert!(!context.text_input_focus);
        assert!(context.search_visible);
        assert!(context.replace_visible);

        let _ = std::fs::remove_file(path);
    }
}

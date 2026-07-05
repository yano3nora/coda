//! Terminal event loop integrating input, resolver, editor core, and renderer.

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use libc::{POLLIN, STDIN_FILENO, pollfd};

use crate::{
    core::editor::{EditorCore, Motion},
    highlight::{HighlightEngine, ThemeChoice},
    input::{
        BracketedPasteGuard, InputEvent, Key, KeyEvent, KeyboardProtocolGuard, Modifiers,
        RawModeGuard, drain_input_events, flush_pending_escape,
    },
    keymap::{EditorAction, EditorContext, ResolveResult, Resolver},
    ui::{
        AltScreenGuard, Screen, Style, render_diff, render_full, take_pending_resize, terminal_size,
    },
};

use super::{
    clipboard, default_bindings,
    document::Document,
    editor_view::StatusLine,
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
    documents: Vec<Document>,
    active: usize,
    resolver: Resolver,
    highlight_engine: HighlightEngine,
    palette: CommandPalette,
    search: SearchOverlay,
    message: String,
    clipboard: String,
    pending_terminal_write: Vec<u8>,
    pending_keys: Vec<KeyEvent>,
    pending_since: Option<Instant>,
    quit_guard: QuitGuard,
    close_guard: QuitGuard,
}

impl EventLoop {
    #[cfg(test)]
    pub fn open(
        path: PathBuf,
        warnings: Vec<String>,
        user_bindings: Vec<crate::keymap::Binding>,
        theme: ThemeChoice,
    ) -> Result<Self, file::LoadError> {
        Self::open_many(vec![path], warnings, user_bindings, theme)
    }

    pub fn open_many(
        paths: Vec<PathBuf>,
        mut warnings: Vec<String>,
        user_bindings: Vec<crate::keymap::Binding>,
        theme: ThemeChoice,
    ) -> Result<Self, file::LoadError> {
        let mut documents = Vec::new();
        for path in dedupe_paths(paths) {
            let (document, load_info) = Document::open(path.clone())?;
            if load_info.is_new {
                warnings.push(format!("{}: new file", document.display_name()));
            }
            if load_info.mixed_line_endings {
                warnings.push(format!("{}: mixed line endings", document.display_name()));
            }
            documents.push(document);
        }
        if documents.is_empty() {
            documents.push(Document::unnamed());
        }
        let mut bindings = default_bindings::bindings();
        bindings.extend(user_bindings);
        Ok(Self {
            documents,
            active: 0,
            resolver: Resolver::new(bindings),
            highlight_engine: HighlightEngine::new(theme),
            palette: CommandPalette::default(),
            search: SearchOverlay::default(),
            message: warnings.join("; "),
            clipboard: String::new(),
            pending_terminal_write: Vec::new(),
            pending_keys: Vec::new(),
            pending_since: None,
            quit_guard: QuitGuard::default(),
            close_guard: QuitGuard::default(),
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
        draw_tab_bar(screen, &self.tab_items());

        let active = self.active;
        let filename = self.documents[active].display_name();
        let editor_rows = screen.height().saturating_sub(2) as usize;
        let path = self.documents[active].path.clone();
        let syntax = path
            .as_deref()
            .and_then(|path| self.highlight_engine.syntax_for_path(path));
        let document = &mut self.documents[active];
        let highlights = document.highlight_cache.spans_for(
            &document.editor.buffer,
            document.view.top_line..document.view.top_line + editor_rows,
            &self.highlight_engine,
            syntax,
        );
        let modified = document.is_modified();
        document.view.draw(
            &document.editor,
            screen,
            &highlights,
            StatusLine {
                filename: &filename,
                modified,
                message: &self.message,
                pending: &pending,
            },
            1,
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

    fn active_document(&self) -> &Document {
        &self.documents[self.active]
    }

    fn active_document_mut(&mut self) -> &mut Document {
        &mut self.documents[self.active]
    }

    fn editor(&self) -> &EditorCore {
        &self.active_document().editor
    }

    fn editor_mut(&mut self) -> &mut EditorCore {
        &mut self.active_document_mut().editor
    }

    fn handle_paste_input(&mut self, text: &str) -> QuitDecision {
        let sanitized = text.replace('\n', "");
        if self.palette.visible {
            self.palette.push_text(&sanitized);
        } else if self.search.visible {
            let editor = &mut self.documents[self.active].editor;
            self.search.paste_text(&sanitized, editor);
        } else {
            self.editor_mut().insert_text(text);
            self.editor_mut().commit_group();
            self.quit_guard.reset();
            self.close_guard.reset();
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

        if self.search.visible
            && self
                .search
                .handle_key(&event, &mut self.documents[self.active].editor)
        {
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
                self.editor_mut().insert_text(&character.to_string());
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            Key::Enter if event.modifiers == Modifiers::none() => {
                self.editor_mut().insert_text("\n");
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            Key::Tab if event.modifiers == Modifiers::none() => {
                self.editor_mut().insert_text("\t");
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            _ => {}
        }
        QuitDecision::Continue
    }

    fn dispatch(&mut self, action: EditorAction) -> QuitDecision {
        self.message.clear();
        match action {
            EditorAction::CursorUp => self.editor_mut().move_cursor(Motion::Up, false),
            EditorAction::CursorDown => self.editor_mut().move_cursor(Motion::Down, false),
            EditorAction::CursorLeft => self.editor_mut().move_cursor(Motion::Left, false),
            EditorAction::CursorRight => self.editor_mut().move_cursor(Motion::Right, false),
            EditorAction::CursorWordLeft => self.editor_mut().move_cursor(Motion::WordLeft, false),
            EditorAction::CursorWordRight => {
                self.editor_mut().move_cursor(Motion::WordRight, false)
            }
            EditorAction::CursorLineStart => {
                self.editor_mut().move_cursor(Motion::LineStart, false)
            }
            EditorAction::CursorLineEnd => self.editor_mut().move_cursor(Motion::LineEnd, false),
            EditorAction::CursorBufferStart => {
                self.editor_mut().move_cursor(Motion::BufferStart, false)
            }
            EditorAction::CursorBufferEnd => {
                self.editor_mut().move_cursor(Motion::BufferEnd, false)
            }
            EditorAction::CursorPageUp => self
                .editor_mut()
                .move_cursor(Motion::PageUp { rows: 10 }, false),
            EditorAction::CursorPageDown => self
                .editor_mut()
                .move_cursor(Motion::PageDown { rows: 10 }, false),
            EditorAction::SelectionUp => self.editor_mut().move_cursor(Motion::Up, true),
            EditorAction::SelectionDown => self.editor_mut().move_cursor(Motion::Down, true),
            EditorAction::SelectionLeft => self.editor_mut().move_cursor(Motion::Left, true),
            EditorAction::SelectionRight => self.editor_mut().move_cursor(Motion::Right, true),
            EditorAction::SelectionWordLeft => {
                self.editor_mut().move_cursor(Motion::WordLeft, true)
            }
            EditorAction::SelectionWordRight => {
                self.editor_mut().move_cursor(Motion::WordRight, true)
            }
            EditorAction::SelectionLineStart => {
                self.editor_mut().move_cursor(Motion::LineStart, true)
            }
            EditorAction::SelectionLineEnd => self.editor_mut().move_cursor(Motion::LineEnd, true),
            EditorAction::SelectionBufferStart => {
                self.editor_mut().move_cursor(Motion::BufferStart, true)
            }
            EditorAction::SelectionBufferEnd => {
                self.editor_mut().move_cursor(Motion::BufferEnd, true)
            }
            EditorAction::SelectionPageUp => self
                .editor_mut()
                .move_cursor(Motion::PageUp { rows: 10 }, true),
            EditorAction::SelectionPageDown => self
                .editor_mut()
                .move_cursor(Motion::PageDown { rows: 10 }, true),
            EditorAction::SelectionAll => self.editor_mut().select_all(),
            EditorAction::EditBackspace => self.editor_mut().backspace(),
            EditorAction::EditDelete => self.editor_mut().delete_forward(),
            EditorAction::EditDeleteWordLeft => self.editor_mut().delete_word_left(),
            EditorAction::EditDeleteToLineStart => self.editor_mut().delete_to_line_start(),
            EditorAction::EditInsertLineAfter => self.editor_mut().insert_line_after(),
            EditorAction::EditInsertLineBefore => self.editor_mut().insert_line_before(),
            EditorAction::EditMoveLinesUp => self.editor_mut().move_lines_up(),
            EditorAction::EditMoveLinesDown => self.editor_mut().move_lines_down(),
            EditorAction::EditCopy => {
                if let Some(text) = self.editor_mut().copy_text() {
                    self.copy_to_clipboards(text, "copied");
                }
            }
            EditorAction::EditCut => {
                if let Some(text) = self.editor_mut().cut() {
                    self.copy_to_clipboards(text, "cut");
                    self.quit_guard.reset();
                }
            }
            EditorAction::EditPaste => {
                if !self.clipboard.is_empty() {
                    let text = self.clipboard.clone();
                    self.editor_mut().insert_text(&text);
                    self.editor_mut().commit_group();
                    self.quit_guard.reset();
                }
            }
            EditorAction::EditUndo => {
                if !self.editor_mut().undo() {
                    self.message = "nothing to undo".to_string();
                }
            }
            EditorAction::EditRedo => {
                if !self.editor_mut().redo() {
                    self.message = "nothing to redo".to_string();
                }
            }
            EditorAction::FileSave => match self.active_document_mut().save() {
                Ok(()) => {
                    self.message = "saved".to_string();
                    self.quit_guard.reset();
                    self.close_guard.reset();
                }
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                    self.message = error.to_string();
                }
                Err(error) => self.message = format!("save failed: {error}"),
            },
            EditorAction::BufferNew => {
                self.documents.push(Document::unnamed());
                self.active = self.documents.len() - 1;
                self.search.close();
                self.close_guard.reset();
            }
            EditorAction::BufferNext => self.switch_buffer(1),
            EditorAction::BufferPrevious => self.switch_buffer(-1),
            EditorAction::BufferClose => {
                if self.close_active_buffer() == QuitDecision::Quit {
                    return QuitDecision::Quit;
                }
            }
            EditorAction::PaletteOpen => {
                self.search.close();
                self.palette.open();
            }
            EditorAction::SearchOpen => {
                self.palette.close();
                self.search
                    .open(false, &mut self.documents[self.active].editor);
            }
            EditorAction::ReplaceOpen => {
                self.palette.close();
                self.search
                    .open(true, &mut self.documents[self.active].editor);
            }
            EditorAction::SearchNext => {
                if self.search.query.is_empty() {
                    self.search
                        .open(false, &mut self.documents[self.active].editor);
                } else if self.search.visible {
                    self.search.next(&mut self.documents[self.active].editor);
                } else {
                    self.search
                        .next_from_cursor(&mut self.documents[self.active].editor);
                }
            }
            EditorAction::SearchPrevious => {
                if self.search.query.is_empty() {
                    self.search
                        .open(false, &mut self.documents[self.active].editor);
                } else if self.search.visible {
                    self.search
                        .previous(&mut self.documents[self.active].editor);
                } else {
                    self.search
                        .previous_from_cursor(&mut self.documents[self.active].editor);
                }
            }
            EditorAction::ReplaceNext => {
                if self.search.query.is_empty() {
                    self.search
                        .open(true, &mut self.documents[self.active].editor);
                } else {
                    if !self.search.visible {
                        self.search
                            .next_from_cursor(&mut self.documents[self.active].editor);
                    }
                    self.search
                        .replace_current(&mut self.documents[self.active].editor);
                }
            }
            EditorAction::ReplaceAll => {
                if self.search.query.is_empty() {
                    self.search
                        .open(true, &mut self.documents[self.active].editor);
                } else {
                    if !self.search.visible {
                        self.search
                            .next_from_cursor(&mut self.documents[self.active].editor);
                    }
                    self.search
                        .replace_all(&mut self.documents[self.active].editor);
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
                .editor()
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
        self.documents.iter().any(Document::is_modified)
    }

    fn switch_buffer(&mut self, delta: isize) {
        let len = self.documents.len();
        if len <= 1 {
            return;
        }
        self.active = if delta.is_negative() {
            (self.active + len - 1) % len
        } else {
            (self.active + 1) % len
        };
        self.search.close();
        self.close_guard.reset();
    }

    fn close_active_buffer(&mut self) -> QuitDecision {
        let modified = self.active_document().is_modified();
        if self.close_guard.request_quit(modified) == QuitDecision::Continue {
            self.message = "unsaved changes; close again to discard".to_string();
            return QuitDecision::Continue;
        }

        if self.documents.len() == 1 {
            return QuitDecision::Quit;
        }

        self.documents.remove(self.active);
        if self.active >= self.documents.len() {
            self.active = self.documents.len() - 1;
        }
        self.search.close();
        self.close_guard.reset();
        QuitDecision::Continue
    }

    fn tab_items(&self) -> Vec<TabItem> {
        self.documents
            .iter()
            .enumerate()
            .map(|(index, document)| TabItem {
                label: format!(
                    "{}:{}{}",
                    index + 1,
                    document.display_name(),
                    if document.is_modified() { " [+]" } else { "" }
                ),
                active: index == self.active,
            })
            .collect()
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

#[derive(Debug, Clone, Eq, PartialEq)]
struct TabItem {
    label: String,
    active: bool,
}

fn draw_tab_bar(screen: &mut Screen, tabs: &[TabItem]) {
    if screen.height() == 0 {
        return;
    }

    let mut x = 0;
    for (index, tab) in tabs.iter().enumerate() {
        if index > 0 {
            x = screen.put_str(x as u16, 0, "  ", Style::default());
        }
        if x >= usize::from(screen.width()) {
            break;
        }

        let style = Style {
            reverse: tab.active,
            dim: false,
            fg: None,
        };
        let remaining = usize::from(screen.width()).saturating_sub(x);
        let label = ellipsize(&tab.label, remaining);
        x = screen.put_str(x as u16, 0, &label, style);
    }
}

fn ellipsize(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }

    let mut clipped = text.chars().take(width - 1).collect::<String>();
    clipped.push('…');
    clipped
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            unique.push(path);
        }
    }
    unique
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
    use super::{EventLoop, QuitDecision, QuitGuard, TabItem, draw_tab_bar};
    use crate::{
        highlight::ThemeChoice,
        input::{InputEvent, Key, KeyEvent},
        keymap::EditorAction,
        ui::Screen,
    };
    use std::path::PathBuf;

    fn buffer_text(event_loop: &EventLoop) -> String {
        String::from_utf8(event_loop.editor().buffer.to_bytes()).unwrap()
    }

    fn active_buffer_text(event_loop: &EventLoop) -> String {
        buffer_text(event_loop)
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "coda-test-{name}-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    fn row_text(screen: &Screen, y: u16) -> String {
        screen
            .row(y)
            .unwrap()
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>()
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
        assert!(
            event_loop.editor_mut().undo(),
            "whole paste is one undo group"
        );
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

    #[test]
    fn open_many_keeps_argument_order_and_dedupes_paths() {
        let first = temp_path("open-many-a");
        let second = temp_path("open-many-b");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();

        let event_loop = EventLoop::open_many(
            vec![first.clone(), second.clone(), first.clone()],
            Vec::new(),
            Vec::new(),
            ThemeChoice::Dark,
        )
        .unwrap();

        assert_eq!(event_loop.documents.len(), 2);
        assert_eq!(event_loop.active, 0);
        assert_eq!(event_loop.documents[0].path.as_ref(), Some(&first));
        assert_eq!(event_loop.documents[1].path.as_ref(), Some(&second));

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn buffer_next_and_previous_wrap_around() {
        let paths = [
            temp_path("wrap-a"),
            temp_path("wrap-b"),
            temp_path("wrap-c"),
        ];
        for path in &paths {
            std::fs::write(path, b"").unwrap();
        }
        let mut event_loop =
            EventLoop::open_many(paths.to_vec(), Vec::new(), Vec::new(), ThemeChoice::Dark)
                .unwrap();

        event_loop.dispatch(EditorAction::BufferNext);
        assert_eq!(event_loop.active, 1);
        event_loop.dispatch(EditorAction::BufferNext);
        assert_eq!(event_loop.active, 2);
        event_loop.dispatch(EditorAction::BufferNext);
        assert_eq!(event_loop.active, 0);
        event_loop.dispatch(EditorAction::BufferPrevious);
        assert_eq!(event_loop.active, 2);

        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn buffer_close_removes_clean_buffer_immediately_and_warns_for_modified() {
        let clean = temp_path("close-clean");
        let modified = temp_path("close-modified");
        std::fs::write(&clean, b"clean").unwrap();
        std::fs::write(&modified, b"modified").unwrap();
        let mut event_loop = EventLoop::open_many(
            vec![clean.clone(), modified.clone()],
            Vec::new(),
            Vec::new(),
            ThemeChoice::Dark,
        )
        .unwrap();

        assert_eq!(
            event_loop.dispatch(EditorAction::BufferClose),
            QuitDecision::Continue
        );
        assert_eq!(event_loop.documents.len(), 1);
        assert_eq!(event_loop.documents[0].path.as_ref(), Some(&modified));

        event_loop.editor_mut().insert_text("!");
        assert_eq!(
            event_loop.dispatch(EditorAction::BufferClose),
            QuitDecision::Continue
        );
        assert_eq!(
            event_loop.message,
            "unsaved changes; close again to discard"
        );
        assert_eq!(event_loop.documents.len(), 1);
        assert_eq!(
            event_loop.dispatch(EditorAction::BufferClose),
            QuitDecision::Quit
        );

        let _ = std::fs::remove_file(clean);
        let _ = std::fs::remove_file(modified);
    }

    #[test]
    fn last_clean_buffer_close_quits() {
        let path = temp_path("last-clean-close");
        std::fs::write(&path, b"clean").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert_eq!(
            event_loop.dispatch(EditorAction::BufferClose),
            QuitDecision::Quit
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn buffer_state_is_independent_per_document() {
        let first = temp_path("state-a");
        let second = temp_path("state-b");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();
        let mut event_loop = EventLoop::open_many(
            vec![first.clone(), second.clone()],
            Vec::new(),
            Vec::new(),
            ThemeChoice::Dark,
        )
        .unwrap();

        event_loop.editor_mut().insert_text("A");
        event_loop.documents[0].view.top_line = 7;
        assert!(event_loop.documents[0].is_modified());

        event_loop.dispatch(EditorAction::BufferNext);
        event_loop.editor_mut().insert_text("B");
        assert_eq!(active_buffer_text(&event_loop), "Bb");
        assert_eq!(event_loop.documents[1].view.top_line, 0);

        event_loop.dispatch(EditorAction::BufferPrevious);
        assert_eq!(active_buffer_text(&event_loop), "Aa");
        assert_eq!(event_loop.documents[0].view.top_line, 7);
        assert!(event_loop.editor_mut().undo());
        assert_eq!(active_buffer_text(&event_loop), "a");
        assert_eq!(event_loop.documents[1].editor.buffer.to_bytes(), b"Bb");

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn buffer_new_creates_unnamed_active_buffer_and_save_reports_save_as_missing() {
        let path = temp_path("buffer-new-base");
        std::fs::write(&path, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.dispatch(EditorAction::BufferNew);
        assert_eq!(event_loop.documents.len(), 2);
        assert_eq!(event_loop.active, 1);
        assert_eq!(event_loop.active_document().display_name(), "[No Name]");

        event_loop.editor_mut().insert_text("draft");
        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(event_loop.message, "save as: not implemented yet");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tab_bar_marks_active_modified_and_truncates() {
        let mut screen = Screen::new(25, 1);
        draw_tab_bar(
            &mut screen,
            &[
                TabItem {
                    label: "1:main.rs".to_string(),
                    active: true,
                },
                TabItem {
                    label: "2:notes.md [+]".to_string(),
                    active: false,
                },
            ],
        );

        assert_eq!(row_text(&screen, 0), "1:main.rs  2:notes.md [+]");
        assert!(screen.cell(0, 0).unwrap().style.reverse);
        assert!(!screen.cell(11, 0).unwrap().style.reverse);

        let mut narrow = Screen::new(20, 1);
        draw_tab_bar(
            &mut narrow,
            &[TabItem {
                label: "1:very-long-file-name.rs".to_string(),
                active: true,
            }],
        );
        assert_eq!(row_text(&narrow, 0), "1:very-long-file-na…");
    }

    #[test]
    fn app_quit_warns_when_any_buffer_is_modified() {
        let first = temp_path("quit-a");
        let second = temp_path("quit-b");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();
        let mut event_loop = EventLoop::open_many(
            vec![first.clone(), second.clone()],
            Vec::new(),
            Vec::new(),
            ThemeChoice::Dark,
        )
        .unwrap();

        event_loop.dispatch(EditorAction::BufferNext);
        event_loop.editor_mut().insert_text("dirty");
        event_loop.dispatch(EditorAction::BufferPrevious);

        assert_eq!(
            event_loop.dispatch(EditorAction::AppQuit),
            QuitDecision::Continue
        );
        assert_eq!(
            event_loop.message,
            "unsaved changes; press quit again to exit"
        );
        assert_eq!(
            event_loop.dispatch(EditorAction::AppQuit),
            QuitDecision::Quit
        );

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}

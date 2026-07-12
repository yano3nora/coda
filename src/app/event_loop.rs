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
        BracketedPasteGuard, CapabilityDetection, CapabilityProbe, InputEvent, Key, KeyEvent,
        KeyboardCapabilities, KeyboardProtocolGuard, Modifiers, RawModeGuard, drain_input_events,
        flush_pending_escape,
        quirks::{self, QuirkEffect, TerminalQuirk},
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
    inspector::{InspectorOverlay, draw_inspector, is_close_key},
    palette::{CommandPalette, filter_actions},
    search_overlay::{SearchOverlay, draw_search_overlay},
};

const SEQUENCE_TIMEOUT: Duration = Duration::from_millis(800);
const IDLE_POLL_MS: i32 = 100;
/// Keyboard capability negotiation deadline (SPEC-0003 Open Question answer,
/// design decision 260712): DA1 answering first resolves "legacy" sooner on
/// terminals that support it, so this is a worst-case fallback, not the
/// common-case latency.
const CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Startup warning shown once a legacy terminal is confirmed (TASK-260712-16).
const LEGACY_CAPABILITY_WARNING: &str = "legacy terminal input: Ctrl+Shift+J / Shift+Enter etc. cannot be distinguished — run inspector.open for details";

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
    inspector: InspectorOverlay,
    message: String,
    clipboard: String,
    pending_terminal_write: Vec<u8>,
    pending_keys: Vec<KeyEvent>,
    pending_since: Option<Instant>,
    quit_guard: QuitGuard,
    close_guard: QuitGuard,
    /// Ghostty keybind interception quirks detected at startup (ADR-0007
    /// decision 2(b)). Kept for the `:inspect-key` live mode (TASK-260711-17)
    /// to annotate incoming events, not just for the one-line startup warning.
    ghostty_quirks: Vec<TerminalQuirk>,
    /// In-flight keyboard capability detection, armed by `run()` right after
    /// the protocol push. `None` once resolved (or if never armed, as in
    /// `EventLoop::open`/`open_many` used by tests without a real terminal)
    /// — see `resolve_capabilities` (TASK-260712-16).
    capability_probe: Option<CapabilityProbe>,
    /// The resolved capability judgment, once known. `None` means detection
    /// is still pending; surfaced to the `:inspect-key` overlay's protocol
    /// line via `draw_inspector`.
    capability_detection: Option<CapabilityDetection>,
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
        let resolver = Resolver::new(bindings);

        // Query Ghostty's own keybind table before raw mode / the alt screen
        // are entered (that happens later, in `run()`): `quirks::detect()`
        // shells out to `ghostty +list-keybinds`, which requires the
        // terminal to still be in its normal state. Any interception that
        // would change behavior gets folded into `warnings` as a single
        // summary line (ADR-0007 decision 2(b), TASK-260711-17).
        let ghostty_quirks = quirks::detect();
        if let Some(warning) = ghostty_intercept_warning(&ghostty_quirks, &resolver) {
            // Front of the list, not the back: the status bar shows one line
            // and config warnings can be numerous (one per broken binding),
            // which would push this single aggregated summary out of view.
            // Interim measure until a full warning viewer exists (backlog).
            warnings.insert(0, warning);
        }

        Ok(Self {
            documents,
            active: 0,
            resolver,
            highlight_engine: HighlightEngine::new(theme),
            palette: CommandPalette::default(),
            search: SearchOverlay::default(),
            inspector: InspectorOverlay::default(),
            message: warnings.join("; "),
            clipboard: String::new(),
            pending_terminal_write: Vec::new(),
            pending_keys: Vec::new(),
            pending_since: None,
            quit_guard: QuitGuard::default(),
            close_guard: QuitGuard::default(),
            ghostty_quirks,
            capability_probe: None,
            capability_detection: None,
        })
    }

    /// Ghostty quirks detected at startup, for the `:inspect-key` live mode
    /// to annotate incoming events against (TASK-260711-17).
    pub(crate) fn ghostty_quirks(&self) -> &[TerminalQuirk] {
        &self.ghostty_quirks
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
        // DA1 (Primary Device Attributes) is the fallback signal for
        // terminals that don't understand `CSI ?u` at all: almost every
        // terminal answers DA1, so its arrival (without a preceding
        // `CapabilityReply`) proves "legacy" without waiting out the full
        // timeout (SPEC-0003 detection design 260712).
        alt.writer_mut().write_all(b"\x1b[c")?;
        alt.writer_mut().flush()?;
        self.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + CAPABILITY_PROBE_TIMEOUT,
        ));
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
                if self.inspector.visible {
                    // Chunk-level approximation (design decision 260712):
                    // attribute the whole read chunk to whichever event(s) it
                    // decodes into rather than tracking exact byte spans.
                    self.inspector.push_raw(&chunk[..read]);
                }
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

            // Idle poll granularity (100ms) is fine grain enough for the
            // 500ms capability deadline (SPEC-0003 Open Question answer);
            // this is a no-op once `capability_probe` has resolved and been
            // cleared.
            self.tick_capability_probe();

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
        // Inspector before palette: when both are visible, the palette (its
        // rescue entry point always wins per AGENTS.md) draws on top.
        draw_inspector(screen, &self.inspector, self.capability_detection);
        super::palette::draw_palette(screen, &self.palette, &items);
    }

    fn handle_input_event(&mut self, event: InputEvent) -> QuitDecision {
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Paste(text) => self.handle_paste_input(&text),
            InputEvent::CapabilityReply(_) | InputEvent::DeviceAttributes => {
                self.feed_capability_probe(&event);
                QuitDecision::Continue
            }
        }
    }

    /// Feeds a decoded capability-query reply to the in-flight probe, if
    /// any. `capability_probe` only exists while detection is pending
    /// (`run()` arms it right after the protocol push, `resolve_capabilities`
    /// clears it), so this is a no-op once capabilities are known or when
    /// running without a real terminal (tests via `EventLoop::open`).
    fn feed_capability_probe(&mut self, event: &InputEvent) {
        let Some(probe) = self.capability_probe.as_mut() else {
            return;
        };
        if let Some(detection) = probe.on_event(event) {
            self.resolve_capabilities(detection);
        }
    }

    /// Called once per event-loop iteration so a probe that never receives a
    /// reply (terminal ignores both queries entirely) still resolves after
    /// its deadline instead of leaving capabilities undetected forever.
    fn tick_capability_probe(&mut self) {
        let Some(probe) = self.capability_probe.as_mut() else {
            return;
        };
        if let Some(detection) = probe.on_tick(Instant::now()) {
            self.resolve_capabilities(detection);
        }
    }

    /// Records the resolved capability judgment and, for a legacy result,
    /// prepends the startup warning to `self.message` — same "front of the
    /// list" reasoning as `ghostty_intercept_warning` (TASK-260711-17): the
    /// status bar shows one line, and this is a single fact the user needs
    /// regardless of whatever else is already queued there.
    fn resolve_capabilities(&mut self, detection: CapabilityDetection) {
        self.capability_probe = None;
        self.capability_detection = Some(detection);
        if detection.capabilities() != KeyboardCapabilities::modern() {
            self.message = if self.message.is_empty() {
                LEGACY_CAPABILITY_WARNING.to_string()
            } else {
                format!("{LEGACY_CAPABILITY_WARNING}; {}", self.message)
            };
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
        } else if self.inspector.visible {
            // Observe-only: record that a paste happened without ever
            // exposing its contents (design decision 260712).
            self.inspector.record_paste(text.len());
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

        if self.inspector.visible {
            return self.handle_inspector_key(event);
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

    /// Handles a key while the inspector overlay is visible. This is the
    /// enforcement point for "observation must never mutate editor state"
    /// (TASK-260711-17): every key is either the close key or gets recorded,
    /// never forwarded to the resolver/dispatch path.
    fn handle_inspector_key(&mut self, event: KeyEvent) -> QuitDecision {
        if is_close_key(&event) {
            self.inspector.close();
            return QuitDecision::Continue;
        }
        let context = self.context();
        // `ghostty_quirks()` borrows `&self`, which cannot overlap with the
        // `&mut self.inspector` borrow below, so its (small) result is
        // cloned out first rather than passed through directly.
        let quirks = self.ghostty_quirks().to_vec();
        self.inspector
            .record_key(&event, &self.resolver, &context, &quirks);
        QuitDecision::Continue
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
            EditorAction::InspectorOpen => {
                self.search.close();
                self.inspector.open();
                self.message = "inspect-key: press any key".to_string();
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

/// Cross-references detected Ghostty quirks against the resolver's active
/// bindings and summarizes the ones worth warning about into one line.
///
/// A quirk is only worth mentioning if this program actually has a binding
/// on the trigger chord (otherwise there is nothing to lose). For
/// `Translated` quirks, the translated keystroke is resolved too: if it maps
/// to the *same* action as the original trigger, the terminal's rewrite is
/// harmless (this is exactly the ADR-0011 case — `cmd+left` arrives as
/// `ctrl+a`, and the default table binds both to `cursor.lineStart`) and the
/// quirk is not reported.
fn ghostty_intercept_warning(quirks: &[TerminalQuirk], resolver: &Resolver) -> Option<String> {
    // `EditorContext::default()` already has editor_focus/text_input_focus
    // true, which is the representative context the design calls for.
    let context = EditorContext::default();

    let mut affected = Vec::new();
    for quirk in quirks {
        let Some(trigger_action) =
            resolved_action(resolver, std::slice::from_ref(&quirk.trigger), &context)
        else {
            continue;
        };

        let warn = match &quirk.effect {
            QuirkEffect::Consumed { .. } => true,
            QuirkEffect::Translated { events, .. } => {
                events.is_empty()
                    || resolved_action(resolver, events, &context) != Some(trigger_action)
            }
        };

        if warn {
            affected.push(quirk.trigger.to_string());
        }
    }

    format_intercept_warning(&affected)
}

/// Resolves a key sequence to a bound action, treating anything short of an
/// exact `Matched` result (no binding, or a sequence prefix still pending) as
/// "no action" for warning purposes.
fn resolved_action(
    resolver: &Resolver,
    keys: &[KeyEvent],
    context: &EditorContext,
) -> Option<EditorAction> {
    match resolver.resolve(keys, context) {
        ResolveResult::Matched(action) => Some(action),
        _ => None,
    }
}

/// Formats the startup warning line, showing up to 3 example chords and
/// truncating the rest with `…`. Returns `None` when nothing is affected.
fn format_intercept_warning(affected: &[String]) -> Option<String> {
    if affected.is_empty() {
        return None;
    }

    let shown = affected
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if affected.len() > 3 { ", …" } else { "" };
    Some(format!(
        "Ghostty intercepts {} bindings: {shown}{suffix} — run inspector.open for details",
        affected.len()
    ))
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
    use super::{
        EventLoop, QuitDecision, QuitGuard, TabItem, draw_tab_bar, format_intercept_warning,
        ghostty_intercept_warning,
    };
    use crate::{
        app::default_bindings::{Platform, bindings_for},
        highlight::ThemeChoice,
        input::{
            CapabilityDetection, CapabilityProbe, InputEvent, Key, KeyEvent,
            quirks::parse_ghostty_keybinds,
        },
        keymap::{EditorAction, Resolver},
        ui::Screen,
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

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
    fn inspector_open_dispatch_shows_overlay_with_press_a_key_message() {
        let path = temp_path("inspector-open");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert_eq!(
            event_loop.dispatch(EditorAction::InspectorOpen),
            QuitDecision::Continue
        );
        assert!(event_loop.inspector.visible);
        assert_eq!(event_loop.message, "inspect-key: press any key");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn inspector_observes_keys_without_mutating_the_buffer_and_esc_resumes_editing() {
        let path = temp_path("inspector-observe");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.dispatch(EditorAction::InspectorOpen);
        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert_eq!(
            buffer_text(&event_loop),
            "abc",
            "inspector must not forward keys to the editor"
        );
        assert!(event_loop.inspector.visible);

        assert_eq!(
            event_loop.handle_key(KeyEvent::plain(Key::Esc)),
            QuitDecision::Continue
        );
        assert!(!event_loop.inspector.visible, "Esc closes the overlay");

        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert_eq!(
            buffer_text(&event_loop),
            "xabc",
            "normal editing resumes once the overlay is closed"
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

    // Faithful to real `+list-keybinds` output: `text:` payloads carry a
    // doubled backslash on the wire (see input/quirks.rs fixture note).
    const GHOSTTY_FIXTURE: &str = "\
keybind = super+arrow_right=text:\\\\x05
keybind = super+arrow_left=text:\\\\x01
keybind = alt+arrow_left=esc:b
keybind = alt+arrow_right=esc:f
keybind = super+c=copy_to_clipboard:mixed
keybind = super+a=select_all
keybind = super+f=start_search
keybind = shift+arrow_left=adjust_selection:left
keybind = super+shift+arrow_up=jump_to_prompt:-1
keybind = super+1=goto_tab:1
keybind = super+digit_1=goto_tab:1
";

    #[test]
    fn ghostty_warning_suppresses_same_action_translation_but_flags_consumed_binds() {
        let quirks = parse_ghostty_keybinds(GHOSTTY_FIXTURE);
        let resolver = Resolver::new(bindings_for(Platform::MacOs));

        let warning =
            ghostty_intercept_warning(&quirks, &resolver).expect("some quirks are warn-worthy");

        // cmd+left is translated to ^A, which resolves to the same
        // cursor.lineStart action as the cmd+left default binding (ADR-0011)
        // — the terminal rewrite is harmless, so it must not be reported.
        assert!(
            !warning.contains("Left"),
            "cmd+left same-action translation must be suppressed: {warning}"
        );
        // cmd+f (start_search) and cmd+a (select_all) are consumed outright
        // by Ghostty and coda has default bindings on both chords, so both
        // must be flagged.
        assert!(warning.contains('F'), "cmd+f must be flagged: {warning}");
        assert!(warning.contains('A'), "cmd+a must be flagged: {warning}");
    }

    #[test]
    fn ghostty_warning_is_none_when_no_quirks_hit_a_binding() {
        let resolver = Resolver::new(bindings_for(Platform::MacOs));
        assert_eq!(ghostty_intercept_warning(&[], &resolver), None);
    }

    #[test]
    fn format_intercept_warning_is_none_for_empty_list() {
        assert_eq!(format_intercept_warning(&[]), None);
    }

    #[test]
    fn format_intercept_warning_lists_up_to_three_examples_without_truncation() {
        let affected = vec![
            "Cmd+A".to_string(),
            "Cmd+F".to_string(),
            "Cmd+W".to_string(),
        ];
        assert_eq!(
            format_intercept_warning(&affected),
            Some(
                "Ghostty intercepts 3 bindings: Cmd+A, Cmd+F, Cmd+W — run inspector.open for details"
                    .to_string()
            )
        );
    }

    #[test]
    fn format_intercept_warning_truncates_beyond_three_examples() {
        let affected = vec![
            "Cmd+A".to_string(),
            "Cmd+F".to_string(),
            "Cmd+W".to_string(),
            "Cmd+C".to_string(),
        ];
        assert_eq!(
            format_intercept_warning(&affected),
            Some(
                "Ghostty intercepts 4 bindings: Cmd+A, Cmd+F, Cmd+W, … — run inspector.open for details"
                    .to_string()
            )
        );
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

    /// TASK-260712-16 testcases: capability resolution and the startup
    /// warning it triggers. `EventLoop::open` never touches a real terminal,
    /// so these arm/feed the probe manually rather than going through `run()`.

    #[test]
    fn modern_capability_reply_resolves_without_a_legacy_warning() {
        let path = temp_path("capability-modern");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        // Startup `message` may already carry an unrelated warning (e.g. a
        // real Ghostty quirk summary on a dev machine running this suite
        // inside Ghostty, TASK-260711-17) — capture it instead of assuming
        // empty, so this test only asserts what modern capability resolution
        // itself is responsible for: not touching `message` at all.
        let message_before = event_loop.message.clone();

        event_loop.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + Duration::from_millis(500),
        ));
        event_loop.handle_input_event(InputEvent::CapabilityReply(1));

        assert_eq!(
            event_loop.capability_detection,
            Some(CapabilityDetection::KittyFlags(1))
        );
        assert!(
            event_loop.capability_probe.is_none(),
            "probe is cleared once resolved"
        );
        assert_eq!(
            event_loop.message, message_before,
            "modern detection must not warn"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_capability_detection_prepends_warning_to_empty_message() {
        let path = temp_path("capability-legacy-empty");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.message.clear();

        event_loop.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + Duration::from_millis(500),
        ));
        event_loop.handle_input_event(InputEvent::DeviceAttributes);

        assert_eq!(
            event_loop.capability_detection,
            Some(CapabilityDetection::LegacyDeviceAttributes)
        );
        assert_eq!(
            event_loop.message,
            "legacy terminal input: Ctrl+Shift+J / Shift+Enter etc. cannot be distinguished — run inspector.open for details"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_capability_detection_prepends_warning_before_existing_message() {
        let path = temp_path("capability-legacy-existing");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.message = "new file".to_string();

        event_loop.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + Duration::from_millis(500),
        ));
        event_loop.handle_input_event(InputEvent::DeviceAttributes);

        assert_eq!(
            event_loop.message,
            "legacy terminal input: Ctrl+Shift+J / Shift+Enter etc. cannot be distinguished — run inspector.open for details; new file"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn capability_probe_tick_resolves_legacy_after_the_deadline_passes() {
        let path = temp_path("capability-timeout");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        // Deadline already in the past: the very next tick must resolve.
        event_loop.capability_probe = Some(CapabilityProbe::arm(Instant::now()));
        event_loop.tick_capability_probe();

        assert_eq!(
            event_loop.capability_detection,
            Some(CapabilityDetection::LegacyTimeout)
        );
        assert!(event_loop.capability_probe.is_none());
        assert!(event_loop.message.contains("legacy terminal input"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn capability_probe_tick_before_deadline_leaves_message_untouched() {
        let path = temp_path("capability-pending");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.message.clear();

        event_loop.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + Duration::from_secs(60),
        ));
        event_loop.tick_capability_probe();

        assert_eq!(event_loop.capability_detection, None, "still pending");
        assert_eq!(event_loop.message, "");
        assert!(event_loop.capability_probe.is_some());

        let _ = std::fs::remove_file(path);
    }
}

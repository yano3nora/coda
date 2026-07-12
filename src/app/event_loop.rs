//! Terminal event loop integrating input, resolver, editor core, and renderer.

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use libc::STDIN_FILENO;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    core::editor::{EditorCore, Motion},
    core::position::Position,
    highlight::{HighlightEngine, ThemeChoice},
    input::{
        BracketedPasteGuard, CapabilityDetection, CapabilityProbe, InputEvent, Key, KeyEvent,
        KeyboardCapabilities, KeyboardProtocolGuard, Modifiers, MouseButton, MouseEvent,
        MouseEventKind, MouseReportingGuard, RawModeGuard, drain_input_events,
        flush_pending_escape, poll_readable,
        quirks::{self, QuirkEffect, TerminalQuirk},
    },
    keymap::{EditorAction, EditorContext, ResolveResult, Resolver},
    ui::{
        AltScreenGuard, Screen, Style, render_diff, render_full, take_pending_resize, terminal_size,
    },
};

use super::{
    clipboard, config, default_bindings,
    document::{Document, SaveError},
    editor_view::StatusLine,
    file,
    inspector::{InspectorOverlay, draw_inspector, is_close_key},
    palette::{CommandPalette, filter_actions},
    prompt_overlay::{PromptOutcome, PromptOverlay, PromptPurpose, draw_prompt_overlay},
    search_overlay::{SearchOverlay, draw_search_overlay},
    which_key::{draw_which_key, which_key_lines},
};

/// Startup default for `[keymap] sequence_timeout_ms`; see
/// `config::DEFAULT_SEQUENCE_TIMEOUT_MS` (SPEC-0005).
const DEFAULT_SEQUENCE_TIMEOUT: Duration =
    Duration::from_millis(config::DEFAULT_SEQUENCE_TIMEOUT_MS);
const IDLE_POLL_MS: i32 = 100;
/// Wheel scroll unit (ADR-0008 Open Question answer, 260712): 3 visual rows
/// per notch, no acceleration.
const WHEEL_SCROLL_LINES: isize = 3;
const WHEEL_SCROLL_COLUMNS: isize = 3;
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);
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

/// Two-stage confirm for `file.save` hitting an external-change conflict
/// (TASK-260712 Gate 1), mirroring `QuitGuard`: the first conflicting save
/// only warns, the very next `file.save` forces the overwrite. Any other
/// dispatched action resets it, so stepping away and coming back never
/// silently forces a stale save.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
struct SaveConflictGuard {
    warned: bool,
}

impl SaveConflictGuard {
    fn should_force(&self) -> bool {
        self.warned
    }

    fn warn(&mut self) {
        self.warned = true;
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
    /// Generic single-line input overlay, currently only used for
    /// `file.saveAs` (TASK-260712 Gate 1).
    prompt: PromptOverlay,
    inspector: InspectorOverlay,
    message: String,
    clipboard: String,
    pending_terminal_write: Vec<u8>,
    pending_keys: Vec<KeyEvent>,
    pending_since: Option<Instant>,
    quit_guard: QuitGuard,
    close_guard: QuitGuard,
    save_conflict_guard: SaveConflictGuard,
    /// Save As target that was already warned about via "file exists;
    /// enter again to overwrite" (TASK-260712 Gate 1). A resubmission with
    /// a *different* path (typing more, or picking another purpose) is a
    /// fresh attempt, not a confirmation, so this must match exactly.
    save_as_overwrite_confirm: Option<PathBuf>,
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
    /// Visual line wrap (TASK-260711-18). One editor-wide flag, not
    /// per-document: `view.toggleWrap` and the `[editor] wrap` config apply
    /// to every buffer, mirroring how VS Code's setting behaves in practice.
    wrap: bool,
    /// `[keymap] sequence_timeout_ms` (SPEC-0005): pending-sequence wait
    /// before the exact match fires.
    sequence_timeout: Duration,
    /// `[terminal] capability_warning` (SPEC-0005): gates only the startup
    /// status-bar warning; detection itself still runs for `:inspect-key`.
    capability_warning: bool,
    /// Last known terminal size, for mapping mouse cells to buffer positions
    /// outside `draw` (updated by `run()` on startup and resize).
    screen_size: (u16, u16),
    /// Buffer position where the current left-button drag started (ADR-0008):
    /// set on press, extended into a selection on drag, cleared on release.
    drag_anchor: Option<Position>,
    last_click: Option<(Instant, Position, u8)>,
    /// Whether `draw` keeps the viewport attached to the cursor. Wheel
    /// scrolling detaches (the view moves, the cursor stays); any keystroke
    /// or click re-attaches — mirroring how GUI editors treat scroll vs input.
    follow_cursor: bool,
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
            if load_info.readonly {
                warnings.push(format!(
                    "{}: large file (>10 MB); opened read-only",
                    document.display_name()
                ));
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
            prompt: PromptOverlay::default(),
            inspector: InspectorOverlay::default(),
            message: warnings.join("; "),
            clipboard: String::new(),
            pending_terminal_write: Vec::new(),
            pending_keys: Vec::new(),
            pending_since: None,
            quit_guard: QuitGuard::default(),
            close_guard: QuitGuard::default(),
            save_conflict_guard: SaveConflictGuard::default(),
            save_as_overwrite_confirm: None,
            ghostty_quirks,
            capability_probe: None,
            capability_detection: None,
            wrap: false,
            sequence_timeout: DEFAULT_SEQUENCE_TIMEOUT,
            capability_warning: true,
            screen_size: (80, 24),
            drag_anchor: None,
            last_click: None,
            follow_cursor: true,
        })
    }

    /// Rebuilds only the default Ctrl+C policy while preserving user and
    /// imported bindings and their higher source priority.
    pub(crate) fn set_ctrl_c_quits(&mut self, enabled: bool) {
        let mut bindings = default_bindings::bindings_with_ctrl_c_quit(enabled);
        bindings.extend(
            self.resolver
                .bindings()
                .iter()
                .filter(|binding| binding.source != crate::keymap::Source::Default)
                .cloned(),
        );
        self.resolver = Resolver::new(bindings);
    }

    /// Removes every binding containing a chord measured as mismatched for
    /// this exact terminal program/version. Sequences are atomic: if one
    /// chord cannot arrive, the whole binding is unusable.
    pub(crate) fn disable_chords(&mut self, disabled: &[KeyEvent]) {
        if disabled.is_empty() {
            return;
        }
        let bindings = self
            .resolver
            .bindings()
            .iter()
            .filter(|binding| !binding.keys.iter().any(|key| disabled.contains(key)))
            .cloned()
            .collect();
        self.resolver = Resolver::new(bindings);
    }

    /// Applies the `[editor] wrap` startup default from config.toml
    /// (TASK-260711-18); `view.toggleWrap` flips it at runtime.
    pub(crate) fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
    }

    /// Applies `[keymap] sequence_timeout_ms` from config.toml (SPEC-0005).
    pub(crate) fn set_sequence_timeout(&mut self, timeout: Duration) {
        self.sequence_timeout = timeout;
    }

    /// Applies `[terminal] capability_warning` from config.toml (SPEC-0005).
    pub(crate) fn set_capability_warning(&mut self, enabled: bool) {
        self.capability_warning = enabled;
    }

    /// Applies `[keymap] palette_key` from config.toml: rebinds the
    /// `ctrl+space` convenience rescue binding to the configured chord. The
    /// `escape` (palette-visible) rescue binding and the hardwired F1 are
    /// untouched, so the palette can never be configured out of reach
    /// (SPEC-0002).
    pub(crate) fn set_palette_key(&mut self, key: KeyEvent) {
        let bindings = self
            .resolver
            .bindings()
            .iter()
            .cloned()
            .map(|mut binding| {
                if binding.source == crate::keymap::Source::Rescue
                    && binding.action == EditorAction::PaletteOpen
                    && binding.when.is_none()
                {
                    binding.keys = vec![key.clone()];
                }
                binding
            })
            .collect();
        self.resolver = Resolver::new(bindings);
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
        let _mouse = MouseReportingGuard::enable(alt.writer_mut())?;
        let mut stdin = io::stdin().lock();
        let mut byte_buffer = Vec::new();
        let (width, height) = terminal_size().unwrap_or((80, 24));
        self.screen_size = (width, height);
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
            if poll_readable(STDIN_FILENO, timeout)? {
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
                self.screen_size = (width, height);
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
        let wrap = self.wrap;
        let follow_cursor = self.follow_cursor;
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
            wrap,
            follow_cursor,
        );
        // Which-key (backlog P1): re-resolve the pending prefix each frame so
        // the panel always mirrors what the resolver would do — no cached
        // candidate state to fall out of sync.
        if !self.pending_keys.is_empty()
            && let ResolveResult::Pending { exact, candidates } =
                self.resolver.resolve(&self.pending_keys, &self.context())
        {
            let lines = which_key_lines(&self.pending_keys, &candidates, exact);
            draw_which_key(screen, &pending, &lines);
        }
        let items = filter_actions(&self.palette.query, self.resolver.bindings());
        draw_search_overlay(screen, &self.search);
        draw_prompt_overlay(screen, &self.prompt);
        // Inspector before palette: when both are visible, the palette (its
        // rescue entry point always wins per AGENTS.md) draws on top.
        draw_inspector(screen, &self.inspector, self.capability_detection);
        super::palette::draw_palette(screen, &self.palette, &items);
    }

    fn handle_input_event(&mut self, event: InputEvent) -> QuitDecision {
        match event {
            InputEvent::Key(key) => self.handle_key(key),
            InputEvent::Paste(text) => self.handle_paste_input(&text),
            InputEvent::Mouse(mouse) => self.handle_mouse_event(mouse),
            InputEvent::CapabilityReply(_) | InputEvent::DeviceAttributes => {
                self.feed_capability_probe(&event);
                QuitDecision::Continue
            }
        }
    }

    /// Click = cursor move, drag = selection, wheel = viewport scroll
    /// (ADR-0008 §3). Terminals normally reserve Shift+drag before sending an
    /// SGR event. If one is nevertheless delivered, coda consumes but ignores
    /// it; input bytes cannot be returned to the terminal after decoding.
    /// Events during an overlay are also dropped — overlays are keyboard-driven
    /// and a click must not silently move the cursor underneath them.
    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> QuitDecision {
        if mouse.modifiers.contains_shift() {
            return QuitDecision::Continue;
        }
        if self.palette.visible || self.prompt.visible || self.inspector.visible {
            return QuitDecision::Continue;
        }
        match mouse.kind {
            MouseEventKind::WheelUp => self.scroll_view(-WHEEL_SCROLL_LINES),
            MouseEventKind::WheelDown => self.scroll_view(WHEEL_SCROLL_LINES),
            MouseEventKind::WheelLeft => self.scroll_view_horizontal(-WHEEL_SCROLL_COLUMNS),
            MouseEventKind::WheelRight => self.scroll_view_horizontal(WHEEL_SCROLL_COLUMNS),
            MouseEventKind::Press(MouseButton::Left) => {
                if mouse.row == 1 {
                    if let Some(index) = tab_index_at_column(
                        &self.tab_items(),
                        mouse.column.saturating_sub(1),
                        self.screen_size.0,
                    ) {
                        self.active = index;
                        self.search.close();
                        self.close_guard.reset();
                    }
                    return QuitDecision::Continue;
                }
                if let Some(position) = self.mouse_buffer_position(&mouse) {
                    let now = Instant::now();
                    let count = self
                        .last_click
                        .filter(|(at, previous, _)| {
                            *previous == position && now.duration_since(*at) <= MULTI_CLICK_INTERVAL
                        })
                        .map_or(1, |(_, _, count)| (count % 3) + 1);
                    self.last_click = Some((now, position, count));
                    match count {
                        2 => self.select_word_at(position),
                        3 => self.select_line_at(position.line),
                        _ => self.editor_mut().set_cursor_position(position),
                    }
                    self.drag_anchor = Some(
                        self.editor()
                            .selection
                            .map_or(position, |selection| selection.anchor),
                    );
                    self.follow_cursor = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let (Some(anchor), Some(position)) =
                    (self.drag_anchor, self.mouse_buffer_position(&mouse))
                {
                    self.editor_mut().select_range(anchor, position);
                    self.follow_cursor = true;
                }
            }
            MouseEventKind::Release(MouseButton::Left) => self.drag_anchor = None,
            // Middle/right buttons have no assigned behavior (ADR-0008 keeps
            // scope at click/drag/wheel).
            MouseEventKind::Press(_) | MouseEventKind::Drag(_) | MouseEventKind::Release(_) => {}
        }
        QuitDecision::Continue
    }

    /// Maps a 1-based mouse cell to a buffer position, excluding the tab bar
    /// (row 0) and the status line (last row).
    fn mouse_buffer_position(&self, mouse: &MouseEvent) -> Option<Position> {
        let x = mouse.column.saturating_sub(1);
        let y = mouse.row.saturating_sub(1);
        let (width, height) = self.screen_size;
        if y < 1 || y + 1 >= height {
            return None;
        }
        let document = self.active_document();
        document
            .view
            .screen_to_buffer(&document.editor, x, y, 1, width, self.wrap)
    }

    /// Wheel scroll: moves the viewport and detaches cursor following until
    /// the next keystroke or click (see `follow_cursor`).
    fn scroll_view(&mut self, delta: isize) {
        let (width, _) = self.screen_size;
        let wrap = self.wrap;
        let document = self.active_document_mut();
        document
            .view
            .scroll_lines(&document.editor, delta, width, wrap);
        self.follow_cursor = false;
    }

    fn scroll_view_horizontal(&mut self, delta: isize) {
        let wrap = self.wrap;
        self.active_document_mut().view.scroll_columns(delta, wrap);
        self.follow_cursor = false;
    }

    fn select_word_at(&mut self, position: Position) {
        let Some(line) = self.editor().buffer.line(position.line) else {
            return;
        };
        let graphemes = line.graphemes(true).collect::<Vec<_>>();
        if graphemes.is_empty() {
            return;
        }
        let at = position.grapheme.min(graphemes.len().saturating_sub(1));
        let class = |g: &str| {
            if g.chars().all(char::is_whitespace) {
                0
            } else if g.chars().all(|c| c.is_alphanumeric() || c == '_') {
                1
            } else {
                2
            }
        };
        let target = class(graphemes[at]);
        let mut start = at;
        while start > 0 && class(graphemes[start - 1]) == target {
            start -= 1;
        }
        let mut end = at + 1;
        while end < graphemes.len() && class(graphemes[end]) == target {
            end += 1;
        }
        self.editor_mut().select_range(
            Position::new(position.line, start),
            Position::new(position.line, end),
        );
    }

    fn select_line_at(&mut self, line: usize) {
        let last = self.editor().buffer.line_count().saturating_sub(1);
        let line = line.min(last);
        let end = if line < last {
            Position::new(line + 1, 0)
        } else {
            Position::new(line, self.editor().buffer.grapheme_count(line))
        };
        self.editor_mut().select_range(Position::new(line, 0), end);
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
        if self.capability_warning && detection.capabilities() != KeyboardCapabilities::modern() {
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
        self.follow_cursor = true;
        let sanitized = text.replace('\n', "");
        if self.palette.visible {
            self.palette.push_text(&sanitized);
        } else if self.prompt.visible {
            self.prompt.paste_text(&sanitized);
        } else if self.inspector.visible {
            // Observe-only: record that a paste happened without ever
            // exposing its contents (design decision 260712).
            self.inspector.record_paste(text.len());
        } else if self.search.visible {
            let editor = &mut self.documents[self.active].editor;
            self.search.paste_text(&sanitized, editor);
        } else if !self.block_if_readonly() {
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
        // Any keystroke re-attaches the viewport to the cursor after a wheel
        // scroll (GUI editor convention: typing jumps back to the caret).
        self.follow_cursor = true;
        if event == KeyEvent::plain(Key::F(1)) {
            self.toggle_palette();
            return QuitDecision::Continue;
        }

        if self.palette.visible
            && let Some(decision) = self.handle_palette_key(&event)
        {
            return decision;
        }

        if self.prompt.visible {
            return self.handle_prompt_key(&event);
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
            ResolveResult::Pending { .. } => {
                // The which-key overlay (drawn from `draw`, which re-resolves
                // the prefix) shows the candidates; no status message needed.
                self.pending_since = Some(Instant::now());
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
            Key::Char('p') if event.modifiers == Modifiers::none().with_ctrl() => {
                let count = filter_actions(&self.palette.query, self.resolver.bindings()).len();
                self.palette.move_selection(-1, count);
                Some(QuitDecision::Continue)
            }
            Key::Down if event.modifiers == Modifiers::none() => {
                let count = filter_actions(&self.palette.query, self.resolver.bindings()).len();
                self.palette.move_selection(1, count);
                Some(QuitDecision::Continue)
            }
            Key::Char('n') if event.modifiers == Modifiers::none().with_ctrl() => {
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
            Key::Char('u') if event.modifiers == Modifiers::none().with_ctrl() => {
                self.palette.clear_query();
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
                if self.block_if_readonly() {
                    return QuitDecision::Continue;
                }
                self.editor_mut().insert_text(&character.to_string());
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            Key::Enter if event.modifiers == Modifiers::none() => {
                if self.block_if_readonly() {
                    return QuitDecision::Continue;
                }
                self.editor_mut().insert_text("\n");
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            Key::Tab if event.modifiers == Modifiers::none() => {
                if self.block_if_readonly() {
                    return QuitDecision::Continue;
                }
                self.editor_mut().insert_text("\t");
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            _ => {}
        }
        QuitDecision::Continue
    }

    /// Shared guard for both literal-insertion paths (`handle_text_input`,
    /// `handle_paste_input`) and the dispatch-level mutating-action check
    /// (TASK-260712 Gate 1: large file protection). Sets the standard
    /// message and reports whether the caller must stop ("黙って壊れない":
    /// blocking always shows a message, never a silent no-op).
    fn block_if_readonly(&mut self) -> bool {
        if self.active_document().readonly {
            self.message = "read-only buffer (large file)".to_string();
            true
        } else {
            false
        }
    }

    fn dispatch(&mut self, action: EditorAction) -> QuitDecision {
        self.message.clear();
        // Any dispatched action other than a repeated file.save interposes
        // between the two stages of the mtime-conflict confirm, so it must
        // reset the guard (task spec: "他の action を挟んだら reset").
        if action != EditorAction::FileSave {
            self.save_conflict_guard.reset();
        }
        if is_mutating_action(action) && self.block_if_readonly() {
            return QuitDecision::Continue;
        }
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
            EditorAction::EditIndent => self.editor_mut().indent(),
            EditorAction::EditOutdent => self.editor_mut().outdent(),
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
            EditorAction::FileSave => {
                self.handle_file_save();
            }
            EditorAction::FileSaveAs => {
                self.open_save_as_prompt();
            }
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
            EditorAction::GoToLine => {
                self.palette.close();
                self.prompt.open(PromptPurpose::GoToLine, "Go to Line:", "");
            }
            EditorAction::InspectorOpen => {
                self.search.close();
                self.inspector.open();
                self.message = "inspect-key: press any key".to_string();
            }
            EditorAction::ViewToggleWrap => {
                self.wrap = !self.wrap;
                // top_segment is wrap-mode state; stale values would offset
                // the next wrap-on viewport, so clear it on every toggle.
                for document in &mut self.documents {
                    document.view.top_segment = 0;
                }
                self.message = if self.wrap {
                    "wrap: on".to_string()
                } else {
                    "wrap: off".to_string()
                };
            }
            EditorAction::ConfigOpenSettings => {
                self.open_config_document(config::settings_path(), config::SETTINGS_TEMPLATE)
            }
            EditorAction::ConfigOpenKeybindings => {
                self.open_config_document(config::keybindings_path(), config::KEYBINDINGS_TEMPLATE)
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
        if started.elapsed() < self.sequence_timeout {
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

    /// `file.save`: writes to the current path, opening the Save As prompt
    /// for an unnamed buffer and enforcing the two-stage confirm-then-force
    /// flow for a file that changed on disk (TASK-260712 Gate 1; mirrors
    /// `QuitGuard`).
    fn handle_file_save(&mut self) -> QuitDecision {
        let force = self.save_conflict_guard.should_force();
        match self.active_document_mut().save(force) {
            Ok(()) => {
                self.message = format!("saved {}", self.active_document().display_name());
                self.quit_guard.reset();
                self.close_guard.reset();
                self.save_conflict_guard.reset();
            }
            Err(SaveError::NoPath) => {
                self.save_conflict_guard.reset();
                self.open_save_as_prompt();
            }
            Err(SaveError::Conflict) => {
                self.save_conflict_guard.warn();
                self.message = "file changed on disk; save again to overwrite".to_string();
            }
            Err(SaveError::Readonly) => {
                // `dispatch` already blocks file.save on a readonly
                // document before reaching here; kept for defense in depth.
                self.save_conflict_guard.reset();
                self.message = "read-only buffer (large file)".to_string();
            }
            Err(SaveError::Io(error)) => {
                self.save_conflict_guard.reset();
                self.message = format!("save failed: {error}");
            }
        }
        QuitDecision::Continue
    }

    /// `file.saveAs`: opens the generic prompt overlay pre-filled with the
    /// current path (if any). Submission is handled by `perform_save_as`.
    fn open_save_as_prompt(&mut self) {
        self.search.close();
        self.palette.close();
        let initial = self
            .active_document()
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.prompt.open(PromptPurpose::SaveAs, "Save As:", initial);
        self.save_as_overwrite_confirm = None;
    }

    /// Feeds one key to the prompt overlay while it is visible. The overlay
    /// itself consumes every key (`PromptOutcome` always resolves), so this
    /// never falls through to the resolver/dispatch path.
    fn handle_prompt_key(&mut self, event: &KeyEvent) -> QuitDecision {
        match self.prompt.handle_key(event) {
            PromptOutcome::Continue => QuitDecision::Continue,
            PromptOutcome::Cancel => {
                self.prompt.close();
                self.message = "save as: cancelled".to_string();
                QuitDecision::Continue
            }
            PromptOutcome::Submit => self.submit_prompt(),
        }
    }

    /// Routes a submitted prompt to its purpose. Only `SaveAs` exists today.
    fn submit_prompt(&mut self) -> QuitDecision {
        let Some(purpose) = self.prompt.purpose else {
            self.prompt.close();
            return QuitDecision::Continue;
        };
        let input = self.prompt.input.trim().to_string();
        if input.is_empty() {
            self.message = "input cannot be empty".to_string();
            return QuitDecision::Continue;
        }
        match purpose {
            PromptPurpose::SaveAs => self.perform_save_as(PathBuf::from(input)),
            PromptPurpose::GoToLine => {
                match input.parse::<usize>() {
                    Ok(line) if line > 0 => {
                        let last = self.editor().buffer.line_count().max(1);
                        let target = line.min(last);
                        self.editor_mut()
                            .set_cursor_position(Position::new(target - 1, 0));
                        self.prompt.close();
                        self.follow_cursor = true;
                        self.message = if line > last {
                            format!("line {line} out of range; moved to {last}")
                        } else {
                            format!("line {line}")
                        };
                    }
                    _ => self.message = "line number must be a positive integer".to_string(),
                }
                QuitDecision::Continue
            }
        }
    }

    /// Save As target resolution: retargeting the document's own current
    /// path falls back to the normal save flow (so the mtime-conflict guard
    /// still applies); a genuinely different, already-existing path needs
    /// its own "file exists" confirmation before the first write ever
    /// happens (task spec: two-stage confirm, same shape as `QuitGuard`).
    fn perform_save_as(&mut self, path: PathBuf) -> QuitDecision {
        if self.active_document().path.as_ref() == Some(&path) {
            self.prompt.close();
            return self.handle_file_save();
        }

        if path.exists() && self.save_as_overwrite_confirm.as_deref() != Some(path.as_path()) {
            self.save_as_overwrite_confirm = Some(path);
            self.message = "file exists; enter again to overwrite".to_string();
            return QuitDecision::Continue;
        }

        self.save_as_overwrite_confirm = None;
        self.prompt.close();
        let document = self.active_document_mut();
        document.path = Some(path);
        // A new target has no prior mtime on record, and the "file exists"
        // step above (when it applied) already served as the overwrite
        // confirmation, so this write always goes through regardless of
        // whatever is currently on disk.
        document.saved_mtime = None;
        match document.save(true) {
            Ok(()) => {
                self.message = format!("saved {}", self.active_document().display_name());
                self.quit_guard.reset();
                self.close_guard.reset();
            }
            Err(error) => self.message = format!("save failed: {error}"),
        }
        QuitDecision::Continue
    }

    fn context(&self) -> EditorContext {
        EditorContext {
            editor_focus: !self.palette.visible && !self.search.visible && !self.prompt.visible,
            text_input_focus: !self.palette.visible && !self.search.visible && !self.prompt.visible,
            has_selection: self
                .editor()
                .selection
                .is_some_and(|selection| !selection.is_empty()),
            // TASK-260712 Gate 1: wires `Document::readonly` (large file
            // protection) into the `when`-clause boundary type so imported
            // bindings using `!isReadonly` (see vscode_when.rs) resolve
            // correctly. Enforcement itself lives in `dispatch`/
            // `handle_text_input`/`handle_paste_input`, not here — the
            // default binding table does not gate mutating actions on this
            // flag, so relying on it alone would silently do nothing rather
            // than show a message ("黙って壊れない").
            is_readonly: self.active_document().readonly,
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
            // F1 bypasses the prompt/search key-consuming branches entirely
            // (it is checked before them in `handle_key`), so this is the
            // one place that must close them itself — the rescue entry
            // point always wins (AGENTS.md).
            self.search.close();
            self.prompt.close();
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

    /// Opens (or focuses a freshly-created) config file as a new buffer for
    /// `config.openSettings`/`config.openKeybindings` (TASK-260711-19).
    ///
    /// `path` is `None` when `HOME`/`XDG_CONFIG_HOME` cannot be resolved
    /// (same fallback as `config::load()`), which must warn rather than
    /// panic. When the file does not exist yet, `template` is inserted as
    /// starting content so the buffer is never a blank surprise — but
    /// nothing touches disk until the user explicitly saves (buffer.new /
    /// unnamed-buffer precedent, AGENTS.md "黙って壊れない").
    fn open_config_document(&mut self, path: Option<PathBuf>, template: &str) {
        let Some(path) = path else {
            self.message = "HOME is not set; cannot open config file".to_string();
            return;
        };
        match Document::open(path) {
            Ok((mut document, load_info)) => {
                if load_info.is_new {
                    // A brand-new buffer already carries an implicit final
                    // newline (`TextBuffer::default()`'s `trailing_newline`,
                    // core/buffer.rs), so a template ending in its own `\n`
                    // would otherwise round-trip with a spurious blank last
                    // line once saved.
                    let template = template.strip_suffix('\n').unwrap_or(template);
                    document.editor.insert_text(template);
                    document.editor.commit_group();
                }
                self.documents.push(document);
                self.active = self.documents.len() - 1;
                self.search.close();
                self.close_guard.reset();
            }
            Err(error) => self.message = error.to_string(),
        }
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
            if elapsed >= self.sequence_timeout {
                0
            } else {
                (self.sequence_timeout - elapsed)
                    .as_millis()
                    .min(i32::MAX as u128) as i32
            }
        } else {
            IDLE_POLL_MS
        }
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

fn tab_index_at_column(tabs: &[TabItem], column: u16, width: u16) -> Option<usize> {
    let mut x = 0usize;
    let target = usize::from(column);
    let width = usize::from(width);
    for (index, tab) in tabs.iter().enumerate() {
        if index > 0 {
            x += 2;
        }
        if x >= width {
            break;
        }
        let len = ellipsize(&tab.label, width - x).chars().count();
        if target >= x && target < x + len {
            return Some(index);
        }
        x += len;
    }
    None
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

/// Actions that mutate the buffer or its persisted contents, blocked on a
/// read-only document (large-file protection, TASK-260712 Gate 1). Cursor
/// movement, selection, copy, search, buffer switching, palette/inspector,
/// and quit are deliberately excluded — none of them touch buffer contents
/// or disk.
fn is_mutating_action(action: EditorAction) -> bool {
    matches!(
        action,
        EditorAction::EditBackspace
            | EditorAction::EditDelete
            | EditorAction::EditDeleteWordLeft
            | EditorAction::EditDeleteToLineStart
            | EditorAction::EditInsertLineAfter
            | EditorAction::EditInsertLineBefore
            | EditorAction::EditMoveLinesUp
            | EditorAction::EditMoveLinesDown
            | EditorAction::EditIndent
            | EditorAction::EditOutdent
            | EditorAction::EditCut
            | EditorAction::EditPaste
            | EditorAction::EditUndo
            | EditorAction::EditRedo
            | EditorAction::ReplaceOpen
            | EditorAction::ReplaceNext
            | EditorAction::ReplaceAll
            | EditorAction::FileSave
            | EditorAction::FileSaveAs
    )
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

#[cfg(test)]
mod tests {
    use super::{
        EventLoop, PromptPurpose, QuitDecision, QuitGuard, TabItem, draw_tab_bar,
        format_intercept_warning, ghostty_intercept_warning,
    };
    use crate::{
        app::default_bindings::{Platform, bindings_for},
        app::file::LARGE_FILE_BYTES,
        highlight::ThemeChoice,
        input::{
            CapabilityDetection, CapabilityProbe, InputEvent, Key, KeyEvent,
            quirks::parse_ghostty_keybinds,
        },
        keymap::{EditorAction, EditorContext, ResolveResult, Resolver},
        ui::Screen,
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant, SystemTime},
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

    /// TASK-260711-19 testcase: `coda` with no path arguments must not error
    /// out — `run_editor` (src/app/mod.rs) forwards empty `paths` straight
    /// through to `open_many`, which is exercised directly here since it
    /// already owns the "no documents => push one unnamed buffer" fallback.
    #[test]
    fn open_many_with_empty_paths_opens_a_single_unnamed_buffer() {
        let event_loop =
            EventLoop::open_many(Vec::new(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert_eq!(event_loop.documents.len(), 1);
        assert_eq!(event_loop.active, 0);
        assert_eq!(event_loop.documents[0].path, None);
        assert_eq!(event_loop.active_document().display_name(), "[No Name]");
        assert!(!event_loop.active_document().is_modified());
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

    /// TASK-260712 Gate 1 testcase: `file.save` on an unnamed buffer now
    /// opens the Save As prompt instead of just reporting an error message.
    #[test]
    fn buffer_new_creates_unnamed_active_buffer_and_file_save_opens_save_as_prompt() {
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
        assert!(
            event_loop.prompt.visible,
            "unnamed-buffer save must open the Save As prompt"
        );
        assert_eq!(event_loop.prompt.purpose, Some(PromptPurpose::SaveAs));
        assert!(
            event_loop.prompt.input.is_empty(),
            "no current path to prefill"
        );

        let _ = std::fs::remove_file(path);
    }

    /// TASK-260711-18: `view.toggleWrap` flips the editor-wide wrap flag,
    /// reports the new state, and clears per-document wrap scroll state.
    #[test]
    fn toggle_wrap_flips_state_and_resets_top_segment() {
        let path = temp_path("toggle-wrap");
        std::fs::write(&path, b"text").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.set_wrap(true);
        event_loop.documents[0].view.top_segment = 3;

        event_loop.dispatch(EditorAction::ViewToggleWrap);
        assert!(!event_loop.wrap);
        assert_eq!(event_loop.message, "wrap: off");
        assert_eq!(event_loop.documents[0].view.top_segment, 0);

        event_loop.dispatch(EditorAction::ViewToggleWrap);
        assert!(event_loop.wrap);
        assert_eq!(event_loop.message, "wrap: on");

        let _ = std::fs::remove_file(path);
    }

    /// TASK-260711-19 testcase: `config.openSettings`/`config.openKeybindings`
    /// route through `open_config_document`, which is exercised directly here
    /// (rather than through the real `HOME`-derived path) so the test stays
    /// deterministic under parallel `cargo test` — same reasoning as the
    /// `NO_COLOR` tests in `import_cli.rs`.
    #[test]
    fn open_config_document_inserts_template_only_for_a_new_file() {
        let base = temp_path("open-config-base");
        std::fs::write(&base, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(base.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        let settings_path = temp_path("open-config-settings");
        let _ = std::fs::remove_file(&settings_path);

        event_loop.open_config_document(Some(settings_path.clone()), "template content\n");

        assert_eq!(event_loop.documents.len(), 2);
        assert_eq!(event_loop.active, 1);
        assert_eq!(buffer_text(&event_loop), "template content\n");
        assert!(
            event_loop.active_document().is_modified(),
            "template content is unsaved until the user explicitly saves"
        );

        let expected_name = event_loop.active_document().display_name();
        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(event_loop.message, format!("saved {expected_name}"));
        assert_eq!(
            std::fs::read_to_string(&settings_path).unwrap(),
            "template content\n"
        );

        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_file(&settings_path);
    }

    #[test]
    fn open_config_document_leaves_an_existing_files_content_untouched() {
        let base = temp_path("open-config-existing-base");
        std::fs::write(&base, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(base.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        let existing_path = temp_path("open-config-existing");
        std::fs::write(&existing_path, b"already customized\n").unwrap();

        event_loop.open_config_document(Some(existing_path.clone()), "template content\n");

        assert_eq!(buffer_text(&event_loop), "already customized\n");
        assert!(
            !event_loop.active_document().is_modified(),
            "opening an existing file must not mark it modified"
        );

        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_file(&existing_path);
    }

    #[test]
    fn open_config_document_without_home_reports_a_message_instead_of_panicking() {
        let base = temp_path("open-config-no-home");
        std::fs::write(&base, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(base.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.open_config_document(None, "template content\n");

        assert_eq!(event_loop.documents.len(), 1, "no buffer must be opened");
        assert_eq!(
            event_loop.message,
            "HOME is not set; cannot open config file"
        );

        let _ = std::fs::remove_file(&base);
    }

    /// TASK-260711-19 testcase: `edit.indent`/`edit.outdent` dispatch through
    /// to `EditorCore`; the table-driven behavior itself is covered in
    /// `core::editor`'s own tests, so this only checks the dispatch wiring.
    #[test]
    fn indent_and_outdent_actions_dispatch_to_the_editor() {
        let path = temp_path("indent-outdent-dispatch");
        std::fs::write(&path, b"foo\nbar").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.editor_mut().select_range(
            crate::core::position::Position::new(0, 0),
            crate::core::position::Position::new(1, 3),
        );
        event_loop.dispatch(EditorAction::EditIndent);
        assert_eq!(buffer_text(&event_loop), "    foo\n    bar");

        event_loop.dispatch(EditorAction::EditOutdent);
        assert_eq!(buffer_text(&event_loop), "foo\nbar");

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

    /// SPEC-0005 `[terminal] capability_warning = false`: the status-bar
    /// warning is suppressed, but detection itself still resolves so
    /// `:inspect-key` keeps its protocol line.
    #[test]
    fn capability_warning_off_suppresses_message_but_keeps_detection() {
        let path = temp_path("capability-warning-off");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.message.clear();
        event_loop.set_capability_warning(false);

        event_loop.capability_probe = Some(CapabilityProbe::arm(
            Instant::now() + Duration::from_millis(500),
        ));
        event_loop.handle_input_event(InputEvent::DeviceAttributes);

        assert_eq!(
            event_loop.capability_detection,
            Some(CapabilityDetection::LegacyDeviceAttributes)
        );
        assert_eq!(event_loop.message, "");

        let _ = std::fs::remove_file(path);
    }

    /// SPEC-0005 `[keymap] palette_key`: the configured chord replaces the
    /// ctrl+space convenience binding, the old chord stops opening the
    /// palette, and hardwired F1 keeps working (rescue rule, SPEC-0002).
    #[test]
    fn set_palette_key_rebinds_convenience_key_and_keeps_f1() {
        let path = temp_path("palette-key");
        std::fs::write(&path, b"abc").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.set_palette_key(crate::keymap::parse_key_chord("ctrl+k").unwrap());

        event_loop.handle_key(crate::keymap::parse_key_chord("ctrl+k").unwrap());
        assert!(event_loop.palette.visible, "configured chord opens palette");
        event_loop.handle_key(KeyEvent::plain(Key::Esc));
        assert!(!event_loop.palette.visible);

        event_loop.handle_key(crate::keymap::parse_key_chord("ctrl+space").unwrap());
        assert!(
            !event_loop.palette.visible,
            "replaced chord no longer opens palette"
        );

        event_loop.handle_key(KeyEvent::plain(Key::F(1)));
        assert!(event_loop.palette.visible, "F1 stays hardwired");

        let _ = std::fs::remove_file(path);
    }

    /// ADR-0008 mouse: click = cursor move (clearing selection), drag =
    /// selection from the press anchor, release = anchor cleared.
    #[test]
    fn mouse_click_and_drag_drive_cursor_and_selection() {
        use crate::input::{MouseButton, MouseEvent, MouseEventKind};

        let path = temp_path("mouse-click-drag");
        std::fs::write(&path, b"alpha\nbravo\ncharlie\n").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        let mouse = |kind, column, row| MouseEvent {
            kind,
            modifiers: crate::input::Modifiers::default(),
            column,
            row,
        };

        // Gutter is 4 cells; row 2 of the screen = buffer line 0 is the tab
        // bar... row indexing: mouse row is 1-based, row 1 = tab bar, row 2 =
        // buffer line 0. Column 6 = text cell 1.
        event_loop.handle_mouse_event(mouse(MouseEventKind::Press(MouseButton::Left), 6, 3));
        assert_eq!(
            event_loop.editor().cursor,
            crate::core::position::Position::new(1, 1),
            "click places the cursor"
        );
        assert!(event_loop.editor().selection.is_none());

        event_loop.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), 9, 3));
        let selection = event_loop.editor().selection.expect("drag selects");
        assert_eq!(
            selection.range(),
            (
                crate::core::position::Position::new(1, 1),
                crate::core::position::Position::new(1, 4)
            )
        );

        event_loop.handle_mouse_event(mouse(MouseEventKind::Release(MouseButton::Left), 9, 3));
        assert!(
            event_loop.drag_anchor.is_none(),
            "release clears the anchor"
        );

        // Tab bar and status line clicks change nothing.
        let before = event_loop.editor().cursor;
        event_loop.handle_mouse_event(mouse(MouseEventKind::Press(MouseButton::Left), 3, 1));
        event_loop.handle_mouse_event(mouse(MouseEventKind::Press(MouseButton::Left), 3, 24));
        assert_eq!(event_loop.editor().cursor, before);

        let _ = std::fs::remove_file(path);
    }

    /// ADR-0008 §3: if a terminal forwards Shift+drag instead of reserving it
    /// for native selection, coda consumes it without starting a selection.
    #[test]
    fn shift_modified_mouse_events_are_ignored_after_delivery() {
        use crate::input::{MouseButton, MouseEvent, MouseEventKind};

        let path = temp_path("mouse-shift-ignored");
        std::fs::write(&path, b"alpha\nbravo\n").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        let before = event_loop.editor().cursor;

        event_loop.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: crate::input::Modifiers::shift(),
            column: 6,
            row: 3,
        });

        assert_eq!(event_loop.editor().cursor, before);
        assert!(event_loop.editor().selection.is_none());

        let _ = std::fs::remove_file(path);
    }

    /// ADR-0008 wheel: scrolling moves the viewport without the cursor and
    /// detaches cursor-following; the next keystroke re-attaches it.
    #[test]
    fn wheel_scroll_detaches_viewport_until_next_keystroke() {
        use crate::input::{MouseEvent, MouseEventKind};

        let path = temp_path("mouse-wheel");
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        std::fs::write(&path, text).unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::WheelDown,
            modifiers: crate::input::Modifiers::default(),
            column: 1,
            row: 3,
        });
        assert_eq!(event_loop.active_document().view.top_line, 3);
        assert_eq!(event_loop.editor().cursor.line, 0, "cursor did not move");
        assert!(!event_loop.follow_cursor);

        // Drawing while detached must not snap the viewport back to the cursor.
        let mut screen = Screen::new(80, 24);
        event_loop.draw(&mut screen);
        assert_eq!(event_loop.active_document().view.top_line, 3);

        // A keystroke re-attaches, and the next draw follows the cursor again.
        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert!(event_loop.follow_cursor);
        let mut screen = Screen::new(80, 24);
        event_loop.draw(&mut screen);
        assert_eq!(event_loop.active_document().view.top_line, 0);

        let _ = std::fs::remove_file(path);
    }

    /// Backlog P1 `:which-key`: while a sequence prefix is pending, `draw`
    /// shows the candidate continuations in an overlay above the status line.
    #[test]
    fn pending_sequence_draws_which_key_candidates() {
        use crate::keymap::{Binding, Source, parse_key_sequence};

        let path = temp_path("which-key-draw");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        let bindings = vec![Binding::new(
            parse_key_sequence("ctrl+k ctrl+u").unwrap(),
            EditorAction::CursorUp,
            None,
            Source::User,
        )];
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), bindings, ThemeChoice::Dark).unwrap();

        event_loop.handle_key(crate::keymap::parse_key_chord("ctrl+k").unwrap());
        let mut screen = Screen::new(80, 24);
        event_loop.draw(&mut screen);

        let all_rows: Vec<String> = (0..screen.height()).map(|y| row_text(&screen, y)).collect();
        assert!(
            all_rows
                .iter()
                .any(|row| row.contains("Ctrl+U") && row.contains("cursor.up")),
            "which-key overlay lists the continuation:\n{}",
            all_rows.join("\n")
        );

        // Completing the sequence resolves and the overlay disappears.
        event_loop.handle_key(crate::keymap::parse_key_chord("ctrl+u").unwrap());
        let mut screen = Screen::new(80, 24);
        event_loop.draw(&mut screen);
        let all_rows: Vec<String> = (0..screen.height()).map(|y| row_text(&screen, y)).collect();
        assert!(
            !all_rows.iter().any(|row| row.contains("cursor.up")),
            "overlay cleared after the sequence resolved"
        );

        let _ = std::fs::remove_file(path);
    }

    /// SPEC-0005 `[keymap] sequence_timeout_ms`: a shortened timeout fires
    /// the pending exact match without waiting for the 800ms default.
    #[test]
    fn set_sequence_timeout_controls_when_the_exact_match_fires() {
        use crate::keymap::{Binding, Source, parse_key_sequence};

        let path = temp_path("sequence-timeout");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        let bindings = vec![
            Binding::new(
                parse_key_sequence("ctrl+k").unwrap(),
                EditorAction::CursorDown,
                None,
                Source::User,
            ),
            Binding::new(
                parse_key_sequence("ctrl+k ctrl+u").unwrap(),
                EditorAction::CursorUp,
                None,
                Source::User,
            ),
        ];
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), bindings, ThemeChoice::Dark).unwrap();
        event_loop.set_sequence_timeout(Duration::from_millis(1));

        event_loop.handle_key(crate::keymap::parse_key_chord("ctrl+k").unwrap());
        assert!(event_loop.pending_since.is_some(), "sequence is pending");
        std::thread::sleep(Duration::from_millis(5));
        event_loop.handle_sequence_timeout();

        assert!(event_loop.pending_since.is_none());
        assert_eq!(
            event_loop.editor().cursor.line,
            1,
            "exact match (cursor.down) fired after the shortened timeout"
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

    /// TASK-260712 Gate 1 testcases: Save As (prompt + `file.saveAs`).

    #[test]
    fn file_save_as_prompt_submits_and_saves_to_a_new_path() {
        let base = temp_path("save-as-base");
        std::fs::write(&base, b"base").unwrap();
        let target = temp_path("save-as-target");
        let _ = std::fs::remove_file(&target);
        let mut event_loop =
            EventLoop::open(base.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.editor_mut().insert_text("!");
        event_loop.dispatch(EditorAction::FileSaveAs);
        assert!(event_loop.prompt.visible);
        assert_eq!(event_loop.prompt.purpose, Some(PromptPurpose::SaveAs));
        assert_eq!(event_loop.prompt.input, base.display().to_string());

        // Replace the prefilled current path with the target path.
        for _ in 0..event_loop.prompt.input.chars().count() {
            event_loop.handle_key(KeyEvent::plain(Key::Backspace));
        }
        for character in target.display().to_string().chars() {
            event_loop.handle_key(KeyEvent::plain(Key::Char(character)));
        }
        assert_eq!(
            event_loop.handle_key(KeyEvent::plain(Key::Enter)),
            QuitDecision::Continue
        );

        assert!(!event_loop.prompt.visible);
        assert_eq!(event_loop.active_document().path.as_ref(), Some(&target));
        assert_eq!(
            event_loop.active_document().display_name(),
            target.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"!base");
        assert!(
            event_loop.message.starts_with("saved "),
            "{}",
            event_loop.message
        );
        assert!(!event_loop.active_document().is_modified());

        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn file_save_as_to_an_existing_different_path_requires_a_second_enter_to_overwrite() {
        let base = temp_path("save-as-overwrite-base");
        std::fs::write(&base, b"base").unwrap();
        let target = temp_path("save-as-overwrite-target");
        std::fs::write(&target, b"target-original").unwrap();
        let mut event_loop =
            EventLoop::open(base.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();
        event_loop.editor_mut().insert_text("!");

        event_loop.dispatch(EditorAction::FileSaveAs);
        for _ in 0..event_loop.prompt.input.chars().count() {
            event_loop.handle_key(KeyEvent::plain(Key::Backspace));
        }
        for character in target.display().to_string().chars() {
            event_loop.handle_key(KeyEvent::plain(Key::Char(character)));
        }

        event_loop.handle_key(KeyEvent::plain(Key::Enter));
        assert_eq!(event_loop.message, "file exists; enter again to overwrite");
        assert!(
            event_loop.prompt.visible,
            "prompt stays open for the confirmation"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"target-original");

        event_loop.handle_key(KeyEvent::plain(Key::Enter));
        assert!(!event_loop.prompt.visible);
        assert_eq!(std::fs::read(&target).unwrap(), b"!base");

        let _ = std::fs::remove_file(&base);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn file_save_as_esc_cancels_without_changing_the_document() {
        let path = temp_path("save-as-cancel");
        std::fs::write(&path, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.dispatch(EditorAction::FileSaveAs);
        assert!(event_loop.prompt.visible);

        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert_eq!(
            event_loop.handle_key(KeyEvent::plain(Key::Esc)),
            QuitDecision::Continue
        );

        assert!(!event_loop.prompt.visible);
        assert_eq!(event_loop.active_document().path.as_ref(), Some(&path));
        assert_eq!(event_loop.message, "save as: cancelled");

        let _ = std::fs::remove_file(&path);
    }

    /// TASK-260712 Gate 1 testcases: external-change detection (mtime).

    #[test]
    fn file_save_reports_conflict_on_external_change_and_second_save_forces_overwrite() {
        let path = temp_path("save-conflict");
        std::fs::write(&path, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        // Simulate an external editor having touched the file: pin the
        // in-memory saved_mtime to a deliberately stale value instead of
        // racing a real external writer against filesystem mtime
        // resolution (same reasoning as document.rs's own conflict test).
        event_loop.documents[0].saved_mtime = Some(SystemTime::UNIX_EPOCH);
        event_loop.editor_mut().insert_text("!");

        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(
            event_loop.message,
            "file changed on disk; save again to overwrite"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"base",
            "conflicting save must not write"
        );

        event_loop.dispatch(EditorAction::FileSave);
        assert!(event_loop.message.starts_with("saved "));
        assert_eq!(std::fs::read(&path).unwrap(), b"!base");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_save_conflict_guard_resets_when_another_action_is_dispatched() {
        let path = temp_path("save-conflict-reset");
        std::fs::write(&path, b"base").unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        event_loop.documents[0].saved_mtime = Some(SystemTime::UNIX_EPOCH);
        event_loop.editor_mut().insert_text("!");
        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(
            event_loop.message,
            "file changed on disk; save again to overwrite"
        );

        // Any other action interposed between the two save attempts must
        // reset the guard, so the next save warns again instead of
        // silently forcing an overwrite.
        event_loop.dispatch(EditorAction::CursorRight);
        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(
            event_loop.message,
            "file changed on disk; save again to overwrite"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"base");

        let _ = std::fs::remove_file(&path);
    }

    /// TASK-260712 Gate 1 testcases: large file protection.

    #[test]
    fn large_file_over_threshold_opens_readonly_and_blocks_edits_and_save_but_allows_navigation() {
        let path = temp_path("large-file-over");
        std::fs::write(&path, vec![b'a'; (LARGE_FILE_BYTES + 1) as usize]).unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert!(event_loop.active_document().readonly);
        assert!(event_loop.context().is_readonly);
        assert!(
            event_loop
                .message
                .contains("large file (>10 MB); opened read-only"),
            "{}",
            event_loop.message
        );

        let original_len = event_loop.editor().buffer.to_bytes().len();

        // Literal insertion is blocked with a message, not a silent no-op.
        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert_eq!(event_loop.editor().buffer.to_bytes().len(), original_len);
        assert_eq!(event_loop.message, "read-only buffer (large file)");

        // A mutating action is blocked with the same message.
        event_loop.dispatch(EditorAction::EditBackspace);
        assert_eq!(event_loop.message, "read-only buffer (large file)");

        // Save is blocked too.
        event_loop.dispatch(EditorAction::FileSave);
        assert_eq!(event_loop.message, "read-only buffer (large file)");

        // Navigation, view.toggleWrap, and the palette still work.
        assert_eq!(
            event_loop.dispatch(EditorAction::CursorRight),
            QuitDecision::Continue
        );
        event_loop.dispatch(EditorAction::ViewToggleWrap);
        assert_eq!(event_loop.message, "wrap: on");
        assert_eq!(
            event_loop.dispatch(EditorAction::PaletteOpen),
            QuitDecision::Continue
        );
        assert!(event_loop.palette.visible);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn large_file_at_exact_threshold_stays_editable() {
        let path = temp_path("large-file-at-threshold");
        std::fs::write(&path, vec![b'a'; LARGE_FILE_BYTES as usize]).unwrap();
        let mut event_loop =
            EventLoop::open(path.clone(), Vec::new(), Vec::new(), ThemeChoice::Dark).unwrap();

        assert!(!event_loop.active_document().readonly);
        assert!(!event_loop.context().is_readonly);

        event_loop.handle_key(KeyEvent::plain(Key::Char('x')));
        assert!(
            String::from_utf8(event_loop.editor().buffer.to_bytes())
                .unwrap()
                .starts_with('x'),
            "editing must succeed at exactly the threshold"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Confirms the readonly gate operates at the dispatch layer, not by
    /// relying on the default binding table to reference `isReadonly`:
    /// `alt+z` (editorFocus-gated, no readonly clause) must still resolve
    /// while `is_readonly` is set.
    #[test]
    fn is_readonly_context_still_resolves_editor_focus_bindings_like_wrap_toggle() {
        let resolver = Resolver::new(bindings_for(Platform::MacOs));
        let context = EditorContext {
            is_readonly: true,
            ..EditorContext::default()
        };
        assert_eq!(
            resolver.resolve(&["alt+z".parse().unwrap()], &context),
            ResolveResult::Matched(EditorAction::ViewToggleWrap)
        );
    }
}

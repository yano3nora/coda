//! `:inspect-key` live mode: an in-editor, observe-only overlay that shows
//! the next keystroke's raw bytes, decoded `KeyEvent`, and resolve result
//! (TASK-260711-17). Modeled after `search_overlay.rs`.
//!
//! Two properties matter for correctness:
//!
//! - **Observation must never mutate editor state.** While the overlay is
//!   visible, `EventLoop::handle_key` must consume every key here instead of
//!   letting it reach the resolver's dispatch path (see `event_loop.rs`).
//!   This module only *simulates* a resolve via `Resolver::resolve` (which is
//!   already side-effect free) to describe what *would* happen.
//! - **The winning binding's `source`/`when` are derived without touching
//!   the `Resolver` API.** `Resolver::resolve` only returns the matched
//!   `EditorAction`, not which `Binding` produced it, so `winning_binding`
//!   below re-runs the same selection rule (source priority, then `when`
//!   specificity, then definition order — see `keymap::resolver`) over
//!   `resolver.bindings()` restricted to a single-key exact match.

use std::collections::VecDeque;

use crate::{
    input::{
        CapabilityDetection, Key, KeyEvent, Modifiers, escape_bytes,
        quirks::{QuirkEffect, TerminalQuirk},
    },
    keymap::{Binding, EditorContext, ResolveResult, Resolver, Source},
    ui::{Screen, Style},
};

/// How many recent keystrokes stay visible at once (design decision 260712).
const MAX_RECORDS: usize = 3;

/// One inspected event: the raw bytes that produced it (chunk-level
/// approximation — see `push_raw`) plus its pre-formatted display lines.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InspectRecord {
    pub raw: Vec<u8>,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InspectorOverlay {
    pub visible: bool,
    /// Most recent record first (index 0 == newest).
    records: VecDeque<InspectRecord>,
    /// Bytes accumulated since the last record, from raw `read(2)` chunks
    /// the event loop hands us while visible. Cleared into a record's `raw`
    /// field whenever a key or paste event is observed.
    pending_raw: Vec<u8>,
}

impl InspectorOverlay {
    pub fn open(&mut self) {
        self.visible = true;
        self.records.clear();
        self.pending_raw.clear();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.pending_raw.clear();
    }

    /// Appends a raw `read(2)` chunk. The event loop calls this once per
    /// read while the overlay is visible, before decoding; the bytes are
    /// claimed by whichever event they decode into.
    pub fn push_raw(&mut self, bytes: &[u8]) {
        self.pending_raw.extend_from_slice(bytes);
    }

    /// Records a decoded key event: raw bytes accumulated so far, the
    /// decoded event, its (simulated) resolve result, and any matching
    /// Ghostty quirk note.
    pub fn record_key(
        &mut self,
        event: &KeyEvent,
        resolver: &Resolver,
        context: &EditorContext,
        quirks: &[TerminalQuirk],
    ) {
        let raw = std::mem::take(&mut self.pending_raw);
        let lines = format_record(&raw, event, resolver, context, quirks);
        self.push_record(InspectRecord { raw, lines });
    }

    /// Records a bracketed paste as a single summary line. Pasted content is
    /// intentionally never shown (design decision 260712), so the raw bytes
    /// accumulated for it are discarded rather than stored.
    pub fn record_paste(&mut self, byte_len: usize) {
        self.pending_raw.clear();
        self.push_record(InspectRecord {
            raw: Vec::new(),
            lines: format_paste_record(byte_len),
        });
    }

    fn push_record(&mut self, record: InspectRecord) {
        self.records.push_front(record);
        while self.records.len() > MAX_RECORDS {
            self.records.pop_back();
        }
    }
}

/// Formats one key record's display lines: raw bytes, decoded key, resolve
/// result, and an optional Ghostty-quirk note. Pure over its inputs so it is
/// unit-testable without a terminal or a running event loop.
pub fn format_record(
    raw: &[u8],
    event: &KeyEvent,
    resolver: &Resolver,
    context: &EditorContext,
    quirks: &[TerminalQuirk],
) -> Vec<String> {
    let mut lines = vec![
        format!("raw: {} (\"{}\")", hex_bytes(raw), escape_bytes(raw)),
        format!("key: {event}"),
        resolved_line(event, resolver, context),
    ];
    if let Some(note) = quirk_note(event, quirks) {
        lines.push(note);
    }
    lines
}

/// Formats a paste record. Only the byte count is shown — see module docs.
pub fn format_paste_record(byte_len: usize) -> Vec<String> {
    vec![format!("paste ({byte_len} bytes)")]
}

fn resolved_line(event: &KeyEvent, resolver: &Resolver, context: &EditorContext) -> String {
    match resolver.resolve(std::slice::from_ref(event), context) {
        ResolveResult::Matched(action) => {
            let detail = winning_binding(resolver.bindings(), event, context)
                .map(format_binding_detail)
                .unwrap_or_default();
            format!("resolved: {action}{detail}")
        }
        ResolveResult::Pending { candidates, .. } => {
            format!("resolved: pending ({})", format_candidate_list(&candidates))
        }
        ResolveResult::NoMatch => "resolved: no binding".to_string(),
    }
}

fn format_candidate_list(candidates: &[(Vec<KeyEvent>, crate::keymap::EditorAction)]) -> String {
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

fn format_binding_detail(binding: &Binding) -> String {
    let source = source_name(binding.source);
    match &binding.when {
        Some(when) => format!(" [{source}, {when}]"),
        None => format!(" [{source}]"),
    }
}

/// Re-derives which `Binding` the resolver would have picked for a single
/// exact-match key event, so its `source`/`when` can be shown. Mirrors
/// `Resolver::resolve`'s selection rule (context filter, then
/// `(source.priority(), term_count(), definition index)` tie-break) without
/// requiring any change to the `Resolver` API itself.
fn winning_binding<'a>(
    bindings: &'a [Binding],
    event: &KeyEvent,
    context: &EditorContext,
) -> Option<&'a Binding> {
    bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| {
            binding.keys.len() == 1
                && binding.keys[0] == *event
                && binding_matches_context(binding, context)
        })
        .max_by_key(|(index, binding)| (binding.source.priority(), binding.term_count(), *index))
        .map(|(_, binding)| binding)
}

fn binding_matches_context(binding: &Binding, context: &EditorContext) -> bool {
    binding.when.as_ref().is_none_or(|when| when.eval(context))
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Rescue => "rescue",
        Source::User => "user",
        Source::Imported => "imported",
        Source::Default => "default",
    }
}

/// Notes when the observed event matches the translated target of a known
/// Ghostty quirk (e.g. the user pressed `Opt+Left`, Ghostty rewrote it to
/// `Alt+B`, and `Alt+B` is what we just decoded) — the "terminal is
/// intercepting this" explanation the dogfood feedback asked for.
fn quirk_note(event: &KeyEvent, quirks: &[TerminalQuirk]) -> Option<String> {
    quirks.iter().find_map(|quirk| match &quirk.effect {
        QuirkEffect::Translated { events, .. } if events.first() == Some(event) => Some(format!(
            "note: Ghostty rewrites {} to this key ({})",
            quirk.trigger, quirk.source_line
        )),
        _ => None,
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns `true` when `event` should close the overlay (bare `Esc`,
/// per design decision 260712) rather than being recorded.
pub fn is_close_key(event: &KeyEvent) -> bool {
    event.key == Key::Esc && event.modifiers == Modifiers::none()
}

/// Palette-style boxed modal. Draws nothing when the overlay is hidden or
/// the screen is too small, matching `draw_palette`'s own size guard.
///
/// `capability_detection` is `None` while keyboard capability negotiation is
/// still pending (TASK-260712-16) and `Some` once the event loop has
/// resolved it; either way it becomes the body's first line so users can see
/// *why* modifier keys behave the way they do without leaving the overlay.
pub fn draw_inspector(
    screen: &mut Screen,
    overlay: &InspectorOverlay,
    capability_detection: Option<CapabilityDetection>,
) {
    if !overlay.visible || screen.height() < 6 || screen.width() < 12 {
        return;
    }

    let box_x = 2;
    let box_width = screen.width().saturating_sub(4);
    let inner_width = usize::from(box_width.saturating_sub(4));
    let max_rows = usize::from(screen.height().saturating_sub(6)).clamp(1, 20);

    let body = body_lines(overlay, capability_detection);
    let shown = body.len().min(max_rows);
    let box_top = 1;
    let box_height = shown as u16 + 3; // top border + body rows + bottom border

    let dim = Style {
        reverse: false,
        dim: true,
        fg: None,
    };
    let normal = Style::default();

    for row in 0..box_height {
        let y = box_top + row;
        let line = if row == 0 {
            frame_line("╭", "─", "╮", " Inspect Key ", usize::from(box_width))
        } else if row == box_height - 1 {
            frame_line("╰", "─", "╯", " Esc to close ", usize::from(box_width))
        } else {
            format!("│{}│", " ".repeat(usize::from(box_width).saturating_sub(2)))
        };
        screen.put_str(box_x, y, &line, dim);
    }

    for (row, line) in body.iter().take(shown).enumerate() {
        let clipped = clip_to_width(line, inner_width);
        screen.put_str(box_x + 2, box_top + 1 + row as u16, &clipped, normal);
    }
}

/// The protocol status line, prepended to the overlay body ahead of any
/// records (TASK-260712-16). `None` means detection is still in flight —
/// this state only exists transiently during startup, since `CapabilityProbe`
/// always resolves within its deadline.
fn protocol_line(capability_detection: Option<CapabilityDetection>) -> String {
    let status = capability_detection.map_or_else(
        || "detecting…".to_string(),
        |detection| detection.description(),
    );
    format!("protocol: {status}")
}

/// Newest record first, with a blank separator line between records so
/// multi-line records stay visually grouped. The protocol status always
/// leads the body, independent of whether any keystroke has been recorded
/// yet.
fn body_lines(
    overlay: &InspectorOverlay,
    capability_detection: Option<CapabilityDetection>,
) -> Vec<String> {
    let mut lines = vec![protocol_line(capability_detection)];

    if overlay.records.is_empty() {
        lines.push("press any key to inspect it".to_string());
        return lines;
    }

    for record in &overlay.records {
        // Blank separator before every record, including the first: it also
        // separates the leading protocol line from the record list.
        lines.push(String::new());
        lines.extend(record.lines.iter().cloned());
    }
    lines
}

fn frame_line(left: &str, fill: &str, right: &str, title: &str, width: usize) -> String {
    let inner = width.saturating_sub(2);
    let title = clip_to_width(title, inner);
    let title_len = title.chars().count();
    format!(
        "{left}{title}{}{right}",
        fill.repeat(inner.saturating_sub(title_len))
    )
}

fn clip_to_width(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::{InspectorOverlay, body_lines, format_paste_record, format_record, is_close_key};
    use crate::{
        app::default_bindings::{Platform, bindings_for},
        input::{
            CapabilityDetection, Key, KeyEvent, Modifiers, escape_bytes,
            quirks::parse_ghostty_keybinds,
        },
        keymap::{EditorContext, Resolver},
    };

    fn macos_resolver() -> Resolver {
        Resolver::new(bindings_for(Platform::MacOs))
    }

    #[test]
    fn format_record_shows_matched_action_with_source_and_when() {
        let resolver = macos_resolver();
        let context = EditorContext::default();
        let event = KeyEvent::new(Key::Char('b'), Modifiers::alt());
        let raw = [0x1b, b'b'];

        let lines = format_record(&raw, &event, &resolver, &context, &[]);

        assert_eq!(
            lines[0],
            format!("raw: 0x1b 0x62 (\"{}\")", escape_bytes(&raw))
        );
        assert_eq!(lines[1], format!("key: {event}"));
        assert_eq!(lines[2], "resolved: cursor.wordLeft [default, editorFocus]");
        assert_eq!(lines.len(), 3, "no quirk note without matching quirks");
    }

    #[test]
    fn format_record_reports_no_binding_for_unbound_key() {
        let resolver = macos_resolver();
        let context = EditorContext::default();
        // alt+ctrl+shift+k is not assigned anywhere in the default table.
        let event = KeyEvent::new(Key::Char('k'), Modifiers::alt().with_ctrl().with_shift());

        let lines = format_record(&[0x1b], &event, &resolver, &context, &[]);

        assert_eq!(lines[2], "resolved: no binding");
    }

    #[test]
    fn format_record_notes_ghostty_translation_when_event_matches_a_quirk_target() {
        let resolver = macos_resolver();
        let context = EditorContext::default();
        let quirks = parse_ghostty_keybinds("keybind = alt+arrow_left=esc:b\n");
        let event = KeyEvent::new(Key::Char('b'), Modifiers::alt());

        let lines = format_record(&[0x1b, b'b'], &event, &resolver, &context, &quirks);

        assert_eq!(
            lines.last().unwrap(),
            "note: Ghostty rewrites Alt+Left to this key (alt+arrow_left=esc:b)"
        );
    }

    #[test]
    fn format_paste_record_hides_content() {
        assert_eq!(
            format_paste_record(42),
            vec!["paste (42 bytes)".to_string()]
        );
    }

    #[test]
    fn is_close_key_matches_only_unmodified_esc() {
        assert!(is_close_key(&KeyEvent::plain(Key::Esc)));
        assert!(!is_close_key(&KeyEvent::new(Key::Esc, Modifiers::alt())));
        assert!(!is_close_key(&KeyEvent::plain(Key::Char('a'))));
    }

    #[test]
    fn overlay_open_close_resets_state_and_keeps_last_three_records() {
        let resolver = macos_resolver();
        let context = EditorContext::default();
        let mut overlay = InspectorOverlay::default();
        overlay.open();
        assert!(overlay.visible);

        for character in ['a', 'b', 'c', 'd'] {
            overlay.push_raw(&[character as u8]);
            overlay.record_key(
                &KeyEvent::plain(Key::Char(character)),
                &resolver,
                &context,
                &[],
            );
        }

        assert_eq!(overlay.records.len(), 3, "only the last 3 keystrokes stay");
        assert_eq!(overlay.records[0].raw, vec![b'd'], "newest first");
        assert_eq!(overlay.records[2].raw, vec![b'b']);

        overlay.close();
        assert!(!overlay.visible);
        overlay.push_raw(b"z");
        overlay.open();
        assert!(
            overlay.records.is_empty(),
            "re-opening clears stale records"
        );
    }

    #[test]
    fn overlay_paste_record_hides_bytes_and_clears_pending_raw() {
        let mut overlay = InspectorOverlay::default();
        overlay.open();
        overlay.push_raw(b"\x1b[200~secret\x1b[201~");

        overlay.record_paste(6);

        assert_eq!(overlay.records.len(), 1);
        assert_eq!(overlay.records[0].raw, Vec::<u8>::new());
        assert_eq!(
            overlay.records[0].lines,
            vec!["paste (6 bytes)".to_string()]
        );
    }

    /// Table-driven per TASK-260712-16 testcases: the overlay body's leading
    /// protocol line for all 4 states the design calls out.
    #[test]
    fn body_lines_show_the_four_protocol_states() {
        let overlay = InspectorOverlay::default();
        let cases: &[(&str, Option<CapabilityDetection>, &str)] = &[
            ("still detecting", None, "protocol: detecting…"),
            (
                "modern kitty CSI u",
                Some(CapabilityDetection::KittyFlags(1)),
                "protocol: kitty CSI u (flags=1)",
            ),
            (
                "legacy via DA1",
                Some(CapabilityDetection::LegacyDeviceAttributes),
                "protocol: legacy (DA1 answered, no CSI ?u reply)",
            ),
            (
                "legacy via timeout",
                Some(CapabilityDetection::LegacyTimeout),
                "protocol: legacy (query timed out)",
            ),
        ];

        for (name, detection, expected) in cases {
            let lines = body_lines(&overlay, *detection);
            assert_eq!(lines[0], *expected, "{name}");
        }
    }

    #[test]
    fn body_lines_keep_protocol_line_ahead_of_records() {
        let resolver = macos_resolver();
        let context = EditorContext::default();
        let mut overlay = InspectorOverlay::default();
        overlay.open();
        overlay.push_raw(b"a");
        overlay.record_key(&KeyEvent::plain(Key::Char('a')), &resolver, &context, &[]);

        let lines = body_lines(&overlay, Some(CapabilityDetection::KittyFlags(1)));
        assert_eq!(lines[0], "protocol: kitty CSI u (flags=1)");
        assert_eq!(lines[1], "", "blank separator before the first record");
        assert_eq!(
            lines[3],
            format!("key: {}", KeyEvent::plain(Key::Char('a')))
        );
    }
}

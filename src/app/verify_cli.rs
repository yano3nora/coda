//! `coda keymap verify`: interactive chord deliverability measurement.
//!
//! ADR-0007 decision 2(c): protocol negotiation and quirk knowledge can only
//! warn — whether a chord actually arrives can only be *measured*. This
//! command asks the user to press each imported chord and records what the
//! terminal really delivered.
//!
//! The decision core is the pure `VerifySession` state machine (unit-tested
//! without a terminal); the interactive loop is a thin blocking-read shell in
//! the style of `input::inspect_key`, not the editor event loop.

use std::io::{self, Read, Write};
use std::{env, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    input::{
        InputEvent, Key, KeyEvent, KeyboardProtocolGuard, Modifiers, RawModeGuard,
        drain_input_events, flush_pending_escape, poll_readable,
        quirks::{self, TerminalQuirk, suggest_ghostty_fix},
    },
    keymap::{Binding, Source, format_key_for_config},
};

use super::{
    config,
    import_cli::{config_base_dir, write_parented},
};

/// How long a lone ESC may sit in the buffer before it is flushed as the
/// skip key (mirrors the editor's idle poll granularity).
const ESC_FLUSH_POLL_MS: i32 = 100;

/// One chord to verify, with the action names it is bound to (possibly via
/// different sequences) so the prompt can say why the chord matters.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct VerifyTarget {
    pub(crate) chord: KeyEvent,
    pub(crate) actions: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ChordOutcome {
    /// The pressed key decoded to exactly the expected chord.
    Delivered,
    /// Something else arrived — the terminal consumed or rewrote the chord.
    /// Carries what was actually decoded so the report can show the delta.
    Mismatch(KeyEvent),
    /// The user skipped this chord (Esc).
    Skipped,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum FeedResult {
    /// The event was recorded (or ignored because verification is done).
    Recorded,
    /// The user aborted with Ctrl+C; remaining targets stay untested.
    Aborted,
}

pub(crate) struct VerifySession {
    targets: Vec<VerifyTarget>,
    outcomes: Vec<Option<ChordOutcome>>,
    index: usize,
}

const VERIFY_STATE_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
struct TerminalIdentity {
    program: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifyState {
    schema_version: u32,
    terminal: TerminalIdentity,
    chords: Vec<StoredChordOutcome>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredChordOutcome {
    chord: String,
    outcome: String,
}

fn current_terminal_identity() -> TerminalIdentity {
    TerminalIdentity {
        program: env::var("TERM_PROGRAM").unwrap_or_else(|_| "unknown".to_string()),
        version: env::var("TERM_PROGRAM_VERSION").unwrap_or_else(|_| "unknown".to_string()),
    }
}

impl VerifySession {
    /// Collects unique chords from imported bindings plus every Super chord
    /// in defaults/user bindings. Super delivery is reserved per key, so
    /// even built-in Cmd gestures need measurement. Sequences contribute
    /// each chord individually.
    pub(crate) fn from_bindings(bindings: &[Binding]) -> Self {
        let mut targets: Vec<VerifyTarget> = Vec::new();
        for binding in bindings.iter().filter(|b| {
            b.source == Source::Imported || b.keys.iter().any(|key| key.modifiers.contains_super())
        }) {
            for chord in &binding.keys {
                let action = binding.action.to_string();
                match targets.iter_mut().find(|target| target.chord == *chord) {
                    Some(target) => {
                        if !target.actions.contains(&action) {
                            target.actions.push(action);
                        }
                    }
                    None => targets.push(VerifyTarget {
                        chord: chord.clone(),
                        actions: vec![action],
                    }),
                }
            }
        }
        let outcomes = vec![None; targets.len()];
        Self {
            targets,
            outcomes,
            index: 0,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.index >= self.targets.len()
    }

    pub(crate) fn current(&self) -> Option<&VerifyTarget> {
        self.targets.get(self.index)
    }

    /// Feeds one decoded key event. Equality against the expected chord is
    /// checked *before* the Esc/Ctrl+C control keys, so a binding whose chord
    /// IS Esc or Ctrl+C can still be verified as delivered.
    pub(crate) fn feed(&mut self, event: KeyEvent) -> FeedResult {
        let Some(target) = self.targets.get(self.index) else {
            return FeedResult::Recorded;
        };
        let outcome = if event == target.chord {
            ChordOutcome::Delivered
        } else if event == KeyEvent::plain(Key::Esc) {
            ChordOutcome::Skipped
        } else if event == KeyEvent::new(Key::Char('c'), Modifiers::ctrl()) {
            return FeedResult::Aborted;
        } else {
            ChordOutcome::Mismatch(event)
        };
        self.outcomes[self.index] = Some(outcome);
        self.index += 1;
        FeedResult::Recorded
    }

    /// One line per target plus totals; ends with the `--cmd=ctrl` escape
    /// hatch hint when a super chord did not arrive (ADR-0007 §3).
    ///
    /// `quirks` is the detected Ghostty keybind table (possibly empty, e.g.
    /// outside Ghostty or on non-macOS): when a `Mismatch` target's expected
    /// chord matches a detected quirk's trigger, the machine-generated
    /// Ghostty config fix (TASK-260713) rides along as two extra lines right
    /// after the `MISMATCH` line.
    pub(crate) fn summary_lines(&self, quirks: &[TerminalQuirk]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut delivered = 0;
        let mut mismatched = 0;
        let mut skipped = 0;
        let mut untested = 0;
        let mut super_undelivered = false;

        for (target, outcome) in self.targets.iter().zip(&self.outcomes) {
            let chord = format_key_for_config(std::slice::from_ref(&target.chord));
            let actions = target.actions.join(", ");
            match outcome {
                Some(ChordOutcome::Delivered) => {
                    delivered += 1;
                    lines.push(format!("delivered  {chord}  ({actions})"));
                }
                Some(ChordOutcome::Mismatch(actual)) => {
                    mismatched += 1;
                    super_undelivered |= target.chord.modifiers.contains_super();
                    lines.push(format!(
                        "MISMATCH   {chord}  arrived as {actual}  ({actions})"
                    ));
                    if let Some(suggestion) =
                        matching_quirk(quirks, &target.chord).and_then(suggest_ghostty_fix)
                    {
                        lines.push(format!("fix: {}", suggestion.config_line));
                        lines.push(format!("     {}", suggestion.reason));
                    }
                }
                Some(ChordOutcome::Skipped) => {
                    skipped += 1;
                    lines.push(format!("skipped    {chord}  ({actions})"));
                }
                None => {
                    untested += 1;
                    lines.push(format!("untested   {chord}  ({actions})"));
                }
            };
        }

        lines.push(format!(
            "total: {delivered} delivered, {mismatched} mismatched, {skipped} skipped, {untested} untested"
        ));
        if super_undelivered {
            lines.push(
                "hint: a Cmd/Super chord did not arrive — the terminal may reserve it; \
                 consider re-importing with --cmd=ctrl (ADR-0007)"
                    .to_string(),
            );
        }
        lines
    }
}

/// Finds the detected quirk (if any) whose trigger is the expected chord, so
/// a `Mismatch` report line can be paired with `suggest_ghostty_fix`.
fn matching_quirk<'a>(quirks: &'a [TerminalQuirk], chord: &KeyEvent) -> Option<&'a TerminalQuirk> {
    quirks.iter().find(|quirk| quirk.trigger == *chord)
}

/// CLI entry: loads bindings, runs the interactive loop, prints and persists
/// the report. Returns the process exit code.
pub(crate) fn run_keymap_verify() -> i32 {
    let loaded = config::load();
    // Verify default Cmd bindings as well as imported bindings. Per-key
    // reservation means Cmd+S succeeding says nothing about Cmd+C.
    let mut bindings = super::default_bindings::bindings_with_ctrl_c_quit(loaded.ctrl_c_quits);
    bindings.extend(loaded.user_bindings);
    let session = VerifySession::from_bindings(&bindings);
    if session.is_empty() {
        println!("keymap verify: no imported or Cmd/Super bindings found");
        return 0;
    }

    let session = match run_interactive(session) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("keymap verify failed: {error} (an interactive terminal is required)");
            return 1;
        }
    };

    // Cheap and already guarded by TERM_PROGRAM=ghostty (input/quirks.rs
    // module docs); calling it once here is fine even outside Ghostty, where
    // it just yields an empty Vec.
    let quirks = quirks::detect();
    let lines = session.summary_lines(&quirks);
    println!();
    for line in &lines {
        println!("{line}");
    }

    let base_dir = config_base_dir();
    match persist_results(base_dir.as_deref(), &lines, &session) {
        Ok(()) => {
            let path = base_dir
                .expect("persist_report succeeded only when a config base exists")
                .join("import-reports")
                .join("latest-verify.txt");
            println!("report written to {}", path.display());
            0
        }
        Err(error) => {
            eprintln!("could not write verify report: {error}");
            1
        }
    }
}

/// Persists a completed measurement. A missing config root is an error rather
/// than a silent success because the command promises a reusable report.
fn persist_report(base_dir: Option<&Path>, lines: &[String]) -> io::Result<()> {
    let base_dir = base_dir.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_CONFIG_HOME nor HOME is set",
        )
    })?;
    let path = base_dir.join("import-reports").join("latest-verify.txt");
    let mut body = lines.join("\n");
    body.push('\n');
    write_parented(&path, body.as_bytes())
}

fn persist_results(
    base_dir: Option<&Path>,
    lines: &[String],
    session: &VerifySession,
) -> io::Result<()> {
    persist_report(base_dir, lines)?;
    let base_dir = base_dir.expect("persist_report validated the config root");
    let state = VerifyState {
        schema_version: VERIFY_STATE_SCHEMA,
        terminal: current_terminal_identity(),
        chords: session
            .targets
            .iter()
            .zip(&session.outcomes)
            .map(|(target, outcome)| StoredChordOutcome {
                chord: format_key_for_config(std::slice::from_ref(&target.chord)),
                outcome: match outcome {
                    Some(ChordOutcome::Delivered) => "delivered",
                    Some(ChordOutcome::Mismatch(_)) => "mismatched",
                    Some(ChordOutcome::Skipped) => "skipped",
                    None => "untested",
                }
                .to_string(),
            })
            .collect(),
    };
    let mut body =
        serde_json::to_vec_pretty(&state).map_err(|error| io::Error::other(error.to_string()))?;
    body.push(b'\n');
    write_parented(&base_dir.join("keymap-verification.json"), &body)
}

/// Loads mismatched chords only when the complete terminal identity matches.
/// Corrupt/stale state is ignored with a warning so startup always succeeds.
pub(crate) fn load_disabled_chords(base_dir: &Path, warnings: &mut Vec<String>) -> Vec<KeyEvent> {
    let path = base_dir.join("keymap-verification.json");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(format!("{}: {error}; ignored verify state", path.display()));
            return Vec::new();
        }
    };
    let state: VerifyState = match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(error) => {
            warnings.push(format!("{}: {error}; ignored verify state", path.display()));
            return Vec::new();
        }
    };
    if state.schema_version != VERIFY_STATE_SCHEMA || state.terminal != current_terminal_identity()
    {
        return Vec::new();
    }
    state
        .chords
        .into_iter()
        .filter(|entry| entry.outcome == "mismatched")
        .filter_map(|entry| match crate::keymap::parse_key_chord(&entry.chord) {
            Ok(chord) => Some(chord),
            Err(error) => {
                warnings.push(format!(
                    "{}: invalid verified chord {:?} ({error}); ignored",
                    path.display(),
                    entry.chord
                ));
                None
            }
        })
        .collect()
}

/// Raw-mode read loop. Restores the terminal before returning so the caller
/// can print the summary through normal stdout.
fn run_interactive(mut session: VerifySession) -> io::Result<VerifySession> {
    let mut raw = RawModeGuard::enable_stdin()?;
    let mut stdout = io::stdout().lock();
    // Same protocol posture as the editor: without the kitty push, a modern
    // terminal would deliver fewer distinguishable chords here than in the
    // editor itself and the measurement would be wrong.
    let protocol = KeyboardProtocolGuard::push(&mut stdout)?;

    write_raw_line(
        &mut stdout,
        "keymap verify: press each chord as prompted. Esc skips, Ctrl+C quits.",
    )?;
    prompt(&mut stdout, &session)?;

    let mut stdin = io::stdin().lock();
    let mut byte_buffer = Vec::new();
    let mut chunk = [0_u8; 128];
    'outer: while !session.is_done() {
        // Poll instead of blocking on read: a bare ESC (the skip key) is
        // decoder-Incomplete because it could be the start of an escape
        // sequence. Only a timeout can disambiguate it — same reasoning as
        // the editor event loop's `flush_pending_escape` path.
        let mut keys: Vec<KeyEvent> = Vec::new();
        if poll_readable(libc::STDIN_FILENO, ESC_FLUSH_POLL_MS)? {
            let read = stdin.read(&mut chunk)?;
            if read == 0 {
                write_raw_line(&mut stdout, "stdin closed; stopping")?;
                break;
            }
            byte_buffer.extend_from_slice(&chunk[..read]);
            keys.extend(
                drain_input_events(&mut byte_buffer)
                    .into_iter()
                    .filter_map(|event| match event {
                        InputEvent::Key(key) => Some(key),
                        _ => None, // capability replies etc. are not presses
                    }),
            );
        } else if let Some(key) = flush_pending_escape(&mut byte_buffer) {
            keys.push(key);
        }
        for key in keys {
            let before = session.index;
            match session.feed(key) {
                FeedResult::Aborted => {
                    write_raw_line(&mut stdout, "aborted")?;
                    break 'outer;
                }
                FeedResult::Recorded => {
                    if let Some(Some(outcome)) = session.outcomes.get(before) {
                        let text = match outcome {
                            ChordOutcome::Delivered => "  -> delivered".to_string(),
                            ChordOutcome::Mismatch(actual) => {
                                format!("  -> arrived as {actual}")
                            }
                            ChordOutcome::Skipped => "  -> skipped".to_string(),
                        };
                        write_raw_line(&mut stdout, &text)?;
                    }
                    if session.is_done() {
                        break 'outer;
                    }
                    prompt(&mut stdout, &session)?;
                }
            }
        }
    }

    drop(protocol);
    raw.restore()?;
    Ok(session)
}

fn prompt(stdout: &mut impl Write, session: &VerifySession) -> io::Result<()> {
    let Some(target) = session.current() else {
        return Ok(());
    };
    let line = format!(
        "[{}/{}] press: {}  ({})",
        session.index + 1,
        session.targets.len(),
        format_key_for_config(std::slice::from_ref(&target.chord)),
        target.actions.join(", ")
    );
    write_raw_line(stdout, &line)
}

/// Raw mode disables `\n` -> `\r\n` output translation; write CRLF manually
/// (same reasoning as `input::inspect_key`).
fn write_raw_line(stdout: &mut impl Write, line: &str) -> io::Result<()> {
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::{
        ChordOutcome, FeedResult, StoredChordOutcome, TerminalIdentity, VERIFY_STATE_SCHEMA,
        VerifySession, VerifyState, current_terminal_identity, load_disabled_chords,
        persist_report,
    };
    use crate::{
        input::quirks::parse_ghostty_keybinds,
        keymap::{Binding, EditorAction, Source, parse_key_sequence},
    };

    fn binding(keys: &str, action: EditorAction, source: Source) -> Binding {
        Binding::new(parse_key_sequence(keys).unwrap(), action, None, source)
    }

    fn chord(text: &str) -> crate::input::KeyEvent {
        crate::keymap::parse_key_chord(text).unwrap()
    }

    #[test]
    fn from_bindings_collects_unique_imported_chords_including_sequence_parts() {
        let bindings = vec![
            binding("cmd+s", EditorAction::FileSave, Source::Imported),
            // sequence: both chords become targets; ctrl+k deduped below
            binding("ctrl+k ctrl+u", EditorAction::CursorUp, Source::Imported),
            binding("ctrl+k ctrl+d", EditorAction::CursorDown, Source::Imported),
            // non-imported sources are not verified
            binding("ctrl+q", EditorAction::AppQuit, Source::Default),
            binding("ctrl+j", EditorAction::CursorDown, Source::User),
        ];

        let session = VerifySession::from_bindings(&bindings);

        let chords: Vec<_> = session
            .targets
            .iter()
            .map(|target| target.chord.clone())
            .collect();
        assert_eq!(
            chords,
            vec![
                chord("cmd+s"),
                chord("ctrl+k"),
                chord("ctrl+u"),
                chord("ctrl+d")
            ]
        );
        assert_eq!(
            session.targets[1].actions,
            vec!["cursor.up".to_string(), "cursor.down".to_string()],
            "shared prefix chord lists every action it serves"
        );
    }

    /// Table-driven transitions: delivered / mismatch / skip / abort, plus
    /// the "expected chord IS a control key" precedence rule.
    #[test]
    fn feed_records_outcomes_and_control_keys() {
        let bindings = vec![
            binding("cmd+s", EditorAction::FileSave, Source::Imported),
            binding("cmd+z", EditorAction::EditUndo, Source::Imported),
            binding("cmd+a", EditorAction::SelectionAll, Source::Imported),
        ];
        let mut session = VerifySession::from_bindings(&bindings);

        assert_eq!(session.feed(chord("cmd+s")), FeedResult::Recorded);
        assert_eq!(session.outcomes[0], Some(ChordOutcome::Delivered));

        // terminal rewrote cmd+z into ctrl+z
        assert_eq!(session.feed(chord("ctrl+z")), FeedResult::Recorded);
        assert_eq!(
            session.outcomes[1],
            Some(ChordOutcome::Mismatch(chord("ctrl+z")))
        );

        assert_eq!(session.feed(chord("escape")), FeedResult::Recorded);
        assert_eq!(session.outcomes[2], Some(ChordOutcome::Skipped));
        assert!(session.is_done());
    }

    #[test]
    fn ctrl_c_aborts_leaving_the_rest_untested() {
        let bindings = vec![
            binding("cmd+s", EditorAction::FileSave, Source::Imported),
            binding("cmd+z", EditorAction::EditUndo, Source::Imported),
        ];
        let mut session = VerifySession::from_bindings(&bindings);

        assert_eq!(session.feed(chord("ctrl+c")), FeedResult::Aborted);
        assert_eq!(session.outcomes, vec![None, None]);

        let summary = session.summary_lines(&[]);
        assert!(
            summary
                .iter()
                .any(|line| line.contains("0 delivered, 0 mismatched, 0 skipped, 2 untested")),
            "{summary:?}"
        );
    }

    /// A binding whose chord IS Esc or Ctrl+C must match before the control
    /// keys are interpreted.
    #[test]
    fn expected_control_chords_verify_as_delivered() {
        let bindings = vec![
            binding("escape", EditorAction::PaletteOpen, Source::Imported),
            binding("ctrl+c", EditorAction::EditCopy, Source::Imported),
        ];
        let mut session = VerifySession::from_bindings(&bindings);

        assert_eq!(session.feed(chord("escape")), FeedResult::Recorded);
        assert_eq!(session.outcomes[0], Some(ChordOutcome::Delivered));
        assert_eq!(session.feed(chord("ctrl+c")), FeedResult::Recorded);
        assert_eq!(session.outcomes[1], Some(ChordOutcome::Delivered));
    }

    #[test]
    fn summary_suggests_cmd_ctrl_when_a_super_chord_is_undelivered() {
        let bindings = vec![binding("cmd+z", EditorAction::EditUndo, Source::Imported)];
        let mut session = VerifySession::from_bindings(&bindings);
        session.feed(chord("ctrl+z"));

        let summary = session.summary_lines(&[]);
        assert!(
            summary.iter().any(|line| line.contains("--cmd=ctrl")),
            "{summary:?}"
        );

        // ...but not when everything arrived
        let mut session = VerifySession::from_bindings(&bindings);
        session.feed(chord("cmd+z"));
        let summary = session.summary_lines(&[]);
        assert!(
            !summary.iter().any(|line| line.contains("--cmd=ctrl")),
            "{summary:?}"
        );
    }

    /// Mismatch report lines get the machine-generated Ghostty fix appended
    /// when the expected chord matches a detected quirk's trigger
    /// (TASK-260713).
    ///
    /// The three asserted lines are quoted verbatim in README.md's
    /// "Terminal setup (macOS)" example — if this test changes, update the
    /// README block too.
    #[test]
    fn summary_appends_suggested_fix_when_a_mismatch_matches_a_detected_quirk() {
        let bindings = vec![binding("cmd+z", EditorAction::EditUndo, Source::Imported)];
        let mut session = VerifySession::from_bindings(&bindings);
        session.feed(chord("ctrl+z")); // terminal delivered ctrl+z instead of cmd+z

        let quirks = parse_ghostty_keybinds("keybind = super+z=undo\n");
        let summary = session.summary_lines(&quirks);

        let mismatch_index = summary
            .iter()
            .position(|line| line.starts_with("MISMATCH"))
            .expect("a mismatch line is present");
        assert_eq!(
            summary[mismatch_index],
            "MISMATCH   cmd+z  arrived as Ctrl+Z  (edit.undo)"
        );
        assert_eq!(summary[mismatch_index + 1], "fix: keybind = super+z=unbind");
        assert_eq!(
            summary[mismatch_index + 2],
            "     once unbound the key falls through and is encoded via the kitty keyboard \
             protocol. Apply, restart Ghostty (reload is not enough), then re-run \
             `coda keymap verify`."
        );
    }

    /// A mismatch with no corresponding quirk (e.g. outside Ghostty) must not
    /// grow extra lines.
    #[test]
    fn summary_omits_fix_lines_when_no_quirk_matches_the_mismatch() {
        let bindings = vec![binding("cmd+z", EditorAction::EditUndo, Source::Imported)];
        let mut session = VerifySession::from_bindings(&bindings);
        session.feed(chord("ctrl+z"));

        let summary = session.summary_lines(&[]);
        assert!(!summary.iter().any(|line| line.starts_with("fix:")));
    }

    #[test]
    fn persist_report_rejects_a_missing_config_root() {
        let error = persist_report(None, &["summary".to_string()]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn verify_state_disables_only_mismatches_for_matching_terminal() {
        let base = std::env::temp_dir().join(format!(
            "coda-verify-state-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::create_dir_all(&base).unwrap();
        let state = VerifyState {
            schema_version: VERIFY_STATE_SCHEMA,
            terminal: current_terminal_identity(),
            chords: vec![
                StoredChordOutcome {
                    chord: "cmd+c".to_string(),
                    outcome: "mismatched".to_string(),
                },
                StoredChordOutcome {
                    chord: "cmd+s".to_string(),
                    outcome: "delivered".to_string(),
                },
            ],
        };
        std::fs::write(
            base.join("keymap-verification.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let mut warnings = Vec::new();
        assert_eq!(
            load_disabled_chords(&base, &mut warnings),
            vec![chord("cmd+c")]
        );
        assert!(warnings.is_empty());

        let stale = VerifyState {
            terminal: TerminalIdentity {
                program: "different".to_string(),
                version: "0".to_string(),
            },
            ..state
        };
        std::fs::write(
            base.join("keymap-verification.json"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert!(load_disabled_chords(&base, &mut warnings).is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }
}

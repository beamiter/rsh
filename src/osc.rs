/// OSC (Operating System Command) escape sequences for terminal integration.
///
/// Supported sequences:
/// - OSC 7:    Report current working directory (file:// URI)
/// - OSC 9:    Desktop notification (Windows Terminal, ConEmu)
/// - OSC 133:  Semantic prompt / shell integration (iTerm2, VS Code, WezTerm, Kitty)
/// - OSC 777:  Terminal notification (iTerm2, Kitty)
/// - OSC 1337: iTerm2 proprietary (CurrentDir)
/// - OSC 0/2:  Window/tab title
use std::env;

/// Percent-encode a path for use in file:// URIs (OSC 7).
/// Encodes everything except unreserved characters and `/`.
fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'-' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

// ── OSC 7: Current Working Directory ──────────────────────────

/// Emit OSC 7 to report the current working directory to the terminal.
/// Format: `\x1b]7;file://hostname/path\x1b\\`
///
/// Supported by: iTerm2, WezTerm, Kitty, foot, GNOME Terminal, Windows Terminal.
pub fn report_cwd(hostname: &str) {
    if let Ok(cwd) = env::current_dir() {
        let encoded = percent_encode_path(&cwd.to_string_lossy());
        eprint!("\x1b]7;file://{}{}\x1b\\", hostname, encoded);
    }
}

// ── OSC 1337: iTerm2 CurrentDir ───────────────────────────────

/// Emit OSC 1337 CurrentDir for iTerm2.
/// Format: `\x1b]1337;CurrentDir=path\x07`
pub fn report_cwd_iterm2() {
    if let Ok(cwd) = env::current_dir() {
        eprint!("\x1b]1337;CurrentDir={}\x07", cwd.to_string_lossy());
    }
}

// ── OSC 0/2: Window Title ─────────────────────────────────────

/// Emit OSC 2 to set the window/tab title.
/// Format: `\x1b]2;title\x07`
///
/// Supported by virtually all terminal emulators.
pub fn set_title(title: &str) {
    eprint!("\x1b]2;{}\x07", title);
}

// ── OSC 133: Semantic Prompt (Shell Integration) ──────────────
//
// These markers allow terminals to understand the structure of
// shell interaction: where the prompt is, where user input ends,
// where command output begins and ends. This enables features like
// click-to-jump between prompts, select command output, scroll to
// previous command, and per-command exit status indicators.
//
// Lifecycle per command:
//   133;A  →  [prompt displayed]  →  [user types]  →  133;B
//          →  133;C  →  [command output]  →  133;D;exitcode
//
// Supported by: iTerm2, VS Code terminal, WezTerm, Kitty, foot.

/// Emit OSC 133;A — Prompt start marker.
/// Call this immediately before rendering the prompt.
pub fn prompt_start() {
    eprint!("\x1b]133;A\x07");
}

/// Emit OSC 133;B — Command start marker.
/// Call this after the user presses Enter, before command execution.
pub fn command_start() {
    eprint!("\x1b]133;B\x07");
}

/// Emit OSC 133;C — Command output start marker.
/// Call this just before the command's output begins.
pub fn command_output_start() {
    eprint!("\x1b]133;C\x07");
}

/// Emit OSC 133;D — Command finished marker with exit code.
/// Call this after the command completes.
pub fn command_finished(exit_code: i32) {
    eprint!("\x1b]133;D;{}\x07", exit_code);
}

// ── OSC 7770: rsh Session ID ─────────────────────────────────

/// Emit OSC 7770 to report the rsh session ID to the terminal emulator.
/// Format: `\x1b]7770;session_id\x07`
///
/// This is a custom rsh-specific OSC used by jterm4 to associate
/// a terminal pane with a persistent session.
pub fn report_session_id(session_id: &str) {
    eprint!("\x1b]7770;{}\x07", session_id);
}

// ── OSC 9: Desktop Notification ───────────────────────────────

/// Emit OSC 9 desktop notification.
/// Format: `\x1b]9;message\x07`
///
/// Supported by: Windows Terminal, ConEmu.
pub fn notify_osc9(message: &str) {
    eprint!("\x1b]9;{}\x07", message);
}

// ── OSC 777: Terminal Notification ────────────────────────────

/// Emit OSC 777 terminal notification.
/// Format: `\x1b]777;notify;summary;body\x07`
///
/// Supported by: iTerm2, Kitty, rxvt-unicode.
pub fn notify_osc777(summary: &str, body: &str) {
    eprint!("\x1b]777;notify;{};{}\x07", summary, body);
}

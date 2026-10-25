//! Integration tests for foto's CLI surface.
//!
//! These spawn the real binary. None of them need a compositor:
//!
//! * `--list`        — pure desktop-entry scanning + printing
//! * `--print-config`— prints the built-in example KDL and exits
//! * `--dmenu`       — the only interactive one: items come from stdin
//!   (a pipe), key events come from `/dev/tty`. We run foto under
//!   `script -qec` (util-linux), which provides a pty as the controlling
//!   terminal and forwards our keystrokes into it — so the TUI fallback
//!   is driven exactly like a human would, without any GUI.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_foto")
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "foto-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const FIXTURE_APP: &str = "\
[Desktop Entry]
Type=Application
Name=Fixture Editor
GenericName=Text Editor
Exec=fixture-editor %u
Icon=fixture-editor
Terminal=false
";

const FIXTURE_SECOND: &str = "\
[Desktop Entry]
Type=Application
Name=Fixture Viewer
Exec=fixture-viewer
Icon=fixture-viewer
NoDisplay=true
";

#[test]
fn list_exits_zero_on_fixture_data_dir() {
    let xdg = temp_dir("list");
    let apps = xdg.join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("editor.desktop"), FIXTURE_APP).unwrap();
    // NoDisplay entries are skipped by design — this one must NOT show up
    std::fs::write(apps.join("hidden.desktop"), FIXTURE_SECOND).unwrap();

    let out = Command::new(bin())
        .arg("--list")
        // pin the XDG search path to the fixture; XDG_DATA_DIRS="" (set but
        // empty) drops the system data dirs, so only the fixture is scanned
        .env("XDG_DATA_HOME", &xdg)
        .env("XDG_DATA_DIRS", "")
        .output()
        .expect("failed to run foto --list");

    assert!(out.status.success(), "--list must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Fixture Editor"), "stdout was: {stdout}");
    assert!(
        !stdout.contains("Fixture Viewer"),
        "NoDisplay entries must be skipped; stdout was: {stdout}"
    );
    // GenericName is shown as a subtitle after an em dash
    assert!(stdout.contains("Text Editor"), "stdout was: {stdout}");

    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn print_config_emits_parseable_kdl() {
    let out = Command::new(bin())
        .arg("--print-config")
        .output()
        .expect("failed to run foto --print-config");

    assert!(out.status.success(), "--print-config must exit 0");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.trim().is_empty(), "config output must not be empty");

    // the emitted document must be valid KDL with the documented keys
    let doc: kdl::KdlDocument = text
        .parse()
        .expect("--print-config output must be parseable KDL");
    let names: Vec<&str> = doc.nodes().iter().map(|n| n.name().value()).collect();
    for key in [
        "font",
        "font-size",
        "icon-theme",
        "terminal",
        "open-with",
        "layout",
    ] {
        assert!(names.contains(&key), "missing config key {key:?} in output");
    }
}

/// Path to `script(1)` (util-linux) — the pty provider for the dmenu tests.
fn script_path() -> PathBuf {
    for p in ["/usr/bin/script", "/bin/script", "/usr/local/bin/script"] {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p;
        }
    }
    panic!("script(1) (util-linux) is required for the dmenu integration tests");
}

/// Run `foto --dmenu` under a pty (via `script`), feed `keys` as keystrokes
/// and return (exit code, stdout of foto). The menu items are piped on
/// stdin; the keys reach foto through the pty that `script` provides as
/// its controlling terminal, exactly like a real terminal session.
///
/// The keystrokes are deliberately delivered ~2s after startup: the pty
/// starts in canonical mode, where ICRNL would turn a `\r` into `\n`
/// before foto enables raw mode — and crossterm maps `\n` to Enter only
/// OUTSIDE raw mode. Once foto's TUI is up, raw mode is active, `\r`
/// stays `\r` and arrives as a proper Enter keypress.
fn run_dmenu_with_keys(keys: &[u8]) -> (i32, Vec<u8>) {
    use std::io::Write;
    use std::process::Stdio;
    use std::time::Duration;

    let work = temp_dir("dmenu");
    let out_file = work.join("stdout.txt");

    // foto's stdout is redirected into a file so we can assert on exactly
    // what foto *printed* (as opposed to what the TUI *drew* on the pty)
    let inner = format!(
        "printf 'alpha\\nbeta\\ngamma\\n' | '{}' --dmenu > {}",
        bin(),
        out_file.display()
    );

    let mut child = Command::new(script_path())
        .args(["-qec", &inner, "/dev/null"])
        .stdin(Stdio::piped())
        .env("TERM", "xterm-256color")
        .current_dir(&work)
        .spawn()
        .expect("failed to run script(1)");

    // watchdog: if the session ever stalls, kill it so the suite fails
    // fast with a clear status instead of blocking forever
    let pid = child.id().to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        let _ = Command::new("kill").args(["-9", &pid]).output();
    });

    // forward the keystrokes into the pty after foto's TUI is up
    let mut stdin = child.stdin.take().unwrap();
    let keys = keys.to_vec();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2000));
        let _ = stdin.write_all(&keys);
        let _ = stdin.flush();
        // dropping closes script's stdin -> EOF after the keys
    });

    let status = child.wait().expect("failed to wait for script(1)");
    // None = killed by a signal (the watchdog, if the session ever stalls)
    let code = status.code().unwrap_or(-1);
    let stdout = std::fs::read(&out_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&work);
    (code, stdout)
}

/// Everything foto prints after leaving the alternate screen. The TUI's
/// drawings live *inside* the alternate-screen session and end with
/// CSI ?1049l (crossterm's LeaveAlternateScreen); whatever comes after
/// that is foto's real stdout — the dmenu selection echo.
fn printed_after_leave_alt_screen(stdout: &[u8]) -> &[u8] {
    const LEAVE: &[u8] = b"\x1b[?1049l";
    match stdout.windows(LEAVE.len()).rposition(|w| w == LEAVE) {
        Some(pos) => &stdout[pos + LEAVE.len()..],
        None => stdout,
    }
}

/// Remove ANSI escape sequences (CSI ... final-byte) plus any lone ESC
/// that crossterm/ratatui leave behind when the terminal session closes
/// (e.g. the cursor-show on Terminal drop), so only *visible text* stays.
fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                // CSI: skip parameters until the final byte (0x40..=0x7e)
                let mut j = i + 2;
                while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() {
                    i = j + 1;
                    continue;
                }
            }
            i += 1; // lone ESC — drop it
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[test]
fn dmenu_enter_echoes_selected_line() {
    // type "be" (only "beta" matches), then Enter (\r)
    let (code, stdout) = run_dmenu_with_keys(b"be\r");
    assert_eq!(code, 0, "dmenu must exit 0 after a selection");
    let printed = strip_ansi(printed_after_leave_alt_screen(&stdout));
    assert_eq!(
        printed,
        b"beta\n",
        "the selected line must be echoed to stdout; raw stdout: {:?}",
        String::from_utf8_lossy(&stdout)
    );
}

#[test]
fn dmenu_escape_prints_nothing() {
    let (code, stdout) = run_dmenu_with_keys(b"\x1b");
    assert_eq!(code, 0, "Esc must exit cleanly");
    let printed = strip_ansi(printed_after_leave_alt_screen(&stdout));
    assert!(
        printed.iter().all(|b| b.is_ascii_whitespace()),
        "Esc must print nothing (got {:?} after the alternate screen)",
        String::from_utf8_lossy(&printed)
    );
}

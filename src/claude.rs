//! Bridge to Claude Code's status line (documented input JSON; `rate_limits` is present for
//! claude.ai Pro/Max/Ultra subscribers). Claude Code runs the status line command often; the
//! bridge forwards the rate limits to the running app and then runs the user's original
//! status line command, so an existing status line keeps working.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FLAG: &str = "--claude-statusline";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// 0..100 (can exceed 100 when over the limit).
    pub used_percentage: f64,
    /// Unix epoch seconds.
    pub resets_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    /// When we received it (Unix seconds).
    #[serde(default)]
    pub received_at: i64,
}

/// Extract the rate limits from a status line input JSON. None if absent (API-key users, or
/// before the first response of a session).
pub fn parse_statusline(input: &str) -> Option<Usage> {
    let v: Value = serde_json::from_str(input).ok()?;
    let rl = v.get("rate_limits")?;
    let window = |k: &str| -> Option<Window> { serde_json::from_value(rl.get(k)?.clone()).ok() };
    let u = Usage {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
        received_at: 0,
    };
    (u.five_hour.is_some() || u.seven_day.is_some()).then_some(u)
}

/// `notch-utils --claude-statusline [-- 'original command']`. Never starts the GUI.
pub fn run_bridge(args: &[String]) -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(1024 * 1024).read_to_end(&mut input);
    if let Some(u) = parse_statusline(&String::from_utf8_lossy(&input))
        && let Ok(json) = serde_json::to_string(&u)
    {
        // Not running: nothing to update. Errors must never break the status line.
        let _ = crate::ipc::send(&[crate::ipc::Command::Usage(json)]);
    }
    match args
        .iter()
        .position(|a| a == "--")
        .and_then(|i| args.get(i + 1))
    {
        Some(original) => run_original(original, &input),
        None => 0,
    }
}

fn run_original(command: &str, input: &[u8]) -> i32 {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return 1 };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    match child.wait_with_output() {
        Ok(out) => {
            let _ = std::io::stdout().write_all(&out.stdout);
            out.status.code().unwrap_or(1)
        }
        Err(_) => 1,
    }
}

// ---------- ~/.claude/settings.json ----------

pub fn settings_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().join(".claude").join("settings.json"))
}

/// Single-quote for `sh` (and `'` survives `cmd /C` as a literal argument we pass through).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn sh_unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace(r"'\''", "'"))
}

/// The status line command that calls the bridge, wrapping `original` if there is one.
pub fn wrap_command(exe: &Path, original: Option<&str>) -> String {
    let exe = format!("\"{}\"", exe.display());
    match original {
        Some(o) if !o.trim().is_empty() => format!("{exe} {FLAG} -- {}", sh_quote(o)),
        _ => format!("{exe} {FLAG}"),
    }
}

/// The original command inside a wrapped one (None = there was none).
pub fn unwrap_command(cmd: &str) -> Option<String> {
    let rest = cmd.split_once(&format!("{FLAG} -- "))?.1;
    sh_unquote(rest.trim())
}

pub fn is_connected(settings: &Value) -> bool {
    settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(FLAG))
}

/// Point `statusLine` at the bridge. Keeps every other setting and statusLine field.
pub fn connect(settings: &mut Value, exe: &Path) {
    if is_connected(settings) {
        return;
    }
    let original = settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let cmd = wrap_command(exe, original.as_deref());
    if !settings.is_object() {
        *settings = Value::Object(Default::default());
    }
    let obj = settings.as_object_mut().expect("object");
    let line = obj
        .entry("statusLine")
        .or_insert_with(|| Value::Object(Default::default()));
    if !line.is_object() {
        *line = Value::Object(Default::default());
    }
    line["type"] = Value::from("command");
    line["command"] = Value::from(cmd);
}

/// Restore the original status line (or remove ours if there was none).
pub fn disconnect(settings: &mut Value) {
    let Some(cmd) = settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
    else {
        return;
    };
    if !cmd.contains(FLAG) {
        return;
    }
    match unwrap_command(cmd) {
        Some(original) => settings["statusLine"]["command"] = Value::from(original),
        None => {
            if let Some(o) = settings.as_object_mut() {
                o.remove("statusLine");
            }
        }
    }
}

/// Read-modify-write `~/.claude/settings.json` with a timestamped backup first.
pub fn edit_settings(f: impl FnOnce(&mut Value)) -> anyhow::Result<()> {
    let path = settings_path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let mut v: Value = match std::fs::read_to_string(&path) {
        Ok(t) => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            std::fs::write(
                path.with_extension(format!("json.notch-utils-backup-{ts}")),
                &t,
            )?;
            // Refuse to touch a file we can't parse instead of overwriting it.
            serde_json::from_str(&t)
                .map_err(|e| anyhow::anyhow!("{} is not valid JSON: {e}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(e) => return Err(e.into()),
    };
    f(&mut v);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v)? + "\n")?;
    std::fs::rename(tmp, &path)?;
    Ok(())
}

pub fn read_settings() -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(settings_path()?).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_documented_shape() {
        let input = r#"{"model":{"display_name":"Opus"},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},"seven_day":{"used_percentage":41.2,"resets_at":1738857600}}}"#;
        let u = parse_statusline(input).unwrap();
        assert_eq!(
            u.five_hour,
            Some(Window {
                used_percentage: 23.5,
                resets_at: 1738425600
            })
        );
        assert_eq!(u.seven_day.unwrap().used_percentage, 41.2);
        assert_eq!(
            parse_statusline(r#"{"model":{}}"#),
            None,
            "API-key users have no rate_limits"
        );
        assert_eq!(parse_statusline("not json"), None);
        let partial = parse_statusline(
            r#"{"rate_limits":{"seven_day":{"used_percentage":5,"resets_at":1}}}"#,
        )
        .unwrap();
        assert!(partial.five_hour.is_none() && partial.seven_day.is_some());
    }

    #[test]
    fn wrap_unwrap_round_trip_with_quotes() {
        let exe = Path::new("/Applications/notch utils/notch-utils");
        let orig = r#"jq -r '"\(.model.display_name)"' | tr a-z A-Z"#;
        let w = wrap_command(exe, Some(orig));
        assert!(
            w.starts_with("\"/Applications/notch utils/notch-utils\" --claude-statusline -- '")
        );
        assert_eq!(unwrap_command(&w).as_deref(), Some(orig));
        assert_eq!(unwrap_command(&wrap_command(exe, None)), None);
    }

    #[test]
    fn connect_disconnect_preserves_other_settings() {
        let exe = Path::new("/x/notch-utils");
        let mut s = json!({"model": "opus", "statusLine": {"type": "command", "command": "bash ~/sl.sh", "padding": 1}});
        connect(&mut s, exe);
        assert!(is_connected(&s));
        assert_eq!(s["statusLine"]["padding"], 1);
        assert_eq!(s["model"], "opus");
        connect(&mut s, exe); // idempotent: never wraps twice
        assert_eq!(
            unwrap_command(s["statusLine"]["command"].as_str().unwrap()).as_deref(),
            Some("bash ~/sl.sh")
        );
        disconnect(&mut s);
        assert_eq!(
            s,
            json!({"model": "opus", "statusLine": {"type": "command", "command": "bash ~/sl.sh", "padding": 1}})
        );

        let mut empty = json!({"model": "opus"});
        connect(&mut empty, exe);
        assert!(is_connected(&empty));
        disconnect(&mut empty);
        assert_eq!(empty, json!({"model": "opus"}));
    }
}

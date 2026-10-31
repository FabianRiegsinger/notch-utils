//! Single instance + remote commands. A second launch forwards its command to the
//! running instance over a local socket (named pipe on Windows) and exits.

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};
use std::io::{self, BufRead, BufReader, Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Settings,
    /// Plain launch while already running: nothing to do.
    Ping,
}

impl Command {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "toggle" | "--toggle" => Some(Self::Toggle),
            "settings" | "--settings" => Some(Self::Settings),
            "ping" => Some(Self::Ping),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Settings => "settings",
            Self::Ping => "ping",
        }
    }
}

fn name() -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        "notch-utils.sock".to_ns_name::<GenericNamespaced>()
    } else {
        // macOS has no abstract namespace: use a per-user file socket.
        let dir = directories::ProjectDirs::from("", "", "notch-utils")
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir)?;
        dir.join("notch-utils.sock")
            .to_fs_name::<GenericFilePath>()
            .map(|n| n.into_owned())
    }
}

/// Ok(true) if a running instance received the command.
pub fn send(cmd: Command) -> io::Result<bool> {
    match Stream::connect(name()?) {
        Ok(mut s) => {
            s.write_all(format!("{}\n", cmd.as_str()).as_bytes())?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// Listen on a background thread; `on_cmd` runs on that thread.
pub fn serve(on_cmd: impl Fn(Command) + Send + 'static) -> io::Result<()> {
    // `send` already failed, so a leftover socket file is a stale one from a crash.
    let listener = ListenerOptions::new()
        .name(name()?)
        .try_overwrite(true)
        .create_sync()?;
    std::thread::Builder::new()
        .name("ipc".into())
        .spawn(move || {
            for conn in listener.incoming().filter_map(Result::ok) {
                let mut line = String::new();
                // Commands are tiny; cap the read so a bad client can't make us buffer forever.
                if BufReader::new(conn.take(64)).read_line(&mut line).is_ok() {
                    match Command::parse(&line) {
                        Some(cmd) => on_cmd(cmd),
                        None => log::warn!("ipc: unknown command {line:?}"),
                    }
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_commands() {
        assert_eq!(Command::parse("toggle\n"), Some(Command::Toggle));
        assert_eq!(Command::parse("--settings"), Some(Command::Settings));
        assert_eq!(Command::parse("rm -rf"), None);
        for c in [Command::Toggle, Command::Settings, Command::Ping] {
            assert_eq!(Command::parse(c.as_str()), Some(c));
        }
    }
}

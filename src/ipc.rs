//! Single instance + remote commands. A second launch forwards its commands to the
//! running instance over a local socket (named pipe on Windows) and exits.

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;

/// Cap what one client may send so a bad client can't make us buffer forever.
const MAX_REQUEST: u64 = 64 * 1024;
const MAX_LINES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Settings,
    /// Put a file on the shelf (absolute path).
    Shelf(PathBuf),
    /// Plain launch while already running: nothing to do.
    Ping,
}

impl Command {
    /// One line of the wire protocol: "toggle", "settings", "ping", "shelf <abs path>".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim_end_matches(['\r', '\n']);
        match s {
            "toggle" => Some(Self::Toggle),
            "settings" => Some(Self::Settings),
            "ping" => Some(Self::Ping),
            _ => s
                .strip_prefix("shelf ")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(Self::Shelf),
        }
    }

    fn line(&self) -> String {
        match self {
            Self::Toggle => "toggle".into(),
            Self::Settings => "settings".into(),
            Self::Ping => "ping".into(),
            Self::Shelf(p) => format!("shelf {}", p.display()),
        }
    }
}

/// Command line → commands. `--shelf` takes the remaining arguments as files.
pub fn from_args(args: &[String]) -> Result<Vec<Command>, String> {
    match args.first().map(String::as_str) {
        None => Ok(vec![Command::Ping]),
        Some("--toggle") if args.len() == 1 => Ok(vec![Command::Toggle]),
        Some("--settings") if args.len() == 1 => Ok(vec![Command::Settings]),
        Some("--shelf") if args.len() > 1 => args[1..]
            .iter()
            .map(|a| {
                std::path::absolute(a)
                    .map(Command::Shelf)
                    .map_err(|e| format!("{a}: {e}"))
            })
            .collect(),
        Some(a) => Err(format!("unknown argument {a:?}")),
    }
}

fn name() -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        // Abstract sockets (Linux) and named pipes (Windows) are machine-wide: one per user.
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        format!("notch-utils-{user}.sock").to_ns_name::<GenericNamespaced>()
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

/// Ok(true) if a running instance received the commands.
pub fn send(cmds: &[Command]) -> io::Result<bool> {
    match Stream::connect(name()?) {
        Ok(mut s) => {
            for c in cmds {
                s.write_all(format!("{}\n", c.line()).as_bytes())?;
            }
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
                let lines = BufReader::new(conn.take(MAX_REQUEST)).lines();
                for line in lines.map_while(Result::ok).take(MAX_LINES) {
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
        assert_eq!(Command::parse("rm -rf"), None);
        assert_eq!(
            Command::parse("shelf relative/path"),
            None,
            "only absolute paths"
        );
        let c = Command::Shelf(std::path::absolute("x y.txt").unwrap());
        assert_eq!(Command::parse(&c.line()), Some(c));
        for c in [Command::Toggle, Command::Settings, Command::Ping] {
            assert_eq!(Command::parse(&c.line()), Some(c));
        }
    }

    #[test]
    fn args() {
        assert_eq!(from_args(&[]), Ok(vec![Command::Ping]));
        assert_eq!(from_args(&["--toggle".into()]), Ok(vec![Command::Toggle]));
        assert!(from_args(&["--shelf".into()]).is_err());
        assert!(from_args(&["--bogus".into()]).is_err());
        let v = from_args(&["--shelf".into(), "a".into(), "/b".into()]).unwrap();
        assert_eq!(v.len(), 2);
        assert!(matches!(&v[1], Command::Shelf(p) if p == std::path::Path::new("/b")));
    }
}

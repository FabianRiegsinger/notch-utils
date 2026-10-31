//! Live reload: watch the config *directory* (saves replace the file via rename, which
//! would orphan a watch on the file itself).

use notify::{RecursiveMode, Watcher};
use std::path::Path;

/// Keep the returned watcher alive. `on_change` runs on notify's thread.
pub fn watch(
    path: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<notify::RecommendedWatcher> {
    let file = path.file_name().map(|f| f.to_owned());
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && ev.paths.iter().any(|p| p.file_name() == file.as_deref())
        {
            on_change()
        }
    })?;
    if let Some(dir) = path.parent() {
        w.watch(dir, RecursiveMode::NonRecursive)?;
    }
    Ok(w)
}

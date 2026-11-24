//! Linux specifics of the updater.

use std::path::{Path, PathBuf};
use std::process::Command;
use uo_common::OVERLAY_EXE;

pub fn no_window(_c: &mut Command) {}

/// Our own folder if it holds the program, else the default installation
/// (~/.local/share/universal-overlay).
pub fn install_dir() -> PathBuf {
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    if own.join(OVERLAY_EXE).is_file() {
        return own;
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("universal-overlay")
}

pub fn version_file(dir: &Path) -> Option<PathBuf> {
    Some(dir.join("VERSION"))
}

/// Linux can replace a running binary: no staging needed.
pub fn restart_staged(
    _work: &Path,
    _repo: &str,
    _install: &Path,
) -> Option<Result<String, String>> {
    None
}

pub fn run_installer(pkg: &Path, install: &Path) -> Result<(), String> {
    let st = Command::new("/bin/sh")
        .arg(pkg.join("install.sh"))
        .args(["--update", "--install-dir"])
        .arg(install)
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("установщик завершился с кодом {st}"))
    }
}

pub fn pause_if_console() {}

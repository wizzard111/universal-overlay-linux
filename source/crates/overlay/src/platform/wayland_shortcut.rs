//! Wayland gives no global hotkeys to apps, so we register the overlay's
//! hotkey with the desktop itself: it runs `universal-overlay --toggle`.
//!   * GNOME: a custom keybinding in gsettings (applies immediately).
//!   * KDE Plasma: a command shortcut in kglobalshortcutsrc + .desktop file
//!     (applies after re-login on Plasma 6).

use std::path::PathBuf;
use std::process::Command;
use uo_common::hotkey::Hotkey;

const GNOME_KEY: &str = "org.gnome.settings-daemon.plugins.media-keys";
const GNOME_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/universal-overlay/";
const KDE_DESKTOP: &str = "universal-overlay-toggle.desktop";

pub fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
        || (std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none())
}

fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
}

/// GTK accelerator, e.g. "<Control><Alt>o".
fn gtk_accel(hk: &Hotkey) -> Option<String> {
    let k = hk.key.as_str();
    let key = match k {
        _ if k.len() == 1 => k.to_lowercase(),
        _ if k.starts_with('F') && k[1..].parse::<u32>().is_ok() => k.to_string(),
        _ if k.starts_with("Numpad") && k.len() == 7 => format!("KP_{}", &k[6..]),
        "Space" => "space".into(),
        "Enter" => "Return".into(),
        "Backspace" => "BackSpace".into(),
        "PageUp" => "Page_Up".into(),
        "PageDown" => "Page_Down".into(),
        "ScrollLock" => "Scroll_Lock".into(),
        "PrintScreen" => "Print".into(),
        "NumpadMultiply" => "KP_Multiply".into(),
        "NumpadAdd" => "KP_Add".into(),
        "NumpadSubtract" => "KP_Subtract".into(),
        "NumpadDecimal" => "KP_Decimal".into(),
        "NumpadDivide" => "KP_Divide".into(),
        "Minus" => "minus".into(),
        "Equals" => "equal".into(),
        "Backtick" => "grave".into(),
        "OpenBracket" => "bracketleft".into(),
        "CloseBracket" => "bracketright".into(),
        "Backslash" => "backslash".into(),
        "Semicolon" => "semicolon".into(),
        "Quote" => "apostrophe".into(),
        "Comma" => "comma".into(),
        "Period" => "period".into(),
        "Slash" => "slash".into(),
        "Tab" | "Escape" | "Insert" | "Delete" | "Home" | "End" | "Up" | "Down" | "Left"
        | "Right" | "Pause" => k.into(),
        _ => return None,
    };
    let mut s = String::new();
    if hk.ctrl {
        s += "<Control>";
    }
    if hk.alt {
        s += "<Alt>";
    }
    if hk.shift {
        s += "<Shift>";
    }
    if hk.win {
        s += "<Super>";
    }
    Some(s + &key)
}

fn gsettings(args: &[&str]) -> Option<String> {
    let out = Command::new("gsettings").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Parse a gsettings string array like "['/a/', '/b/']" or "@as []".
fn parse_list(s: &str) -> Vec<String> {
    s.trim_start_matches("@as")
        .trim()
        .trim_matches(['[', ']'])
        .split(',')
        .map(|x| x.trim().trim_matches('\'').to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn format_list(v: &[String]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|x| format!("'{x}'"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn gnome_set(hk: &Hotkey, exe: &str) -> Result<(), String> {
    let accel = gtk_accel(hk).ok_or("GNOME does not support this key")?;
    let cur =
        gsettings(&["get", GNOME_KEY, "custom-keybindings"]).ok_or("gsettings is not available")?;
    let mut list = parse_list(&cur);
    if !list.iter().any(|p| p == GNOME_PATH) {
        list.push(GNOME_PATH.into());
        gsettings(&["set", GNOME_KEY, "custom-keybindings", &format_list(&list)])
            .ok_or("could not write the list")?;
    }
    let schema = format!("{GNOME_KEY}.custom-keybinding:{GNOME_PATH}");
    gsettings(&["set", &schema, "name", "Universal Overlay"]);
    gsettings(&["set", &schema, "command", &format!("'{exe}' --toggle")]);
    gsettings(&["set", &schema, "binding", &accel]).ok_or("could not assign the hotkey")?;
    Ok(())
}

fn gnome_remove() {
    if let Some(cur) = gsettings(&["get", GNOME_KEY, "custom-keybindings"]) {
        let list: Vec<String> = parse_list(&cur)
            .into_iter()
            .filter(|p| p != GNOME_PATH)
            .collect();
        gsettings(&["set", GNOME_KEY, "custom-keybindings", &format_list(&list)]);
    }
    let schema = format!("{GNOME_KEY}.custom-keybinding:{GNOME_PATH}");
    for k in ["name", "command", "binding"] {
        gsettings(&["reset", &schema, k]);
    }
}

fn apps_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("applications")
}

fn kwriteconfig() -> Option<&'static str> {
    ["kwriteconfig6", "kwriteconfig5"]
        .into_iter()
        .find(|c| Command::new(c).arg("--help").output().is_ok())
}

fn kde_set(hk: &Hotkey, exe: &str) -> Result<(), String> {
    let dir = apps_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join(KDE_DESKTOP),
        format!("[Desktop Entry]\nType=Application\nName=Universal Overlay: show/hide\nExec={exe} --toggle\nNoDisplay=true\n"),
    )
    .map_err(|e| e.to_string())?;
    let tool = kwriteconfig().ok_or("kwriteconfig not found")?;
    let st = Command::new(tool)
        .args([
            "--file",
            "kglobalshortcutsrc",
            "--group",
            "services",
            "--group",
            KDE_DESKTOP,
            "--key",
            "_launch",
        ])
        .arg(hk.to_string().replace("Win+", "Meta+"))
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        Ok(())
    } else {
        Err("kwriteconfig failed".into())
    }
}

fn kde_remove() {
    let _ = std::fs::remove_file(apps_dir().join(KDE_DESKTOP));
    if let Some(tool) = kwriteconfig() {
        let _ = Command::new(tool)
            .args([
                "--file",
                "kglobalshortcutsrc",
                "--group",
                "services",
                "--group",
                KDE_DESKTOP,
                "--key",
                "_launch",
                "--delete",
            ])
            .status();
    }
}

/// Register `hotkey` with the current desktop. Returns a human message.
pub fn sync(hotkey: &str) -> Result<String, String> {
    let hk = Hotkey::parse(hotkey).ok_or("hotkey not recognized")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();
    let d = desktop();
    if d.contains("gnome") || d.contains("unity") || d.contains("budgie") || d.contains("cinnamon")
    {
        gnome_set(&hk, &exe).map(|_| format!("GNOME: {hotkey} → show/hide"))
    } else if d.contains("kde") || d.contains("plasma") {
        kde_set(&hk, &exe).map(|_| format!("KDE: {hotkey} → show/hide (after signing in again)"))
    } else {
        Err(format!(
            "desktop \"{d}\": bind `{exe} --toggle` to a hotkey manually"
        ))
    }
}

pub fn remove() {
    gnome_remove();
    kde_remove();
}

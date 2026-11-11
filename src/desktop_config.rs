//! The app's own settings: where the engine lives, and whether to start at logon. Stored per user
//! under `%APPDATA%\activity-journal-desktop\desktop.toml`, never next to the binary.

use std::path::PathBuf;

use toml_edit::DocumentMut;

const ENGINE_ENV: &str = "ACTIVITY_JOURNAL_ROOT";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ActivityJournalDesktop";

fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("activity-journal-desktop").join("desktop.toml"))
}

/// The engine folder: `--engine <path>`, then `ACTIVITY_JOURNAL_ROOT`, then the saved value.
pub fn engine_root(args: &[String]) -> Option<PathBuf> {
    if let Some(index) = args.iter().position(|a| a == "--engine")
        && let Some(path) = args.get(index + 1)
    {
        return Some(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os(ENGINE_ENV) {
        return Some(PathBuf::from(path));
    }
    let text = std::fs::read_to_string(config_path()?).ok()?;
    let document: DocumentMut = text.parse().ok()?;
    document.get("engine_root").and_then(|i| i.as_str()).map(PathBuf::from)
}

/// Whether `path` looks like an Activity Journal checkout.
pub fn is_engine_root(path: &std::path::Path) -> bool {
    path.join("pyproject.toml").is_file() && path.join("src").join("activity_journal").is_dir()
}

pub fn save_engine_root(path: &std::path::Path) -> Result<(), String> {
    let file = config_path().ok_or("APPDATA is not set")?;
    let mut document: DocumentMut = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    document["engine_root"] = toml_edit::value(path.display().to_string());
    std::fs::create_dir_all(file.parent().expect("the config file has a folder")).map_err(|e| e.to_string())?;
    std::fs::write(&file, document.to_string()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Whether the app is registered to start at logon (per user, no scheduled task).
pub fn autostart_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", RUN_VALUE]).is_ok()
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let command = format!("\"{}\" --hidden", exe.display());
        reg(&["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &command, "/f"])
    } else {
        reg(&["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])
    }
}

fn reg(args: &[&str]) -> Result<(), String> {
    let mut command = std::process::Command::new("reg");
    command
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg {} failed", args[0]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_wins() {
        let args = ["app".to_owned(), "--engine".to_owned(), "D:/x".to_owned()];
        assert_eq!(engine_root(&args), Some(PathBuf::from("D:/x")));
    }
}

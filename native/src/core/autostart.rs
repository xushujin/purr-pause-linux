//! 开机自启：写入 ~/.config/autostart/purr-pause-linux.desktop。

use std::fs;
use std::path::PathBuf;

fn autostart_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("autostart")
}

fn autostart_file() -> PathBuf {
    autostart_dir().join("purr-pause-linux.desktop")
}

pub fn is_enabled() -> bool {
    let path = autostart_file();
    if !path.exists() {
        return false;
    }
    fs::read_to_string(path).map(|c| !c.contains("Hidden=true")).unwrap_or(false)
}

fn desktop_entry() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "purr-pause-linux".to_string());
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=胖猫暂停一下（PurrPause Linux）\n\
         Comment=胖猫暂停一下 - 屏幕休息提醒\n\
         Exec=\"{exe}\"\n\
         Icon=purr-pause-linux\n\
         Terminal=false\n\
         Hidden=false\n\
         X-GNOME-Autostart-enabled=true\n\
         Categories=Utility;\n"
    )
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    let path = autostart_file();
    if on {
        fs::create_dir_all(autostart_dir()).map_err(|e| e.to_string())?;
        fs::write(&path, desktop_entry()).map_err(|e| e.to_string())?;
        set_hidden(&path, false)?;
    } else if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// GNOME 会根据 Hidden 字段控制显隐。
fn set_hidden(path: &PathBuf, hidden: bool) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let updated = if hidden {
        content.replace("Hidden=false", "Hidden=true")
    } else {
        content.replace("Hidden=true", "Hidden=false")
    };
    fs::write(path, updated).map_err(|e| e.to_string())
}
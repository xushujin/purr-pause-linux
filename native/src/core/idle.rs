//! 系统空闲检测与锁屏/休眠感知。
//!
//! 直接走 gdbus / XScreenSaver，无需探测外部命令是否存在。
//!
//! ⚠️ 单位约定：**Mutter 的 `GetIdletime` 返回毫秒**（不是微秒）。
//!    早期版本误按微秒处理而除以 1000，导致 idle 恒小于阈值，
//!    `active_seconds` 无限累加且从不清零，计时功能完全失效。

use std::process::{Command, Stdio};

/// Mutter IdleMonitor 在 session bus 上的对象路径。
const IDLE_MONITOR_PATH: &str = "/org/gnome/Mutter/IdleMonitor/Core";

/// 获取系统空闲毫秒数。失败返回 None（调用方应视为无法检测）。
pub fn idle_ms() -> Option<u64> {
    // 1) Mutter（GNOME Wayland / X11 通用）
    if let Some(v) = idle_via_mutter() {
        return Some(v);
    }
    // 2) XScreenSaver（X11 兜底，非 GNOME 环境）
    if let Some(v) = idle_via_xprintidle() {
        return Some(v);
    }
    None
}

/// Mutter `org.gnome.Mutter.IdleMonitor.GetIdletime`。
///
/// 历史上该接口存在两个 bus name 归属：
///   - `org.gnome.Shell`（GNOME Shell 导出的标准路径）
///   - `org.gnome.Mutter.IdleMonitor`（部分版本自行 own 的名字）
/// 两者都试，任一可用即可。
///
/// 返回值单位为**毫秒**。
fn idle_via_mutter() -> Option<u64> {
    for dest in ["org.gnome.Shell", "org.gnome.Mutter.IdleMonitor"] {
        if let Some(v) = query_mutter_idle(dest) {
            return Some(v);
        }
    }
    None
}

fn query_mutter_idle(dest: &str) -> Option<u64> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            dest,
            "--object-path",
            IDLE_MONITOR_PATH,
            "--method",
            "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_uint64_field(&String::from_utf8_lossy(&out.stdout))
}

fn idle_via_xprintidle() -> Option<u64> {
    let out = Command::new("xprintidle")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok()
}

/// 从 gdbus 输出中提取 uint64 数值。
///
/// gdbus 的实际输出形如 `(uint64 12345,)` —— 注意类型前缀 `uint64` 里
/// **本身就含数字 64**。若简单地「找第一个数字」，会永远匹配到 64，
/// 导致 idle 恒为 64ms：既永远小于活跃阈值（计时无限累加），
/// 也永远达不到归零条件，计时功能彻底失效。
///
/// 正确做法是先剥掉 `(`，再**跳过整个类型名**（直到第一个空格），
/// 只在类型名之后读数字。
fn parse_uint64_field(s: &str) -> Option<u64> {
    let body = s.trim().trim_start_matches('(');
    //跳过类型名（uint64 / uint32 / int64 …），只在空格之后读数字
    let after_type = body.split_once(' ')?.1;
    let digits: String = after_type
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// 锁屏状态：true 表示已锁屏。
///
/// GNOME 下通过 org.gnome.ScreenSaver 的 Active 属性判断；
/// 其他桌面环境（KDE/Sway 等）返回 None，调用方按「未知」处理。
pub fn screen_locked() -> Option<bool> {
    for dest in ["org.gnome.ScreenSaver", "org.freedesktop.ScreenSaver"] {
        let out = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                dest,
                "--object-path",
                "/org/gnome/ScreenSaver",
                "--method",
                "org.freedesktop.DBus.Properties.Get",
                "org.gnome.ScreenSaver",
                "Active",
            ])
            .stderr(Stdio::null())
            .output()
            .ok();
        let Some(out) = out else { continue };
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        // 输出形如 `(true,)` / `(false,)`
        if s.contains("true") {
            return Some(true);
        }
        if s.contains("false") {
            return Some(false);
        }
    }
    None
}

/// 检测方式名称，用于设置页展示与日志。
pub fn detection_method() -> &'static str {
    if idle_via_mutter().is_some() {
        "gdbus (Mutter)"
    } else if idle_via_xprintidle().is_some() {
        "xprintidle (X11)"
    } else {
        "不可用"
    }
}
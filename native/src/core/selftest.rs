//! 环境自检：`purr-pause-linux --selftest` 一条命令看清「为什么不能用」。
//!
//! 设计目的：这个应用是 GUI 守护进程，默认零输出，出问题时用户手上
//! 什么线索都没有（历史上多次出现「点了没反应」却查不到原因）。
//! 自检把每一个可能失败的环节单独探测一遍并打印结果，用户复制输出即可定位。
//!
//! 只做**只读探测**，唯一的写操作是在数据目录里写一个临时文件验证可写性。

use std::io::Write;
use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::core::{config, idle};
use crate::license::{self, LicenseStatus};
use crate::video;

fn kv(key: &str, value: &str) {
    println!("  {:<14}{}", format!("{key}:"), value);
}

fn ok(key: &str, value: &str) {
    println!("  ✅ {:<12}{}", format!("{key}:"), value);
}

fn bad(key: &str, value: &str) {
    println!("  ❌ {:<12}{}", format!("{key}:"), value);
}

fn read_os_release() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "未知".to_string())
}

fn env_or_dash(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| "（未设置）".to_string())
}

pub fn run() -> i32 {
    println!("胖猫暂停一下（PurrPause Linux） 环境自检");
    println!("==================================================");

    println!("\n[1/8] 程序");
    kv("版本", env!("CARGO_PKG_VERSION"));
    kv("可执行文件", &std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default());
    kv("数据目录", &config::data_dir().display().to_string());
    kv("用户配置", &config::user_config_path().display().to_string());

    println!("\n[2/8] 系统与会话");
    kv("发行版", &read_os_release());
    kv("内核", &uname());
    kv("会话类型", &env_or_dash("XDG_SESSION_TYPE"));
    kv("桌面", &env_or_dash("XDG_CURRENT_DESKTOP"));
    kv("DISPLAY", &env_or_dash("DISPLAY"));
    kv("WAYLAND", &env_or_dash("WAYLAND_DISPLAY"));

    println!("\n[3/8] GTK / 显示");
    let gtk_ver = format!(
        "{}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    kv("GTK 运行时", &gtk_ver);
    match gtk::init() {
        Ok(()) => {
            ok("gtk_init", "成功");
            match gtk::gdk::Display::default() {
                Some(display) => {
                    // gdk::Display::name() 在 gtk4-rs 0.11 里直接返回 GString
                    ok("显示器", &display.name().to_string());
                    let model = display.monitors();
                    let n = gio::prelude::ListModelExt::n_items(&model);
                    kv("屏幕数量", &n.to_string());
                    for i in 0..n {
                        if let Some(obj) = gio::prelude::ListModelExt::item(&model, i) {
                            if let Ok(m) = obj.downcast::<gtk::gdk::Monitor>() {
                                let g = m.geometry();
                                let scale = m.scale_factor();
                                // gdk_monitor_get_refresh_rate 的单位是 **毫赫兹**
                                let hz = m.refresh_rate() as f64 / 1000.0;
                                kv(
                                    &format!("  屏幕 {i}"),
                                    &format!(
                                        "{}×{} @{}x  {:.1} Hz",
                                        g.width(),
                                        g.height(),
                                        scale,
                                        hz
                                    ),
                                );
                            }
                        }
                    }
                }
                None => bad("显示器", "拿不到 GdkDisplay —— 无法创建任何窗口"),
            }
        }
        Err(e) => {
            bad("gtk_init", &format!("失败：{e}"));
            println!("     → 没有可用的图形会话，屏保窗口不可能显示。");
        }
    }

    println!("\n[4/8] 视频渲染（libmpv）");
    match video::probe() {
        Ok(lib) => ok("libmpv", &lib),
        Err(e) => bad("libmpv", &e),
    }
    match video::probe_gl_libs() {
        Ok(libs) => ok("GL 库", &libs),
        Err(e) => bad("GL 库", &e),
    }

    println!("\n[5/8] 素材");
    let builtin = PathBuf::from("/usr/share/purr-pause-linux/webm");
    let cfg = config::load(&builtin.join("../config.json"));
    let (walk, rest) = config::video_paths(&cfg, &builtin);
    report_file("走路素材", &walk);
    report_file("休息素材", &rest);

    println!("\n[6/8] 配置读写");
    kv("阈值 / 休息", &format!("{} 分钟 / {} 分钟", cfg.threshold_minutes, cfg.break_minutes));
    kv("动画模式", &cfg.animation_mode);
    kv(
        "自定义素材目录",
        if cfg.custom_webm_dir.is_empty() {
            "（默认）"
        } else {
            cfg.custom_webm_dir.as_str()
        },
    );
    let dir = config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let probe_file = dir.join(".selftest-write-probe");
    match std::fs::write(&probe_file, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe_file);
            ok("数据目录可写", &dir.display().to_string());
        }
        Err(e) => bad("数据目录不可写", &format!("{}：{e}", dir.display())),
    }

    println!("\n[7/8] 空闲检测");
    match idle::idle_ms() {
        Some(ms) => ok("空闲时间", &format!("{} 秒（{}）", ms / 1000, idle::detection_method())),
        None => bad("空闲检测", "不可用 —— 自动触发休息提醒会失效（只能手动测试）"),
    }
    match idle::screen_locked() {
        Some(v) => kv("锁屏状态", if v { "已锁屏" } else { "未锁屏" }),
        None => kv("锁屏状态", "不可检测（非 GNOME 桌面，属正常）"),
    }

    println!("\n[8/8] 托盘与授权");
    match tray_watcher_available() {
        Some(true) => ok("托盘宿主", "org.kde.StatusNotifierWatcher 在线"),
        Some(false) => bad(
            "托盘宿主",
            "无 StatusNotifierWatcher —— 不会出现托盘图标（GNOME 需装 gnome-shell-extension-appindicator）",
        ),
        None => bad("托盘宿主", "连不上 session D-Bus"),
    }
    let st = license::check_status();
    let label = match st.status {
        LicenseStatus::Active if st.is_permanent => "已激活（永久）".to_string(),
        LicenseStatus::Active => format!("已激活（剩余 {} 天）", st.days_left),
        LicenseStatus::Trial => format!("试用中（剩余 {} 天）", st.days_left),
        LicenseStatus::Expired => "未激活".to_string(),
    };
    kv("授权", &label);

    println!("\n==================================================");
    println!("自检结束。把以上输出整段复制给开发者即可定位问题。");
    let _ = std::io::stdout().flush();
    0
}

fn report_file(label: &str, path: &std::path::Path) {
    if path.exists() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        ok(label, &format!("{}（{} KB）", path.display(), size / 1024));
    } else {
        bad(label, &format!("不存在：{}", path.display()));
    }
}

fn uname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "未知".to_string())
}

fn tray_watcher_available() -> Option<bool> {
    let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let reply = conn
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&glib::Variant::tuple_from_iter([
                "org.kde.StatusNotifierWatcher".to_variant(),
            ])),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .ok()?;
    reply.child_value(0).get::<bool>()
}

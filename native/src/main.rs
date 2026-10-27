//! 胖猫暂停一下（PurrPause）— Linux 原生版主程序。
//!
//! 主要功能：
//!   - 空闲计时，达到阈值后弹出猫咪覆盖层
//!   - 多显示器：每屏一个覆盖层窗口
//!   - 休息倒计时 + 延后（最多 2 次）
//!   - 托盘菜单：剩余时间、待办、暂停、设置、日志、测试、退出
//!   - 消息提醒：轮询第三方待办接口，列表窗口，角标
//!   - 安装即全功能（无授权/试用期）
//!
//! 技术要点：
//!   - 空闲检测直接走 gdbus，无需探测外部命令是否存在
//!   - 动画经 libmpv render API 渲染到 GtkGLArea（OpenGL，保留 alpha）；
//!     libmpv 在运行时 dlopen，兼容 libmpv.so.2 / libmpv.so.1
//!   - VP9+Alpha 素材必须软解（hwdec=no），硬解会丢 alpha 导致绿底

mod core;
mod ui;
mod video;

// glib 是 gtk4 的依赖而非本 crate 的直接依赖，
// Rust 2018 起不能用裸 crate 名，必须经 gtk 路径引入。
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use core::config::{self, Config};
use core::idle;
use core::logger::Logger;
use std::sync::atomic::{AtomicBool, Ordering};
use ui::overlay::{CatOverlay, OverlayEvent};
use ui::tray::{Tray, TrayItem};

/// `--test` 模式：覆盖层关掉后直接退出进程。
///
/// 否则用户在终端里敲 `purr-pause-linux --test`，命令永远不返回 ——
/// 一个「测试」命令变成常驻守护进程，很反直觉。
static TEST_MODE: AtomicBool = AtomicBool::new(false);

const APP_ID: &str = "com.purr-pause-linux.app";
const MONITOR_INTERVAL_MS: u64 = 10_000;
/// idle 超过该值视为离开（10 分钟）。
const IDLE_RESET_MS: u64 = 10 * 60 * 1000;
/// idle 小于该值视为活跃（3 分钟）。
const IDLE_ACTIVE_MS: u64 = 180 * 1000;

/// 启动模式（由命令行参数决定）。
///
/// 这些入口很重要：托盘不可用的环境（GNOME 未装 AppIndicators 扩展、
/// 或覆盖层意外挡住了托盘）下，用户仍然需要能打开设置、能测试屏保。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    /// 正常守护进程
    Normal,
    /// `--test`：启动后立即弹一次猫咪覆盖层
    Test,
    /// `--settings`：启动后直接打开设置窗口
    Settings,
    /// `--selftest`：打印环境自检后退出
    SelfTest,
    /// `--quit`：让已在运行的实例退出（托盘菜单点不动时的备用出口）
    Quit,
    /// `--version` / `--help`
    Version,
    Help,
}

fn parse_mode(args: &[String]) -> RunMode {
    let mut mode = RunMode::Normal;
    for a in args {
        match a.as_str() {
            "--test" | "test" => mode = RunMode::Test,
            "--settings" | "settings" => mode = RunMode::Settings,
            "--selftest" | "selftest" | "--diagnose" => mode = RunMode::SelfTest,
            "--quit" | "-q" | "quit" => mode = RunMode::Quit,
            "--version" | "-V" => mode = RunMode::Version,
            "--help" | "-h" => mode = RunMode::Help,
            _ => {}
        }
    }
    mode
}

const HELP_TEXT: &str = "\
胖猫暂停一下（PurrPause Linux）

用法：
  purr-pause-linux              后台常驻（托盘菜单是主要入口）
  purr-pause-linux --test       启动后立刻弹出一次休息提醒（验证屏保）
  purr-pause-linux --settings   启动后直接打开设置窗口
  purr-pause-linux --selftest   打印环境自检（排查「点了没反应」）
  purr-pause-linux --quit       让已在运行的实例退出
  purr-pause-linux --version    显示版本
  purr-pause-linux --help       显示本帮助

提示：
  · 托盘不可用时（GNOME 需 gnome-shell-extension-appindicator），
    用 --settings 打开设置、用 --test 验证屏保、用 --quit 退出。
  · 覆盖层可以用 Esc / 双击画面 / 关闭按钮退出。
";

/// 应用共享状态。
pub struct AppState {
    pub config: Config,
    active_seconds: i64,
    snooze_count: i64,
    snooze_threshold: i64,
    is_paused: bool,
    /// 暂停时长（分钟）；0 = 直到手动恢复；未暂停时为 -1
    paused_minutes: i64,
    is_overlay_showing: bool,
    last_dismiss: Option<Instant>,
    overlays: Vec<CatOverlay>,
    pub log: Logger,
    /// 屏幕是否处于锁屏状态
    is_screen_locked: bool,
}

fn main() {
    // 排查「启动无反应/秒退」专用：PURR_DEBUG=1 时把关键里程碑打到 stderr。
    // GUI 应用默认零输出，一旦出问题（秒退/无窗口）毫无线索；
    // 有了这条轨迹 + 退出码，一次实测即可定位死在哪一步。
    dbg_trace(&format!(
        "启动 v{} (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = parse_mode(&args);
    if mode == RunMode::Test {
        TEST_MODE.store(true, Ordering::Relaxed);
    }

    // 自检 / 版本 / 帮助：不需要 GUI，直接打印后退出。
    // 放在创建 Application 之前 —— 自检要能在没有显示环境的终端里跑。
    match mode {
        RunMode::SelfTest => std::process::exit(core::selftest::run()),
        RunMode::Version => {
            println!("purr-pause-linux {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        RunMode::Help => {
            print!("{HELP_TEXT}");
            return;
        }
        _ => {}
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // 不设 NON_UNIQUE：GApplication 默认借助 D-Bus well-known name 保证单实例。
    // 若启动第二个进程，它会把 activate 信号转发给已有实例后立即退出。
        // （早期版本误设 NON_UNIQUE —— 该标志恰恰是「允许多实例」，
        //   与注释意图相反，会导致同时跑多个托盘与多套计时器。）
        .build();

    // 「退出」动作：托盘菜单里点不动、或者根本没托盘时的备用出口。
    // 注册到 Application 上之后，`--quit` 的第二个实例会被 GApplication
    // 通过 D-Bus 把动作转发给已在运行的主实例。
    let quit_action = gtk::gio::SimpleAction::new("quit", None);
    {
        let app_weak = app.downgrade();
        quit_action.connect_activate(move |_, _| {
            if let Some(a) = app_weak.upgrade() {
                a.quit();
            }
        });
    }
    app.add_action(&quit_action);

    if mode == RunMode::Quit {
        // 先注册：若已有主实例，本进程会变成 remote，动作随之转发过去。
        let _ = app.register(gtk::gio::Cancellable::NONE);
        app.activate_action("quit", None);
        dbg_trace("--quit：已向主实例发送退出请求");
        return;
    }

    // 注意：hold() 已移出本闭包（见下方 main 作用域），
    // 故此处参数不再使用，用 `_app` 避免 unused_variables 警告。
    // ⚠️ 初始化放在 startup（**只跑一次**），不要放回 connect_activate。
    //
    // GApplication 在**每次**被激活时都会发 activate —— 包括第二个实例通过
    // D-Bus 转发过来的激活。原先整段初始化写在 activate 里，导致用户再点一次
    // 图标就会把一切重来一遍：重复的计时器、重复的消息轮询、重复的托盘。
    app.connect_startup(move |app_ref| {
        dbg_trace("startup：开始初始化");
        // ⚠️ mpv 要求 LC_NUMERIC 必须是 "C"，而 gtk_init() 会把它设成用户 locale。
        //    GTK 初始化已在本回调之前完成，这里改回来 —— 否则 mpv_create 返回 NULL。
        video::ensure_c_numeric_locale();
        // 保存 Application 句柄：设置窗口要 set_application() 才能被应用管理
        // （未关联的裸窗口不进入应用的窗口列表，再次点图标也不会被前置）。
        APP.with(|a| *a.borrow_mut() = Some(app_ref.clone()));
        let data_dir = config::data_dir();
        let _ = std::fs::create_dir_all(&data_dir);
        let log_dir = data_dir.join("logs");

        // 加载配置：debug 用仓库内 data/config.json，发行版用 /usr/share/purr-pause-linux/config.json
        let builtin_cfg = if cfg!(debug_assertions) {
            PathBuf::from("data/config.json")
        } else {
            PathBuf::from("/usr/share/purr-pause-linux/config.json")
        };
        let mut cfg = config::load(&builtin_cfg);

        // debug 时素材目录指向 data/webm（发行版由安装路径 + 用户配置解析）
        if cfg!(debug_assertions) && cfg.custom_webm_dir.is_empty() {
            if let Ok(cwd) = std::env::current_dir() {
                let candidate = cwd.join("data/webm");
                if candidate.exists() {
                    cfg.custom_webm_dir = candidate.to_string_lossy().to_string();
                }
            }
        }

        let log = Logger::new(log_dir, cfg.log_enabled);

        // 首次运行写入素材说明
        write_asset_readme(&data_dir.join("webm"));

        let state = Rc::new(RefCell::new(AppState {
            config: cfg,
            active_seconds: 0,
            snooze_count: 0,
            snooze_threshold: 0,
            is_paused: false,
            paused_minutes: -1,
            is_overlay_showing: false,
            last_dismiss: None,
            overlays: Vec::new(),
            log,
            is_screen_locked: false,
        }));

        log_line(&state, &format!("应用启动, 版本 {}", env!("CARGO_PKG_VERSION")));
        log_line(&state, &format!("空闲检测方式: {}", idle::detection_method()));

        build_tray(&state);
        start_monitoring(&state);

        // 供 activate 回调取用（见下方 connect_activate）
        STATE.with(|s| *s.borrow_mut() = Some(state.clone()));

        // 命令行入口：--test / --settings 在初始化完成后立刻执行一次。
        // 延迟 600ms 是为了等窗口系统准备好（首次映射前 present 会被忽略）。
        if matches!(mode, RunMode::Test | RunMode::Settings) {
            let st = state.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                if mode == RunMode::Test {
                    dbg_trace("--test：立即触发覆盖层");
                    trigger_cat(&st, true);
                } else {
                    dbg_trace("--settings：打开设置窗口");
                    open_settings(&st);
                }
            });
        }

        // 应用常驻：GApplication 在 use_count 归零（无窗口、无 hold）时退出。
        // ⚠️ 两个要点：
        //   1. hold() 必须在注册之后调用 —— startup 信号正是在 register 之后、
        //      activate 之前触发的，位置正确；原先写在 main 里 run() 之前，
        //      属于「未注册就 hold」，行为不可靠。
        //   2. guard 有意 forget：守护进程存活期间永不释放（退出走
        //      设置窗口的「退出应用」按钮 / 未来的托盘菜单）。
        //      若正常绑定到变量，main 返回时 guard 析构 = hold 白做。
        std::mem::forget(app_ref.hold());

        let tray_state = if TRAY.with(|t| t.borrow().is_some()) {
            "托盘已创建"
        } else {
            "托盘不可用（已降级，无托盘图标）"
        };
        dbg_trace(&format!("startup：初始化完成（{tray_state}），已 hold 常驻"));
    });

    // activate 在**每次**被激活时触发：首次启动，以及用户再次点击启动器。
    //
    // ⚠️ 这里必须弹窗，不能照搬 electron 版的「静默进驻」：
    //    electron 启动后什么都不显示，是因为有托盘图标作为唯一入口；
    //    而 native 的托盘（GTK3 AppIndicator）在 GTK4 下必然失败，
    //    若这里也不弹窗，用户点开图标看到的就是「毫无反应、无任务栏图标」，
    //    应用等于不可用 —— 这正是 v1.0.0 用户实测反馈的现象。
    //    托盘（D-Bus StatusNotifierItem）落地后，可改成
    //    「托盘可用 → 静默进驻；不可用 → 弹窗」。
    app.connect_activate(move |_app| {
        dbg_trace("activate：收到激活请求（打开窗口）");
        // --test 模式下不弹设置窗口：否则全屏覆盖层会盖住它，
        // 用户测完还得再找一次设置窗口，体验很糟。
        if mode == RunMode::Test {
            dbg_trace("activate：--test 模式，跳过自动开窗");
            return;
        }
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            dbg_trace("activate：⚠️ STATE 未初始化（startup 未跑过？），忽略");
            return;
        };
        dbg_trace("activate：打开设置窗口");
        open_settings(&state);
    });

    let code = app.run();
    dbg_trace(&format!(
        "⚠️ 主循环已退出（exit={code:?}）——正常情况下守护进程不应走到这里"
    ));
}

/// 排查用轨迹：设 `PURR_DEBUG=1` 时把关键里程碑打到 stderr。
///
/// GUI 应用默认零输出，一旦「秒退/无窗口」毫无线索；
/// `PURR_DEBUG=1 purr-pause-linux; echo $?` 一次实测即可定位死在哪一步、
/// 以及退出码含义（0=正常退出 / 139=段错误 / 134=abort）。
fn dbg_trace(msg: &str) {
    if std::env::var_os("PURR_DEBUG").is_some() {
        eprintln!("[purr-pause-linux] {msg}");
    }
}

/// 写入日志。
fn log_line(state: &Rc<RefCell<AppState>>, msg: &str) {
    let s = state.borrow();
    s.log.info(msg);
}

fn write_asset_readme(dir: &std::path::Path) {
    if dir.exists() {
        let readme = dir.join("purr-pause-linux-素材说明.txt");
        if !readme.exists() {
            let _ = std::fs::write(readme, include_str!("../data/README-assets.txt"));
        }
    }
}

/// 启动 10 秒轮询的空闲监控。
fn start_monitoring(state: &Rc<RefCell<AppState>>) {
    let state = state.clone();
    glib::timeout_add_local(Duration::from_millis(MONITOR_INTERVAL_MS), move || {
        // 锁屏检测：锁屏时不累计活跃时长
        if let Some(locked) = idle::screen_locked() {
            let was = { state.borrow().is_screen_locked };
            if locked && !was {
                log_line(&state, "屏幕已锁定");
            } else if !locked && was {
                // 锁屏超过 10 分钟则计时归零
                state.borrow_mut().is_screen_locked = false;
                let mut s = state.borrow_mut();
                if s.active_seconds > 0 {
                    s.log.info(&format!(
                        "锁屏结束, 计时归零 (之前累计{}秒)",
                        s.active_seconds
                    ));
                    s.active_seconds = 0;
                    s.snooze_threshold = 0;
                }
            }
            state.borrow_mut().is_screen_locked = locked;
        }

        // 覆盖层状态自愈：窗口被窗口管理器或异常路径关掉后，
        // is_overlay_showing 会残留为 true，导致计时和「立即测试」永久失效。
        {
            let mut s = state.borrow_mut();
            if s.is_overlay_showing && s.overlays.is_empty() {
                s.is_overlay_showing = false;
                s.active_seconds = 0;
                s.snooze_threshold = 0;
                s.log.warn("覆盖层已消失，状态自动复位");
            }
        }

        let should_skip = {
            let s = state.borrow();
            s.is_overlay_showing || s.is_paused || s.is_screen_locked
        };
        if !should_skip {
            let idle_ms = idle::idle_ms().unwrap_or(0);
            let mut trigger = false;
            {
                let mut s = state.borrow_mut();
                if idle_ms > IDLE_RESET_MS {
                    if s.active_seconds > 0 {
                        s.log
                            .info(&format!("空闲超过10分钟, 计时归零 (累计{}秒)", s.active_seconds));
                    }
                    s.active_seconds = 0;
                    s.snooze_threshold = 0;
                } else if idle_ms < IDLE_ACTIVE_MS {
                    s.active_seconds += (MONITOR_INTERVAL_MS / 1000) as i64;
                }

                let threshold = if s.snooze_threshold > 0 {
                    s.snooze_threshold
                } else {
                    s.config.threshold_minutes * 60
                };
                if s.active_seconds >= threshold {
                    s.snooze_threshold = 0;
                    trigger = true;
                }
            }
            if trigger {
                trigger_cat(&state, false);
            }
            refresh_tray(&state);
        }
        glib::ControlFlow::Continue
    });
}

/// 弹出猫咪覆盖层（每屏一个）。
fn trigger_cat(state: &Rc<RefCell<AppState>>, manual: bool) {
    let (cfg, break_minutes, snooze_count) = {
        let s = state.borrow();
        if s.is_overlay_showing {
            return;
        }
        if !manual {
            if let Some(last) = s.last_dismiss {
                if last.elapsed() < Duration::from_secs(60) {
                    return; // 1 分钟内不重复弹出
                }
            }
        }
        (
            s.config.clone(),
            s.config.break_minutes,
            s.snooze_count,
        )
    };

    // 素材路径
    let builtin = builtin_webm_dir();
    let (walk, idle_v) = config::video_paths(&cfg, &builtin);

    {
        let mut s = state.borrow_mut();
        s.is_overlay_showing = true;
        for ov in s.overlays.drain(..) {
            ov.close();
        }
    }

    // GTK4 移除了 gtk::gdk::WindowPosition 与 Window::move_()，
    // 多屏全屏的正确做法是给每个显示器各建一个窗口并调`fullscreen_on_monitor`。
    // 这比手动算坐标更可靠：Wayland 下compositor 不允许客户端自行定位窗口。
    let monitors = list_monitors();
    if monitors.is_empty() {
        // 枚举不到显示器（部分 Wayland 合成器 / 远程会话）时不能什么都不做 ——
        // 那样用户点「立即测试」就是毫无反应。退化为单个全屏窗口。
        eprintln!("[purr-pause-linux] 未能枚举显示器，退化为单个全屏覆盖层");
        let st_ev = state.clone();
        let overlay = CatOverlay::new(
            cfg.animation_mode.clone(),
            break_minutes,
            snooze_count,
            walk.clone(),
            idle_v.clone(),
            move |ev| dispatch_overlay_event(&st_ev, ev),
        );
        overlay.window.fullscreen();
        overlay.window.present();
        state.borrow_mut().overlays.push(overlay);
    }
    for monitor in monitors {
        let st_ev = state.clone();
        let overlay = CatOverlay::new(
            cfg.animation_mode.clone(),
            break_minutes,
            snooze_count,
            walk.clone(),
            idle_v.clone(),
            move |ev| dispatch_overlay_event(&st_ev, ev),
        );
        let geo = monitor.geometry();
        overlay.window.set_default_size(geo.width(), geo.height());
        // 先 present 再指定显示器：对未映射的窗口调用 fullscreen_on_monitor
        // 在部分 GTK 版本上只是记下请求，映射后才生效，顺序反过来更稳妥。
        overlay.window.present();
        overlay.window.fullscreen_on_monitor(&monitor);
        state.borrow_mut().overlays.push(overlay);
    }

    log_line(state, "触发猫咪覆盖层");

    // ── 看门狗 ─────────────────────────────────────────────────────
    // 覆盖层是全屏窗口，一旦「显示了但没映射成功」或「建完就被销毁」，
    // 状态位 is_overlay_showing 会永久为 true，从此计时和「立即测试」全部失效。
    // 6 秒后检查：一个窗口都没映射上就自动收摊，保证应用不会卡死。
    {
        let st = state.clone();
        glib::timeout_add_local_once(Duration::from_secs(6), move || {
            let stuck = {
                let s = st.borrow();
                s.is_overlay_showing && !s.overlays.iter().any(|ov| ov.window.is_mapped())
            };
            if stuck {
                eprintln!(
                    "[purr-pause-linux] 覆盖层 6 秒内未能显示，已自动关闭（避免卡住桌面与托盘）"
                );
                dismiss_cat(&st);
            }
        });
    }

}

/// 覆盖层事件分发。
///
/// 覆盖层现在**直接回调**（不再走「事件队列 + 200ms 轮询」）：
/// 轮询版一旦定时器提前退出，按钮就再也关不掉覆盖层 —— 实测
/// 「再等 5 分钟」后第二次弹出时点关闭毫无反应。
fn dispatch_overlay_event(state: &Rc<RefCell<AppState>>, ev: OverlayEvent) {
    match ev {
        OverlayEvent::Dismissed => dismiss_cat(state),
        OverlayEvent::Snoozed => snooze_cat(state),
    }
}

/// 只关闭覆盖层窗口、复位「正在显示」标志，**不清空计时**。
///
/// 用在「用户从托盘/启动器打开了别的窗口」时：全屏覆盖层会把新窗口压在下面，
/// 表现为「点了菜单项，窗口开了但看不见也用不了」。
/// 与 `dismiss_cat` 的区别：不动 active_seconds / snooze_count，也不触发
/// `--test` 的退出逻辑。
fn close_overlays(state: &Rc<RefCell<AppState>>) {
    {
        let mut s = state.borrow_mut();
        if !s.is_overlay_showing && s.overlays.is_empty() {
            return;
        }
        s.is_overlay_showing = false;
        s.last_dismiss = Some(Instant::now());
        for ov in s.overlays.drain(..) {
            ov.close();
        }
    }
    refresh_tray(state);
}

fn dismiss_cat(state: &Rc<RefCell<AppState>>) {
    let mut s = state.borrow_mut();
    s.is_overlay_showing = false;
    s.active_seconds = 0;
    s.snooze_count = 0;
    s.snooze_threshold = 0;
    s.last_dismiss = Some(Instant::now());
    for ov in s.overlays.drain(..) {
        ov.close();
    }
    drop(s);
    log_line(state, "用户关闭提醒, 计时归零");
    refresh_tray(state);

    // `--test` 是一次性验证：关掉覆盖层就退出，别让命令一直挂着。
    if TEST_MODE.load(Ordering::Relaxed) {
        dbg_trace("--test 结束，退出");
        std::process::exit(0);
    }
}

fn snooze_cat(state: &Rc<RefCell<AppState>>) {
    let mut s = state.borrow_mut();
    if s.snooze_count >= 2 {
        return;
    }
    s.snooze_count += 1;
    s.is_overlay_showing = false;
    s.active_seconds = 0;
    s.snooze_threshold = 300; // 5 分钟后再次触发
    for ov in s.overlays.drain(..) {
        ov.close();
    }
    let n = s.snooze_count;
    drop(s);
    log_line(state, &format!("用户延后休息, 第{n}次"));
    refresh_tray(state);
}

/// 列出各显示器。
///
/// GTK4 的 `Display::monitors()` 返回 `gio::ListModel`，需用 `gio::ListModelExt`
/// 迭代并 downcast 成 `gdk::Monitor`。取不到时回退为「单个虚拟显示器」，
/// 让 `fullscreen()` 走默认屏幕，至少保证功能可用。
fn list_monitors() -> Vec<gtk::gdk::Monitor> {
    use gtk::gio::prelude::*;
    use gtk::prelude::*;

    let mut out: Vec<gtk::gdk::Monitor> = Vec::new();
    if let Some(display) = gtk::gdk::Display::default() {
        let model = display.monitors();
        for i in 0..model.n_items() {
            if let Some(obj) = model.item(i) {
                if let Ok(m) = obj.downcast::<gtk::gdk::Monitor>() {
                    out.push(m);
                }
            }
        }
    }
    out
}

fn builtin_webm_dir() -> std::path::PathBuf {
    if cfg!(debug_assertions) {
        if let Ok(cwd) = std::env::current_dir() {
            let c = cwd.join("data/webm");
            if c.exists() {
                return c;
            }
        }
    }
    std::path::PathBuf::from("/usr/share/purr-pause-linux/webm")
}

// ---------------------------------------------------------------------------
// 托盘
// ---------------------------------------------------------------------------

fn build_tray(state: &Rc<RefCell<AppState>>) {
    let tray = match Tray::new("胖猫暂停一下 Linux", "purr-pause-linux") {
        Some(t) => t,
        None => {
            log_line(state, "警告：无法创建托盘图标（缺少 libayatana-appindicator3）");
            eprintln!("{}", ui::tray::tray_fallback_hint());
            return;
        }
    };

    // 托盘实例必须保持存活，否则图标会从托盘消失。
    TRAY.with(|t| *t.borrow_mut() = Some(tray));
    // 首次一定发生变化，refresh_tray 会真正下发一次菜单
    refresh_tray(state);
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    /// 已初始化的应用状态，startup 时写入。
    /// 供 connect_activate 使用（「再次启动 → 打开设置」这条入口）。
    static STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
    /// GtkApplication 句柄，startup 时写入。
    /// 设置窗口要 `set_application()` 才能被应用管理（窗口列表 / 前置 / 分组）。
    static APP: RefCell<Option<adw::Application>> = const { RefCell::new(None) };
    /// 上一次下发的托盘菜单内容签名，用于去重（见 refresh_tray 的说明）。
    static TRAY_SIG: RefCell<String> = const { RefCell::new(String::new()) };
}

fn build_tray_items(state: &Rc<RefCell<AppState>>) -> Vec<TrayItem> {
    let s = state.borrow();

    let remaining = if s.is_paused {
        "监控已暂停".to_string()
    } else {
        let threshold = if s.snooze_threshold > 0 {
            s.snooze_threshold
        } else {
            s.config.threshold_minutes * 60
        };
        let mins = ((threshold - s.active_seconds).max(0) as f64 / 60.0).ceil() as i64;
        format!("距下次休息: {mins} 分钟")
    };

    let mut items = vec![TrayItem {
        label: remaining,
        sensitive: false,
        submenu: None,
        action: None,
    }];

    // 暂停 / 恢复
    //
    // ⚠️ 两个坑：
    //   1. 不要用 submenu —— GNOME 的 appindicator 扩展对二级菜单支持不稳定，
    //      实测「暂停监控」点了完全没反应（子菜单根本不弹）。四个选项平铺。
    //   2. 平铺后要能看出「当前选的是哪个」—— 用 dbusmenu 的 radio 勾选态
    //      （toggle-type + toggle-state），否则点了之后菜单里毫无反馈。
    // 「恢复监控」**恒定存在**（未暂停时禁用）。
    // 菜单结构必须固定 —— 结构一变，dbusmenu 的条目 id 就会整体挪位，
    // 宿主（GNOME 扩展）的菜单模型跟着错位，表现就是「点 A 触发了 B」。
    {
        let st = state.clone();
        items.push(TrayItem {
            label: "恢复监控".to_string(),
            sensitive: s.is_paused,
            submenu: None,
            action: Some(Box::new(move || resume_monitoring(&st))),
        });
    }
    {
        let paused = s.is_paused;
        let pm = s.paused_minutes;
        items.push(pause_item(state, "暂停 30 分钟", 30, paused && pm == 30));
        items.push(pause_item(state, "暂停 1 小时", 60, paused && pm == 60));
        items.push(pause_item(state, "暂停 2 小时", 120, paused && pm == 120));
        items.push(pause_item(state, "暂停到手动恢复", 0, paused && pm == 0));
    }

    let st_settings = state.clone();
    items.push(TrayItem {
        label: "设置".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || open_settings(&st_settings))),
    });

    let st_log = state.clone();
    items.push(TrayItem {
        label: "查看日志".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || open_log_dir(&st_log))),
    });

    let st_test = state.clone();
    items.push(TrayItem {
        label: "立即测试".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || trigger_cat(&st_test, true))),
    });
    items.push(TrayItem {
        label: format!("v{}", env!("CARGO_PKG_VERSION")),
        sensitive: false,
        submenu: None,
        action: None,
    });
    items.push(TrayItem {
        label: "退出".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(|| {
            std::process::exit(0);
        })),
    });

    items
}

/// 刷新托盘菜单（复用已存在的托盘实例）。
///
/// ⚠️ 必须做「内容没变就不下发」的去重：
/// 监控循环每 10 秒调一次本函数，而 `Tray::set_menu` 会重新分配全部条目 id、
/// 递增 revision 并广播 `LayoutUpdated`。宿主（GNOME 扩展）收到后会重建菜单模型 ——
/// 如果此时用户正开着菜单，菜单会被刷新掉/变得点不动，实测表现就是
/// 「菜单能打开，但点了没反应，也退不出去」。
fn refresh_tray(state: &Rc<RefCell<AppState>>) {
    let items = build_tray_items(state);
    if items.is_empty() {
        return;
    }

    // 第一个条目是「距下次休息: N 分钟」，它每分钟都会变。
    // **绝不能**因为它变化就整份重建菜单：重建会换一套 id + 广播 LayoutUpdated，
    // 宿主处理不干净就会错位（实测：动作整体错开一位、末尾多出一个「退出」）。
    // 结构签名只看其余条目，变了才重建；只有倒计时变了就单独推一个 label。
    let head_label = items[0].label.clone();
    let rest_sig = tray_signature(&items[1..]);
    let rest_changed = TRAY_SIG.with(|s| {
        let mut cur = s.borrow_mut();
        if *cur == rest_sig {
            false
        } else {
            *cur = rest_sig;
            true
        }
    });

    TRAY.with(|t| {
        if let Some(tray) = t.borrow_mut().as_mut() {
            if rest_changed {
                tray.set_menu(items);
            } else {
                // 第一个条目在 set_menu 里的 id 恒为 1
                tray.update_label(1, &head_label);
            }
        }
    });
}

/// 菜单内容签名（标签 + 可用性 + 层级结构）。动作闭包无法比较，
/// 但它们总是跟着内容一起重建，所以按内容判等足够。
fn tray_signature(items: &[TrayItem]) -> String {
    fn walk(items: &[TrayItem], out: &mut String) {
        for it in items {
            out.push_str(&it.label);
            out.push(if it.sensitive { '1' } else { '0' });
            if let Some(sub) = &it.submenu {
                out.push('[');
                walk(sub, out);
                out.push(']');
            }
            out.push('\n');
        }
    }
    let mut s = String::new();
    walk(items, &mut s);
    s
}

/// 在文件管理器中打开日志目录。
fn open_log_dir(state: &Rc<RefCell<AppState>>) {
    let path = state.borrow().log.path();
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    // 日志默认关闭（log_enabled=false）时目录可能根本还没建出来，
    // 直接 xdg-open 会失败并打印「日志目录尚不存在」。先建出来。
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    if !dir.exists() {
        eprintln!("无法创建日志目录：{}", dir.display());
        return;
    }
    // 用系统默认文件管理器打开
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
}

/// 发一条桌面通知。
///
/// 托盘菜单的状态刷新依赖宿主（GNOME 扩展）是否老实处理 LayoutUpdated，
/// 实测并不可靠。桌面通知是另一条通道，用户能立刻确认「操作生效了」。
fn notify(msg: &str) {
    let Some(app) = APP.with(|a| a.borrow().clone()) else {
        return;
    };
    let n = gtk::gio::Notification::new(msg);
    app.send_notification(None, &n);
}

fn pause_item(
    state: &Rc<RefCell<AppState>>,
    label: &str,
    minutes: i64,
    checked: bool,
) -> TrayItem {
    let st = state.clone();
    TrayItem {
        // 用标签前缀表示「当前选中」。
        // 之前用 dbusmenu 的 toggle-type=radio + toggle-state 做勾选，
        // 结果整份菜单的动作全部错位（点 A 触发 B），去掉后恢复正常。
        label: if checked {
            format!("✓ {label}")
        } else {
            label.to_string()
        },
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || pause_for(&st, minutes))),
    }
}

fn pause_for(state: &Rc<RefCell<AppState>>, minutes: i64) {
    {
        let mut s = state.borrow_mut();
        s.is_paused = true;
        s.paused_minutes = minutes;
    }
    log_line(state, &format!("暂停监控 {minutes} 分钟"));
    if minutes > 0 {
        notify(&format!("已暂停监控 {minutes} 分钟"));
    } else {
        notify("已暂停监控，直到手动恢复");
    }
    if minutes > 0 {
        let st = state.clone();
        glib::timeout_add_local_once(Duration::from_secs(minutes as u64 * 60), move || {
            resume_monitoring(&st);
        });
    }
    refresh_tray(state);
}

fn resume_monitoring(state: &Rc<RefCell<AppState>>) {
    {
        let mut s = state.borrow_mut();
        s.is_paused = false;
        s.paused_minutes = -1;
    }
    log_line(state, "恢复监控");
    notify("已恢复监控");
    refresh_tray(state);
}

fn open_settings(state: &Rc<RefCell<AppState>>) {
    // 覆盖层是全屏窗口，新开的设置窗口会被它压在下面 ——
    // 用户看到的就是「点了设置，窗口好像开了但完全没法用」。
    close_overlays(state);

    let cfg = state.borrow().config.clone();
    let idle_method = idle::detection_method().to_string();

    // 设置窗口需要关联到 GtkApplication（否则不进入应用窗口列表）。
    let Some(app) = APP.with(|a| a.borrow().clone()) else {
        eprintln!("[purr-pause-linux] Application 尚未初始化，无法打开设置窗口");
        return;
    };

    let st = state.clone();
    let st_test = state.clone();
    let st_log = state.clone();
    ui::settings::present(
        &app,
        cfg,
        idle_method,
        move |new_cfg| {
            let mut s = st.borrow_mut();
            let log_enabled = new_cfg.log_enabled;
            s.config = new_cfg.clone();
            s.log.set_enabled(log_enabled);
            if let Err(e) = config::save(&new_cfg) {
                let msg = format!("配置写入 {} 失败：{e}", config::user_config_path().display());
                eprintln!("[purr-pause-linux] {msg}");
                s.log.error(&msg);
            } else {
                s.log.info(&format!(
                    "配置已保存至 {}",
                    config::user_config_path().display()
                ));
            }
            drop(s);
            refresh_tray(&st);
        },
        move || {
            // 设置窗口里的「立即测试」：不依赖托盘，任何环境都能验证屏保
            trigger_cat(&st_test, true);
        },
        move || {
            open_log_dir(&st_log);
        },
    );
}
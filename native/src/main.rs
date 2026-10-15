//! 胖猫暂停一下（PurrPause）— Linux 原生版主程序。
//!
//! 主要功能：
//!   - 空闲计时，达到阈值后弹出猫咪覆盖层
//!   - 多显示器：每屏一个覆盖层窗口
//!   - 休息倒计时 + 延后（最多 2 次）
//!   - 托盘菜单：状态、剩余时间、暂停、设置、激活、退出
//!   - 消息提醒：轮询第三方待办接口，列表窗口，角标
//!
//! 技术要点：
//!   - 空闲检测直接走 gdbus，无需探测外部命令是否存在
//!   - 动画经 libmpv render API 渲染到 GtkGLArea（OpenGL，保留 alpha）；
//!     libmpv 在运行时 dlopen，兼容 libmpv.so.2 / libmpv.so.1

mod core;
mod license;
mod message;
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
use license::LicenseStatus;
use ui::overlay::{CatOverlay, OverlayEvent};
use ui::tray::{Tray, TrayItem};

const APP_ID: &str = "com.purr-pause-linux.app";
const MONITOR_INTERVAL_MS: u64 = 10_000;
/// idle 超过该值视为离开（10 分钟）。
const IDLE_RESET_MS: u64 = 10 * 60 * 1000;
/// idle 小于该值视为活跃（3 分钟）。
const IDLE_ACTIVE_MS: u64 = 180 * 1000;

/// 应用共享状态。
pub struct AppState {
    pub config: Config,
    active_seconds: i64,
    snooze_count: i64,
    snooze_threshold: i64,
    is_paused: bool,
    is_overlay_showing: bool,
    last_dismiss: Option<Instant>,
    overlays: Vec<CatOverlay>,
    pub log: Logger,
    pub badge_count: i64,
    /// 消息提醒：每源运行时状态
    pub sources: Vec<message::SourceRuntime>,
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

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // 不设 NON_UNIQUE：GApplication 默认借助 D-Bus well-known name 保证单实例。
    // 若启动第二个进程，它会把 activate 信号转发给已有实例后立即退出。
        // （早期版本误设 NON_UNIQUE —— 该标志恰恰是「允许多实例」，
        //   与注释意图相反，会导致同时跑多个托盘与多套计时器。）
        .build();

    // 注意：hold() 已移出本闭包（见下方 main 作用域），
    // 故此处参数不再使用，用 `_app` 避免 unused_variables 警告。
    // ⚠️ 初始化放在 startup（**只跑一次**），不要放回 connect_activate。
    //
    // GApplication 在**每次**被激活时都会发 activate —— 包括第二个实例通过
    // D-Bus 转发过来的激活。原先整段初始化写在 activate 里，导致用户再点一次
    // 图标就会把一切重来一遍：重复的计时器、重复的消息轮询、重复的托盘。
    app.connect_startup(move |_app| {
        dbg_trace("startup：开始初始化");
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

        // build_sources 借用 cfg，必须在把 cfg 移入 AppState 之前算好（E0382）
        let sources = build_sources(&cfg);

        let state = Rc::new(RefCell::new(AppState {
            config: cfg,
            active_seconds: 0,
            snooze_count: 0,
            snooze_threshold: 0,
            is_paused: false,
            is_overlay_showing: false,
            last_dismiss: None,
            overlays: Vec::new(),
            log,
            badge_count: 0,
            sources,
            is_screen_locked: false,
        }));

        log_line(&state, &format!("应用启动, 版本 {}", env!("CARGO_PKG_VERSION")));
        log_line(&state, &format!("空闲检测方式: {}", idle::detection_method()));

        // 检查授权
        let status = license::check_status();
        log_line(
            &state,
            &format!("许可证状态: {:?}", status.status),
        );

        if status.status == LicenseStatus::Expired {
            show_activation(&state);
        }

        build_tray(&state);
        start_monitoring(&state);
        start_message_poll(&state);

        // 供 activate 回调取用（见下方 connect_activate）
        STATE.with(|s| *s.borrow_mut() = Some(state.clone()));

        // 应用常驻：GApplication 在 use_count 归零（无窗口、无 hold）时退出。
        // ⚠️ 两个要点：
        //   1. hold() 必须在注册之后调用 —— startup 信号正是在 register 之后、
        //      activate 之前触发的，位置正确；原先写在 main 里 run() 之前，
        //      属于「未注册就 hold」，行为不可靠。
        //   2. guard 有意 forget：守护进程存活期间永不释放（退出走
        //      设置窗口的「退出应用」按钮 / 未来的托盘菜单）。
        //      若正常绑定到变量，main 返回时 guard 析构 = hold 白做。
        std::mem::forget(_app.hold());

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
        let Some(state) = STATE.with(|s| s.borrow().clone()) else {
            dbg_trace("activate：⚠️ STATE 未初始化（startup 未跑过？），忽略");
            return;
        };
        if license::check_status().status == LicenseStatus::Expired {
            dbg_trace("activate：授权过期 → 打开激活窗口");
            show_activation(&state);
        } else {
            dbg_trace("activate：打开设置窗口");
            open_settings(&state);
        }
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
    for monitor in monitors {
        let overlay = CatOverlay::new(
            cfg.animation_mode.clone(),
            break_minutes,
            snooze_count,
            walk.clone(),
            idle_v.clone(),
        );
        let geo = monitor.geometry();
        overlay.window.set_default_size(geo.width(), geo.height());
        overlay.window.fullscreen_on_monitor(&monitor);
        overlay.window.present();
        state.borrow_mut().overlays.push(overlay);
    }

    log_line(state, "触发猫咪覆盖层");

    // 轮询覆盖层事件
    let state_poll = state.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let events: Vec<OverlayEvent> = {
            let s = state_poll.borrow();
            let mut ev = Vec::new();
            for ov in s.overlays.iter() {
                ev.extend(ov.take_events());
            }
            ev
        };
        for e in events {
            match e {
                OverlayEvent::Dismissed => {
                    dismiss_cat(&state_poll);
                }
                OverlayEvent::Snoozed => {
                    snooze_cat(&state_poll);
                }
            }
        }
        // 注意：必须在上面的借用释放之后再取值，且不必先初始化再赋值
        // （原写法 `let mut alive = false;` 会触发 unused_assignments）
        let alive = {
            let s = state_poll.borrow();
            s.is_overlay_showing && !s.overlays.is_empty()
        };
        if alive {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
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

/// 根据配置构造各消息源的运行时状态。
fn build_sources(cfg: &Config) -> Vec<message::SourceRuntime> {
    cfg.message_sources
        .iter()
        .map(|s| {
            message::SourceRuntime::new(
                s.id.clone(),
                s.name.clone(),
                s.list_url.clone(),
                s.auth_headers.clone(),
                s.enabled,
            )
        })
        .collect()
}

/// 启动消息提醒轮询。每个源独立计时，首轮只建基线不弹动画。
fn start_message_poll(state: &Rc<RefCell<AppState>>) {
    let cfg = state.borrow().config.clone();
    if !cfg.message_notify_enabled {
        return;
    }
    let enabled_count = cfg.message_sources.iter().filter(|s| s.enabled).count();
    if enabled_count == 0 {
        return;
    }
    log_line(state, &format!("消息提醒轮询已启动, 启用源数={enabled_count}"));

    let client = message::build_client();

    for (idx, _src) in cfg.message_sources.iter().enumerate() {
        // 错峰启动，避免同时打接口
        let delay = std::time::Duration::from_millis(idx as u64 * 300);
        let st = state.clone();
        let client = client.clone();

        glib::timeout_add_local_once(delay, move || {
            poll_once(&st, &client);
            schedule_next(&st, &client);
        });
    }
}

/// 排下一次轮询（按各源失败次数应用指数退避）。
fn schedule_next(state: &Rc<RefCell<AppState>>, client: &reqwest::Client) {
    let interval = {
        let s = state.borrow();
        // 消息提醒被关闭时不再续排，避免留下永不休眠的空转定时器。
        // 重新开启时由 toggle_message_notify 重新启动轮询链。
        if !s.config.message_notify_enabled {
            log_line(state, "消息提醒已关闭，停止轮询");
            return;
        }
        // 取启用源中最小的下一次轮询间隔：
        // 正常时为配置的基准间隔；连续失败 >=5 次的源按 2^(n-4) 退避（最高 300s）
        let base = s.config.message_poll_interval.max(10) as u64;
        s.sources
            .iter()
            .filter(|r| r.enabled && !r.list_url.is_empty())
            .map(|r| r.poll_interval_secs(base as i64))
            .min()
            .unwrap_or(base)
    };
    let st = state.clone();
    let c = client.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(interval), move || {
        poll_once(&st, &c);
        schedule_next(&st, &c);
    });
}

/// 执行一轮轮询。
fn poll_once(state: &Rc<RefCell<AppState>>, client: &reqwest::Client) {
    // 取出待轮询的源快照，释放 borrow 后再发请求
    let jobs: Vec<(usize, String, String)> = {
        let s = state.borrow();
        if !s.config.message_notify_enabled {
            return;
        }
        s.sources
            .iter()
            .enumerate()
            .filter(|(_, rt)| rt.enabled && !rt.list_url.is_empty())
            .map(|(i, rt)| (i, rt.list_url.clone(), rt.auth_headers.clone()))
            .collect()
    };

    if jobs.is_empty() {
        return;
    }

    let max_items = state.borrow().config.message_max_cache_items;
    let rt_names: Vec<String> = jobs
        .iter()
        .map(|(i, _, _)| state.borrow().sources[*i].name.clone())
        .collect();

    // 同步阻塞请求会卡住 UI，这里限制为串行单次，
    // 单源超时 5 秒，最坏情况下阻塞主循环 5 秒 —— 需在真机验证是否可接受。
    // TODO: 改用 gio::async 或独立线程 + channel。
    for ((idx, list_url, auth_headers), name) in jobs.into_iter().zip(rt_names) {
        // GTK 主循环无法直接 await，这里用阻塞执行器把 async future 拉平。
        // 单源超时 5 秒（见 message::build_client 的 timeout 设置），最坏情况会阻塞主循环数秒。
        let probe = message::SourceRuntime::new(
            "tmp".into(),
            name.clone(),
            list_url,
            auth_headers,
            true,
        );
        let result = block_on(message::fetch_source(client, &probe, max_items));

        // 先把结果应用到 target 上并取出要记的日志，
        // 然后释放 target 对 s 的可变借用，才能再借用 s.log（E0502）。
        let pending: Option<(bool, String)> = {
            let mut s = state.borrow_mut();
            let target = &mut s.sources[idx];
            let entry = match result {
                Ok((total, items)) => {
                    target.consecutive_failures = 0;
                    target.last_error.clear();
                    target.list_in_flight = false;
                    let is_first = target.first_sync;
                    let new_count = message::apply_items(target, items, total);
                    target.first_sync = false;
                    // true = info, false = warn
                    (!is_first && new_count > 0).then(|| {
                        (
                            true,
                            format!("[消息提醒][{name}] 检测到 {new_count} 条新待办"),
                        )
                    })
                }
                Err(e) => {
                    target.consecutive_failures += 1;
                    target.last_error = e.clone();
                    target.list_in_flight = false;
                    let n = target.consecutive_failures;
                    Some((
                        false,
                        format!("[消息提醒][{name}] 拉取失败：{e}（连续失败 {n} 次）"),
                    ))
                }
            };
            s.badge_count = s.sources.iter().filter(|r| r.enabled).map(|r| r.badge_count).sum();
            entry
        };

        // 此时已无可变借用
        match pending {
            Some((true, msg)) => log_line(state, &msg),
            Some((false, msg)) => {
                let s = state.borrow();
                s.log.warn(&msg);
            }
            None => {}
        }
        refresh_tray(state);
    }
}

/// 简易阻塞执行器：把 async future 适配到 GTK 主循环。
/// 仅用于消息轮询这类低频、可容忍短暂阻塞的场景。
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    let mut fut = Box::pin(fut);
    let waker = Waker::from(Arc::new(NoopWaker));
    let mut cx = Context::from_waker(&waker);

    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            // reqwest 的 I/O 由内部 runtime 驱动，Pending 时短暂让出即可
            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

// ---------------------------------------------------------------------------
// 托盘
// ---------------------------------------------------------------------------

fn build_tray(state: &Rc<RefCell<AppState>>) {
    let mut tray = match Tray::new("胖猫暂停一下 Linux", "purr-pause-linux") {
        Some(t) => t,
        None => {
            log_line(state, "警告：无法创建托盘图标（缺少 libayatana-appindicator3）");
            eprintln!("{}", ui::tray::tray_fallback_hint());
            return;
        }
    };

    let items = build_tray_items(state);
    tray.set_menu(&items);
    // 托盘实例必须保持存活，否则图标会从托盘消失。
    TRAY.with(|t| *t.borrow_mut() = Some(tray));
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    /// 已初始化的应用状态，startup 时写入。
    /// 供 connect_activate 使用（「再次启动 → 打开设置」这条入口）。
    static STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
}

fn build_tray_items(state: &Rc<RefCell<AppState>>) -> Vec<TrayItem> {
    let s = state.borrow();
    let status = license::check_status();
    let status_label = match status.status {
        LicenseStatus::Trial => format!("试用中（剩余 {} 天）", status.days_left),
        LicenseStatus::Active if status.is_permanent => "已激活（永久）".to_string(),
        LicenseStatus::Active => format!("已激活（剩余 {} 天）", status.days_left),
        LicenseStatus::Expired => "未激活".to_string(),
    };

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

    let mut items = vec![
        TrayItem {
            label: status_label,
            sensitive: false,
            submenu: None,
            action: None,
        },
        TrayItem {
            label: remaining,
            sensitive: false,
            submenu: None,
            action: None,
        },
    ];

    // 待办消息：点击打开列表窗口
    let st_msg = state.clone();
    items.push(TrayItem {
        label: format!("待办消息（{}）", s.badge_count),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || open_message_list(&st_msg))),
    });
    // 消息提醒开关：直接在托盘里切换，无需进设置窗口
    let st_toggle = state.clone();
    let notify_label = if s.config.message_notify_enabled {
        "关闭消息提醒"
    } else {
        "开启消息提醒"
    };
    items.push(TrayItem {
        label: notify_label.to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || toggle_message_notify(&st_toggle))),
    });

    // 分隔留白（不可点击）
    items.push(TrayItem {
        label: "---".to_string(),
        sensitive: false,
        submenu: None,
        action: None,
    });

    // 暂停 / 恢复
    if s.is_paused {
        let st = state.clone();
        items.push(TrayItem {
            label: "恢复监控".to_string(),
            sensitive: true,
            submenu: None,
            action: Some(Box::new(move || resume_monitoring(&st))),
        });
    } else {
        items.push(TrayItem {
            label: "暂停监控".to_string(),
            sensitive: true,
            submenu: Some(vec![
                pause_item(state, "30 分钟", 30),
                pause_item(state, "1 小时", 60),
                pause_item(state, "2 小时", 120),
                pause_item(state, "直到手动恢复", 0),
            ]),
            action: None,
        });
    }

    let st_settings = state.clone();
    items.push(TrayItem {
        label: "设置".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || open_settings(&st_settings))),
    });

    let st_act = state.clone();
    items.push(TrayItem {
        label: "激活/续期".to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || show_activation(&st_act))),
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
fn refresh_tray(state: &Rc<RefCell<AppState>>) {
    let items = build_tray_items(state);
    TRAY.with(|t| {
        if let Some(tray) = t.borrow_mut().as_mut() {
            tray.set_menu(&items);
        }
    });
}

/// 打开待办消息列表窗口。
///
/// 即使消息提醒未开启也照常打开 —— 列表窗口内部会显示「未启用」占位文案，
/// 这样用户能从窗口里直接看到原因，而不是只看到一句终端输出。
fn open_message_list(state: &Rc<RefCell<AppState>>) {
    let snapshot = {
        let s = state.borrow();
        message::aggregate(&s.sources, s.config.message_notify_enabled)
    };
    ui::message_list::present(snapshot, state.clone());
}

/// 在托盘里切换消息提醒总开关。
fn toggle_message_notify(state: &Rc<RefCell<AppState>>) {
    let (enabled, has_enabled_source) = {
        let mut s = state.borrow_mut();
        s.config.message_notify_enabled = !s.config.message_notify_enabled;
        let has = s
            .config
            .message_sources
            .iter()
            .any(|src| src.enabled && !src.list_url.is_empty());
        (s.config.message_notify_enabled, has)
    };

    let cfg = state.borrow().config.clone();
    let _ = config::save(&cfg);

    if enabled {
        if has_enabled_source {
            log_line(state, "消息提醒已开启，启动轮询");
            start_message_poll(state);
        } else {
            log_line(state, "消息提醒已开启，但没有已启用的接口源");
        }
    } else {
        log_line(state, "消息提醒已关闭");
    }

    refresh_tray(state);
}

/// 在文件管理器中打开日志目录。
fn open_log_dir(state: &Rc<RefCell<AppState>>) {
    let path = state.borrow().log.path();
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    if !dir.exists() {
        eprintln!("日志目录尚不存在：{}", dir.display());
        return;
    }
    // 用系统默认文件管理器打开
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
}

fn pause_item(state: &Rc<RefCell<AppState>>, label: &str, minutes: i64) -> TrayItem {
    let st = state.clone();
    TrayItem {
        label: label.to_string(),
        sensitive: true,
        submenu: None,
        action: Some(Box::new(move || pause_for(&st, minutes))),
    }
}

fn pause_for(state: &Rc<RefCell<AppState>>, minutes: i64) {
    {
        let mut s = state.borrow_mut();
        s.is_paused = true;
    }
    log_line(state, &format!("暂停监控 {minutes} 分钟"));
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
    }
    log_line(state, "恢复监控");
    refresh_tray(state);
}

fn open_settings(state: &Rc<RefCell<AppState>>) {
    let cfg = state.borrow().config.clone();
    let idle_method = idle::detection_method().to_string();
    let activated = license::check_status().status == LicenseStatus::Active;
    let st = state.clone();
    ui::settings::present(cfg, activated, idle_method, move |new_cfg| {
        let mut s = st.borrow_mut();
        let log_enabled = new_cfg.log_enabled;
        s.config = new_cfg.clone();
        s.log.set_enabled(log_enabled);
        let _ = config::save(&new_cfg);
        s.log.info("配置已保存");
        drop(s);
        refresh_tray(&st);
    });
}

fn show_activation(state: &Rc<RefCell<AppState>>) {
    let st = state.clone();
    ui::activation::present(move || {
        log_line(&st, "激活成功");
        refresh_tray(&st);
    });
}
//! 猫咪覆盖层：透明全屏窗口 + 四种动画模式 + 休息倒计时。
//!
//! 四种动画模式（与 electron 版一致）：
//!   walk-center  中央原地放大（默认）
//!   walk-zoom    从右侧走进中央，同时放大
//!   walk-flat    恒定大小从右侧走进中央
//!   fade-center  从极小缩放淡入中央
//!
//! ## ⚠️ 本文件最重要的设计约束：**永远不能出现「看不见又关不掉」的窗口**
//!
//! v1.0.0 ~ v1.0.x 的真实故障链：
//!   1. 窗口背景被设成透明（修「黑屏」时加的），
//!   2. mpv / GL 任一环节失败 → GLArea 什么都不画，
//!   3. 于是屏幕上**什么都没有**，但那个全屏窗口仍然存在、仍然吃鼠标点击，
//!   4. 用户点不到托盘 → 设置打不开、测试也没法测，
//!   5. 窗口 `deletable(false)` + 无退出按钮 → 只能 kill 进程。
//!
//! 因此这里做了三层兜底，任何一层生效都不会再卡死：
//!   A. **静态猫咪图兜底**：窗口里始终垫着一张猫图（`logo.png`），
//!      视频出不来时用户看到的是「静止的猫 + 倒计时 + 按钮」，而不是空屏。
//!   B. **控制条按时间强制出现**：不等视频结束，最迟 1.2 秒必定显示
//!      倒计时和「精力充沛！/ 关闭」按钮。
//!   C. **三条逃生通道**：Esc、双击画面、标题栏/Alt+F4（`deletable(true)`）。
//!
//! 另外，几何动画完全由**时间**驱动，不再依赖「视频是否就绪」——
//! 之前的实现要等 mpv 出第一帧才开始动画，mpv 一失败猫就永远不出现。

// glib 是 gtk4 的依赖，须经 gtk 路径引入（Rust 2018 起不能用裸 crate 名）
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::video::VideoPlayer;

const BASE_CAT_HEIGHT: f64 = 200.0;
const MAX_CAT_SCALE: f64 = 5.0;
const MAX_SCREEN_HEIGHT_RATIO: f64 = 0.85;
const MAX_SCREEN_WIDTH_RATIO: f64 = 0.9;

/// 走路动画时长（毫秒）。
const WALK_MS: f64 = 3000.0;
/// 走路兜底：超过这个时间无论视频是否播完都进入休息态。
const WALK_FALLBACK_MS: f64 = 5200.0;
/// 控制条最迟出现时间 —— 用户必须能随时关掉它。
const BAR_DEADLINE_MS: f64 = 1200.0;
/// 视频多久没出帧就判定失败，降级为静态猫图。
const VIDEO_DEADLINE_MS: f64 = 3000.0;
/// 主循环节拍（几何 / 倒计时 / 事件泵）。
///
/// 必须是「帧级」的：这个节拍同时驱动猫咪的位移与缩放动画，
/// 之前写 40ms（=25fps）时走路明显一顿一顿的。
const TICK_MS: u64 = 16;
/// 渲染泵节拍。独立于逻辑节拍，只负责「mpv 有新帧就 queue_render」，
/// 这样播放帧率不受逻辑节拍限制。
const RENDER_PUMP_MS: u64 = 8;

pub enum OverlayEvent {
    Dismissed,
    Snoozed,
}

fn ease_out_cubic(p: f64) -> f64 {
    1.0 - (1.0 - p).powi(3)
}

/// 猫咪在窗口内的几何状态。x/y 为中心点坐标（像素）。
#[derive(Clone, Copy, Debug)]
pub struct CatState {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub opacity: f64,
}

impl Default for CatState {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 0.05,
            opacity: 0.0,
        }
    }
}

/// 绘制所需的共享上下文。
struct Ctx {
    state: Rc<RefCell<CatState>>,
    /// 素材宽高比，视频就绪后由 mpv 回报。
    aspect: Rc<RefCell<f64>>,
    win_w: Cell<f64>,
    win_h: Cell<f64>,
    /// 承载「静态猫图 + GL 视频层」的容器。
    frame: gtk::Fixed,
    /// 静态猫图（视频不可用时的兜底视觉）。
    fallback: gtk::Widget,
    /// 视频层。
    gl_area: gtk::GLArea,
    /// mpv 播放器只能在 GL 上下文建立后创建，故用 Option 延迟初始化。
    player: Rc<RefCell<Option<VideoPlayer>>>,
    /// 累计出帧数 —— 用来判断视频是否真的在渲染。
    frames: Rc<Cell<u32>>,
    /// 是否已经尝试过创建播放器。**只试一次**：失败后每帧重试会刷屏，
    /// 实测每 40ms 一条 `视频初始化失败` + 两条 Gtk-CRITICAL。
    tried: Cell<bool>,
    /// 视频是否已判定不可用（降级为静态猫图）。
    failed: Cell<bool>,
    /// 上一次应用的几何 (cx, cy, w, h, opacity)，用来跳过无变化的重复布局。
    last_geo: Cell<(f64, f64, i32, i32, f64)>,
    /// 覆盖层是否还活着。窗口关闭后两个定时器都要停，否则会一直持有 Ctx。
    alive: Cell<bool>,
    /// 控制条与其上的提示文字。
    bar: gtk::Box,
    countdown: gtk::Label,
    hint: gtk::Label,
}

/// 覆盖层窗口。
pub struct CatOverlay {
    pub window: gtk::Window,
    ctx: Rc<Ctx>,
}

impl CatOverlay {
    pub fn new(
        animation_mode: String,
        break_minutes: i64,
        snooze_count: i64,
        walk_video: PathBuf,
        idle_video: PathBuf,
        on_event: impl Fn(OverlayEvent) + 'static,
    ) -> Self {
        install_css();

        // deletable(true)：允许窗口管理器关闭（Alt+F4）。
        // 之前写 false，配合「透明 + 无内容」就是彻底关不掉。
        let window = gtk::Window::builder()
            .title("胖猫暂停一下")
            .decorated(false)
            .resizable(false)
            .deletable(true)
            .build();
        window.add_css_class("cat-overlay");
        // ⚠️ GTK 会自发给 toplevel 加一个 `.background` 样式类，Adwaita 主题的
        //    `window.background { background-color: @window_bg_color }` 就靠它上色。
        //    只写 `window.cat-overlay { background: transparent }` 在部分主题下
        //    会被压掉 —— 表现就是「猫抠出来了，但周围一圈黑底」。
        //    直接摘掉这个类，主题规则就不再匹配本窗口。
        window.remove_css_class("background");

        // ── 画面层：静态猫图在下，GL 视频层在上 ──────────────────────
        let frame = gtk::Fixed::new();
        frame.set_hexpand(true);
        frame.set_vexpand(true);

        let fallback = fallback_widget();
        // 先藏起来：视频正常时不应该在猫身后透出一张静态照片。
        // 只有在「判定视频渲染失败」时才把它亮出来兜底。
        fallback.set_visible(false);
        frame.put(&fallback, 0.0, 0.0);

        let gl_area = gtk::GLArea::new();
        gl_area.set_auto_render(false);
        gl_area.set_has_depth_buffer(false);
        gl_area.set_has_stencil_buffer(false);
        // GLArea 没有固有尺寸，必须显式给尺寸请求，否则宽高恒为 0（视频不可见）
        gl_area.set_size_request(1, 1);
        frame.put(&gl_area, 0.0, 0.0);

        // ── 控制条 ───────────────────────────────────────────────────
        let bar = gtk::Box::new(gtk::Orientation::Vertical, 10);
        bar.add_css_class("cat-bar");
        bar.set_halign(gtk::Align::Center);
        bar.set_valign(gtk::Align::End);
        bar.set_margin_bottom(40);
        bar.set_visible(false);

        let countdown = gtk::Label::new(Some(""));
        countdown.add_css_class("cat-countdown");
        countdown.set_halign(gtk::Align::Center);
        bar.append(&countdown);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        buttons.set_halign(gtk::Align::Center);

        let snooze_btn = gtk::Button::with_label("再等 5 分钟");
        snooze_btn.add_css_class("cat-pill");
        if snooze_count >= 2 {
            snooze_btn.set_sensitive(false);
            snooze_btn.set_label("已用完");
        } else {
            snooze_btn.set_label(&format!("再等 5 分钟（{}次）", 2 - snooze_count.max(0)));
        }

        let dismiss_btn = gtk::Button::with_label("精力充沛！");
        dismiss_btn.add_css_class("cat-pill");
        dismiss_btn.add_css_class("suggested-action");

        // 永远存在的逃生按钮：即使倒计时逻辑出问题，也能立刻关掉覆盖层。
        let close_btn = gtk::Button::with_label("关闭");
        close_btn.add_css_class("cat-pill");

        buttons.append(&snooze_btn);
        buttons.append(&dismiss_btn);
        buttons.append(&close_btn);
        bar.append(&buttons);

        let hint = gtk::Label::new(Some(""));
        hint.add_css_class("cat-hint");
        hint.set_halign(gtk::Align::Center);
        hint.set_wrap(true);
        hint.set_visible(false);
        bar.append(&hint);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&frame);
        root.append(&bar);
        window.set_child(Some(&root));

        // 事件**直接回调**给 main，不再用「Vec + 200ms 轮询」。
        // 轮询版有个致命缺陷：只要那个定时器因为任何原因先退出，
        // 按钮就再也关不掉覆盖层 —— 实测「再等 5 分钟」后第二次弹出时
        // 点关闭毫无反应，正是这个原因。
        let cb: Rc<dyn Fn(OverlayEvent)> = Rc::new(on_event);
        let emit: Rc<dyn Fn(OverlayEvent)> = {
            let cb = cb.clone();
            Rc::new(move |ev| {
                let cb = cb.clone();
                // 延后到下一轮主循环再执行：在自己的点击回调里直接销毁
                // 所在的窗口容易踩重入。
                glib::idle_add_local_once(move || cb(ev));
            })
        };

        let ctx = Rc::new(Ctx {
            state: Rc::new(RefCell::new(CatState::default())),
            aspect: Rc::new(RefCell::new(16.0 / 9.0)),
            win_w: Cell::new(0.0),
            win_h: Cell::new(0.0),
            frame: frame.clone(),
            fallback,
            gl_area: gl_area.clone(),
            player: Rc::new(RefCell::new(None)),
            frames: Rc::new(Cell::new(0)),
            tried: Cell::new(false),
            failed: Cell::new(false),
            last_geo: Cell::new((f64::NAN, f64::NAN, -1, -1, f64::NAN)),
            alive: Cell::new(true),
            bar: bar.clone(),
            countdown: countdown.clone(),
            hint: hint.clone(),
        });

        // ── GL 渲染回调 ─────────────────────────────────────────────
        {
            let c = ctx.clone();
            gl_area.connect_render(move |area, _gl_ctx| {
                // 已判定失败 → 不再碰 GL，避免每帧刷屏
                if c.failed.get() {
                    return glib::Propagation::Stop;
                }
                // mpv 只在「已 current」的 GL 上下文上发指令，每帧都要先 make_current
                area.make_current();
                if c.player.borrow().is_none() {
                    // ⚠️ 只尝试一次。旧实现在每次 render 里重试，失败时
                    //    每 40ms 打印一条错误（实测刷满整个终端）。
                    if c.tried.get() {
                        return glib::Propagation::Stop;
                    }
                    c.tried.set(true);
                    match VideoPlayer::new(area) {
                        Ok(p) => *c.player.borrow_mut() = Some(p),
                        Err(e) => {
                            eprintln!("[purr-pause-linux] 视频初始化失败：{e}");
                            // 只置标志：界面上的降级动作交给主节拍做
                            // （在自己的 render 回调里改可见性容易踩重入）
                            c.failed.set(true);
                            c.hint
                                .set_text(&format!("视频渲染不可用，已降级为静态猫咪：{e}"));
                            c.hint.set_visible(true);
                            return glib::Propagation::Stop;
                        }
                    }
                }
                let w = area.width();
                let h = area.height();
                if let Some(p) = c.player.borrow_mut().as_mut() {
                    p.draw(w, h);
                }
                glib::Propagation::Proceed
            });
        }

        // GL 上下文销毁前释放 mpv render context（此时上下文通常仍可 current）
        {
            let c = ctx.clone();
            gl_area.connect_unrealize(move |_area| {
                if let Some(p) = c.player.borrow_mut().as_mut() {
                    p.shutdown();
                }
            });
        }

        // auto_render=false 时 GTK 不会自动重绘，映射后必须主动请求一帧，
        // 否则 render 回调永远不触发 → 播放器永远不创建。
        gl_area.connect_map(|area| area.queue_render());

        // ── 逃生通道 ───────────────────────────────────────────────
        // 1) 点窗口关闭按钮 / Alt+F4
        {
            let emit = emit.clone();
            let c = ctx.clone();
            window.connect_close_request(move |_| {
                // 窗口被外部关掉时也要停掉定时器，否则它们会一直空转
                c.alive.set(false);
                emit(OverlayEvent::Dismissed);
                glib::Propagation::Proceed
            });
        }
        // 2) Esc
        {
            let key = gtk::EventControllerKey::new();
            let emit = emit.clone();
            key.connect_key_pressed(move |_, k, _, _| {
                if k == gtk::gdk::Key::Escape {
                    emit(OverlayEvent::Dismissed);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            window.add_controller(key);
        }
        // 3) 双击画面（挂在 frame 上，不干扰控制条按钮）
        {
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_PRIMARY);
            let emit = emit.clone();
            click.connect_pressed(move |_, n_press, _, _| {
                if n_press >= 2 {
                    emit(OverlayEvent::Dismissed);
                }
            });
            frame.add_controller(click);
        }

        // ── 按钮 ───────────────────────────────────────────────────
        {
            let e = emit.clone();
            dismiss_btn.connect_clicked(move |_| e(OverlayEvent::Dismissed));
            let e = emit.clone();
            close_btn.connect_clicked(move |_| e(OverlayEvent::Dismissed));
            let e = emit.clone();
            snooze_btn.connect_clicked(move |_| e(OverlayEvent::Snoozed));
        }

        begin(
            ctx.clone(),
            emit.clone(),
            animation_mode,
            walk_video,
            idle_video,
            break_minutes,
        );

        Self { window, ctx }
    }

    pub fn close(&self) {
        // 先停定时器再关窗：两个定时器都持有 Ctx，不停会一直空转
        self.ctx.alive.set(false);
        self.window.close();
    }
}

/// 驱动整个覆盖层：几何动画、视频、倒计时全部由时间推进。
///
/// 用「时间戳 + 定时器」而不是「四五个嵌套 timeout」的原因：
/// 旧实现里动画启动挂在「mpv 出第一帧」的回调链上（50ms 等 player →
/// 再 30ms 取尺寸 → 尺寸为 0 就直接放弃且不再重试），
/// 只要 mpv 慢一点或失败，猫就永远不出现，而且没有任何重试机会。
///
/// 两个定时器分工：
///   · 8ms 渲染泵 —— 只负责把 mpv 的新帧 queue_render 出去；
///   · 16ms 逻辑节拍 —— 几何动画、事件泵、倒计时。
fn begin(
    ctx: Rc<Ctx>,
    emit: Rc<dyn Fn(OverlayEvent)>,
    mode: String,
    walk_video: PathBuf,
    idle_video: PathBuf,
    break_minutes: i64,
) {
    let started = Rc::new(Cell::new(false));
    let switched = Rc::new(Cell::new(false));
    let inited = Rc::new(Cell::new(false));
    let bar_shown = Rc::new(Cell::new(false));
    let countdown_start: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let dumped = Rc::new(Cell::new(false));
    let t0 = Instant::now();
    let total_secs = break_minutes.max(1) as u64 * 60;

    // ── 渲染泵 ────────────────────────────────────────────────────
    // 单独用 8ms 轮询 mpv 的「有新帧」标志，命中就 queue_render。
    // 之前这件事混在 40ms 的逻辑节拍里做，等于把播放上限压到 25fps（卡顿）。
    // 不用 add_tick_callback 的原因：那会让整个全屏窗口每帧都重绘，
    // 空闲时也一直烧 GPU；这里只在真有新帧时才 queue_render。
    {
        let ctx_pump = ctx.clone();
        glib::timeout_add_local(Duration::from_millis(RENDER_PUMP_MS), move || {
            if !ctx_pump.alive.get() {
                return glib::ControlFlow::Break;
            }
            let need = ctx_pump
                .player
                .borrow_mut()
                .as_mut()
                .map(|p| p.take_update_flag())
                .unwrap_or(false);
            if need {
                ctx_pump.frames.set(ctx_pump.frames.get() + 1);
                ctx_pump.gl_area.queue_render();
            }
            glib::ControlFlow::Continue
        });
    }

    glib::timeout_add_local(Duration::from_millis(TICK_MS), move || {
        if !ctx.alive.get() {
            return glib::ControlFlow::Break;
        }
        let t = t0.elapsed().as_millis() as f64;

        // 1) 几何初始化：窗口尺寸没准备好就一直重试（旧实现在这里直接放弃）
        if !inited.get() {
            let w = ctx.frame.width() as f64;
            let h = ctx.frame.height() as f64;
            if w > 4.0 && h > 4.0 {
                ctx.win_w.set(w);
                ctx.win_h.set(h);
                init_state(&ctx, &mode, w, h);
                inited.set(true);
            }
        }

        // 2) 起播走路视频（播放器就绪后）
        if !started.get() {
            let mut p = ctx.player.borrow_mut();
            if let Some(player) = p.as_mut() {
                match player.load(&walk_video) {
                    Ok(()) => {
                        player.set_loop(false);
                        player.play();
                        started.set(true);
                    }
                    Err(e) => {
                        eprintln!("[purr-pause-linux] 走路素材加载失败：{e}");
                        ctx.hint.set_text(&format!("走路素材加载失败：{e}"));
                        ctx.hint.set_visible(true);
                        ctx.failed.set(true);
                        // 素材读不出来 → 立刻降级成静态猫图，别留一个空窗口
                        ctx.gl_area.set_visible(false);
                        ctx.fallback.set_visible(true);
                        *ctx.aspect.borrow_mut() = 1.0;
                        started.set(true); // 不再重试，但动画照常跑
                    }
                }
            }
        }

        // 3) 动画 + 应用几何（纯时间驱动，不依赖视频）
        if inited.get() {
            animate(&ctx, &mode, t);
            apply_geometry(&ctx);
        }

        // 4) 事件泵（渲染已由上面的 8ms 渲染泵负责）
        let mut ended = false;
        {
            let mut p = ctx.player.borrow_mut();
            if let Some(player) = p.as_mut() {
                ended = player.pump_events();
                if let Some(a) = player.aspect() {
                    *ctx.aspect.borrow_mut() = a;
                }
            }
        }

        // 首帧到达后打一行诊断（用来区分「抠像没生效」和「mpv 把透明混成黑底」）
        if ctx.frames.get() > 0 && !dumped.get() {
            dumped.set(true);
            if let Some(p) = ctx.player.borrow().as_ref() {
                p.debug_dump();
            }
        }

        // 视频真的出帧了就不需要静态图（正常情况下它一直是隐藏的）
        if ctx.frames.get() > 0 && ctx.fallback.is_visible() && !ctx.failed.get() {
            ctx.fallback.set_visible(false);
        }

        // 5) 走路结束 → 切躺下
        if started.get() && !switched.get() && (ended || t >= WALK_FALLBACK_MS) {
            switched.set(true);
            let mut p = ctx.player.borrow_mut();
            if let Some(player) = p.as_mut() {
                match player.load(&idle_video) {
                    Ok(()) => {
                        player.set_loop(true);
                        player.play();
                    }
                    Err(e) => {
                        eprintln!("[purr-pause-linux] 休息素材加载失败：{e}");
                        if ctx.hint.text().is_empty() {
                            ctx.hint.set_text(&format!("休息素材加载失败：{e}"));
                        }
                        ctx.hint.set_visible(true);
                    }
                }
            }
        }

        // 6) 视频不可用 → 隐藏 GL 层，露出静态猫图
        //
        //    两个触发源：
        //      a) render 回调里 mpv 初始化失败（立刻，会带上具体原因）；
        //      b) 3 秒内一帧都没出（兜底，覆盖「GL 上下文根本没建起来」）。
        if !ctx.failed.get() && t >= VIDEO_DEADLINE_MS && ctx.frames.get() == 0 {
            ctx.failed.set(true);
        }
        if ctx.failed.get() && !ctx.fallback.is_visible() {
            ctx.gl_area.set_visible(false);
            ctx.fallback.set_visible(true);
            *ctx.aspect.borrow_mut() = 1.0; // 静态猫图是方图
            if ctx.hint.text().is_empty() {
                ctx.hint.set_text("视频渲染未启动（已降级为静态猫咪图）");
            }
            ctx.hint.set_visible(true);
        }

        // 7) 控制条：最迟 1.2 秒必定出现，用户随时能关
        if !bar_shown.get() && (switched.get() || t >= BAR_DEADLINE_MS) {
            bar_shown.set(true);
            ctx.bar.set_visible(true);
            countdown_start.set(Some(Instant::now()));
        }

        // 8) 倒计时
        if let Some(cs) = countdown_start.get() {
            let elapsed = cs.elapsed().as_secs();
            if elapsed >= total_secs {
                ctx.countdown.set_text("休息结束！");
                let e = emit.clone();
                glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                    e(OverlayEvent::Dismissed);
                });
                return glib::ControlFlow::Break;
            }
            let remain = total_secs - elapsed;
            ctx.countdown
                .set_text(&format!("休息中 {:02}:{:02}", remain / 60, remain % 60));
        }

        glib::ControlFlow::Continue
    });
}

fn init_state(ctx: &Ctx, mode: &str, win_w: f64, win_h: f64) {
    let end = constrained_scale(*ctx.aspect.borrow(), win_w, win_h);
    let mut s = ctx.state.borrow_mut();
    match mode {
        "walk-center" | "fade-center" => {
            s.x = win_w / 2.0;
            s.y = win_h / 2.0;
            s.scale = 0.05;
            s.opacity = 0.0;
        }
        _ => {
            s.x = win_w + 220.0;
            s.y = win_h / 2.0;
            s.opacity = 1.0;
            s.scale = if mode == "walk-zoom" { 0.4 } else { end };
        }
    }
}

/// 按时间推进动画。`t_ms` 是覆盖层出现以来的毫秒数。
fn animate(ctx: &Ctx, mode: &str, t_ms: f64) {
    let p = (t_ms / WALK_MS).min(1.0);
    let eased = ease_out_cubic(p);
    let win_w = ctx.win_w.get();
    let win_h = ctx.win_h.get();
    if win_w <= 0.0 || win_h <= 0.0 {
        return;
    }
    let end = constrained_scale(*ctx.aspect.borrow(), win_w, win_h);
    let mut s = ctx.state.borrow_mut();
    match mode {
        "walk-center" | "fade-center" => {
            s.x = win_w / 2.0;
            s.y = win_h / 2.0;
            s.scale = 0.05 + (end - 0.05) * eased;
            s.opacity = (eased * 2.0).min(1.0);
        }
        _ => {
            let sx = win_w + 220.0;
            let tx = win_w / 2.0;
            s.x = sx + (tx - sx) * eased;
            s.y = win_h / 2.0;
            s.opacity = 1.0;
            if mode == "walk-zoom" {
                s.scale = 0.4 + (end - 0.4) * eased;
            } else {
                s.scale = end;
            }
        }
    }
}

fn constrained_scale(aspect: f64, win_w: f64, win_h: f64) -> f64 {
    let base_w = BASE_CAT_HEIGHT * aspect;
    (win_h * MAX_SCREEN_HEIGHT_RATIO / BASE_CAT_HEIGHT)
        .min(win_w * MAX_SCREEN_WIDTH_RATIO / base_w)
        .min(MAX_CAT_SCALE)
        .max(0.05)
}

/// 把 CatState 应用到画面层上：静态猫图与 GL 层共用同一块矩形。
///
/// 注意：必须显式 `set_size_request` —— GLArea 没有固有尺寸，
/// 不设置的话宽高恒为 0，视频将完全不可见（scale 也就白算了）。
fn apply_geometry(ctx: &Ctx) {
    let (x, y, scale, opacity) = {
        let s = *ctx.state.borrow();
        (s.x, s.y, s.scale, s.opacity)
    };
    let aspect = *ctx.aspect.borrow();
    let w = (BASE_CAT_HEIGHT * aspect * scale).round().max(1.0) as i32;
    let h = (BASE_CAT_HEIGHT * scale).round().max(1.0) as i32;

    // Fixed.put 的坐标是子控件左上角，需减去自身尺寸的一半实现居中
    let cx = (x - w as f64 / 2.0).max(0.0);
    let cy = (y - h as f64 / 2.0).max(0.0);

    // 动画结束后几何不再变化，跳过重复布局（set_size_request 会触发整窗 relayout）
    let prev = ctx.last_geo.get();
    if (prev.0 - cx).abs() < 0.5
        && (prev.1 - cy).abs() < 0.5
        && prev.2 == w
        && prev.3 == h
        && (prev.4 - opacity).abs() < 0.001
    {
        return;
    }
    ctx.last_geo.set((cx, cy, w, h, opacity));

    ctx.fallback.set_size_request(w, h);
    ctx.fallback.set_opacity(opacity);
    // ⚠️ 用 move_（gtk_fixed_move），**不能用 put**：
    //    gtk_fixed_put 断言子控件必须没有 parent（`_gtk_widget_get_parent (widget) == NULL`），
    //    对已添加的子控件再调 put 会失败（每帧一条 Gtk-CRITICAL）且位置根本不动。
    //    v1.0.1 实测：猫被钉在左上角、尺寸也不更新，就是因为这里用了 put。
    ctx.frame.move_(&ctx.fallback, cx, cy);

    if ctx.gl_area.is_visible() {
        ctx.gl_area.set_size_request(w, h);
        ctx.gl_area.set_opacity(opacity);
        ctx.frame.move_(&ctx.gl_area, cx, cy);
    }
}

/// 静态兜底视觉：优先用品牌猫图（logo.png），取不到时退化成大号猫字符。
fn fallback_widget() -> gtk::Widget {
    if let Some(p) = fallback_image() {
        let pic = gtk::Picture::for_filename(&p);
        // ⚠️ 不要调 set_content_fit：它和 gtk::ContentFit 都要 GTK 4.8，
        //    而本应用最低支持 4.6（Ubuntu 22.04）。
        //    GtkPicture 的默认 content-fit 就是 contain，正是我们要的等比缩放。
        pic.set_can_shrink(true);
        return pic.upcast();
    }
    let label = gtk::Label::new(Some("🐈"));
    label.add_css_class("cat-fallback-emoji");
    label.upcast()
}

/// 找一张可用的猫图。安装后走 hicolor 里 512 尺寸的图标（由 build-deb.sh 生成），
/// 开发时走仓库内的 data/icons/logo.png。
fn fallback_image() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(debug_assertions) {
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("data/icons/logo.png"));
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/icons/logo.png"));
    }
    candidates.extend([
        PathBuf::from("/usr/share/icons/hicolor/512x512/apps/purr-pause-linux.png"),
        PathBuf::from("/usr/share/icons/hicolor/256x256/apps/purr-pause-linux.png"),
        PathBuf::from("/usr/share/pixmaps/purr-pause-linux.png"),
        PathBuf::from("/usr/share/purr-pause-linux/logo.png"),
    ]);
    candidates.into_iter().find(|p| p.exists())
}

/// 覆盖层专用 CSS。全局只加载一次。
///
/// `window.cat-overlay` 背景透明是**必要**的：mpv 渲染的是带 alpha 的 webm，
/// 窗口不透明的话四周会是一块死黑（v1.0.0 的「黑屏」）。
/// 但正因为透明，才必须保证窗口里**永远有可见内容**（静态猫图 + 控制条），
/// 否则就是「看不见又点不动」的鬼影窗口。
fn install_css() {
    static LOADED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if LOADED.get().is_some() {
        return;
    }
    let css = gtk::CssProvider::new();
    css.load_from_data(
        r#"
        /* 窗口本体：三重保险 —— 颜色、背景图、阴影全部清掉。
           漏掉任何一条都可能露出一块不透明底色（实测是黑底）。 */
        window.cat-overlay,
        window.cat-overlay.background {
            background-color: transparent;
            background-image: none;
            box-shadow: none;
            border: none;
        }
        .cat-bar {
            background-color: rgba(0, 0, 0, 0.66);
            border-radius: 20px;
            padding: 16px 26px;
        }
        .cat-countdown {
            color: #ffffff;
            font-size: 19px;
            font-weight: 600;
        }
        .cat-hint {
            color: rgba(255, 255, 255, 0.82);
            font-size: 12px;
        }
        button.cat-pill {
            border-radius: 999px;
            padding: 8px 22px;
            font-weight: 600;
        }
        label.cat-fallback-emoji {
            font-size: 160px;
        }
        "#,
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // ⚠️ 只有真的挂上去了才置位。否则「第一次拿不到 Display」会把
        //    LOADED 永久置位，之后每次覆盖层都是不透明白底（看起来像黑屏）。
        let _ = LOADED.set(());
    } else {
        eprintln!("[purr-pause-linux] 拿不到 GdkDisplay，覆盖层样式（透明背景）未加载");
    }
}

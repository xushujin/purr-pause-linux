//! 猫咪覆盖层：透明置顶窗口 + 四种动画模式 + 休息倒计时。
//!
//! 四种动画模式：
//!   walk-center  中央原地放大（默认）
//!   walk-zoom    从右侧走进中央，同时放大
//!   walk-flat    恒定大小从右侧走进中央
//!   fade-center  从极小缩放淡入中央
//!
//! 缓动统一为 easeOutCubic（`1 - (1 - p)^3`），与原版一致。
//!
//! ## 渲染方式
//!
//! 视频经 libmpv 解码后由 `GtkGLArea` 直接渲染到 GL 帧缓冲，
//! 猫咪的位置/缩放/透明度通过 `gl_area.set_margin()` 平移整个 GL 区域实现，
//! 透明度通过 mpv 的 `alpha-mode` + GTK 的 CSS opacity 组合控制。
//! 详见 `video.rs` 关于「为何不能用 Cairo」的说明。

use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::video::VideoPlayer;

const BASE_CAT_HEIGHT: f64 = 200.0;
const MAX_CAT_SCALE: f64 = 5.0;
const MAX_SCREEN_HEIGHT_RATIO: f64 = 0.85;
const MAX_SCREEN_WIDTH_RATIO: f64 = 0.9;

/// 走路动画兜底时长（毫秒），与原版 5 秒超时 + 缓冲余量一致。
const WALK_FALLBACK_MS: u32 = 6000;
/// 走路动画时长（毫秒）。
const WALK_DURATION_MS: f64 = 3000.0;

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
            opacity: 1.0,
        }
    }
}

/// 绘制所需的共享上下文。
struct Ctx {
    state: Rc<RefCell<CatState>>,
    aspect: Rc<RefCell<f64>>,
    /// GL 区域。视频直接渲染到它的帧缓冲。
    gl_area: gtk::GLArea,
    /// mpv 播放器只能在 GL 上下文建立后创建，故用Option 延迟初始化。
    player: Rc<RefCell<Option<VideoPlayer>>>,
    /// 猫咪显示区域（承载 GLArea），用于按 scale 调整尺寸。
    frame: gtk::Fixed,
}

/// 覆盖层窗口。
pub struct CatOverlay {
    pub window: gtk::Window,
    ctx: Rc<Ctx>,
    events: Rc<RefCell<Vec<OverlayEvent>>>,
}

impl CatOverlay {
    pub fn new(
        animation_mode: String,
        break_minutes: i64,
        snooze_count: i64,
        walk_video: PathBuf,
        idle_video: PathBuf,
    ) -> Self {
        let window = gtk::Window::builder()
            .decorated(false)
            .resizable(false)
            .deletable(false)
            .build();
        window.set_keep_above(true);
        window.set_focusable(false);
        window.fullscreen();

        // GL 区域：承载视频渲染
        let gl_area = gtk::GLArea::new();
        gl_area.set_auto_render(false);
        gl_area.set_has_depth_buffer(false);
        gl_area.set_has_stencil_buffer(false);

        let ctx = Rc::new(Ctx {
            state: Rc::new(RefCell::new(CatState::default())),
            aspect: Rc::new(RefCell::new(16.0 / 9.0)),
            gl_area: gl_area.clone(),
            player: Rc::new(RefCell::new(None)),
            frame: gtk::Fixed::new(),
        });

        // 首次渲染时 GL 上下文已就绪，此时才能创建 mpv 播放器。
        // render 信号签名：Fn(&GLArea, &gdk::GLContext) -> glib::Propagation
        {
            let c = ctx.clone();
            gl_area.connect_render(move |area, _gl_ctx| {
                if c.player.borrow().is_none() {
                    match VideoPlayer::new() {
                        Ok(p) => *c.player.borrow_mut() = Some(p),
                        Err(e) => {
                            eprintln!("[purr-pause-linux] 视频初始化失败：{e}");
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

        //把 GL 区域放进 Fixed 容器，便于按动画调整位置与尺寸
        ctx.frame.put(&gl_area, 0.0, 0.0);
        ctx.frame.set_halign(gtk::Align::Fill);
        ctx.frame.set_valign(gtk::Align::Fill);

        let overlay_box = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        overlay_box.set_halign(gtk::Align::Center);
        overlay_box.set_margin_bottom(40);
        overlay_box.set_margin_top(16);
        overlay_box.set_visible(false);

        let countdown = gtk::Label::new(Some(""));
        countdown.add_css_class("cat-countdown");

        let snooze_btn = gtk::Button::with_label("再等 5 分钟");
        snooze_btn.add_css_class("cat-pill");
        snooze_btn.add_css_class("suggested-action");

        let dismiss_btn = gtk::Button::with_label("精力充沛！");
        dismiss_btn.add_css_class("cat-pill");
        dismiss_btn.add_css_class("destructive-action");

        if snooze_count >= 2 {
            snooze_btn.set_sensitive(false);
            snooze_btn.set_label("已用完");
        } else {
            snooze_btn.set_label(&format!("再等 5 分钟（{}次）", 2 - snooze_count.max(0)));
        }

        overlay_box.append(&countdown);
        overlay_box.append(&snooze_btn);
        overlay_box.append(&dismiss_btn);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&ctx.frame);
        root.append(&overlay_box);
        window.set_child(Some(&root));

        let events: Rc<RefCell<Vec<OverlayEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let ev = events.clone();
        dismiss_btn.connect_clicked(move |_| ev.borrow_mut().push(OverlayEvent::Dismissed));
        let ev2 = events.clone();
        snooze_btn.connect_clicked(move |_| ev2.borrow_mut().push(OverlayEvent::Snoozed));

        let overlay = Self {
            window,
            ctx,
            events,
        };

        overlay.begin(
            &animation_mode,
            &walk_video,
            &idle_video,
            break_minutes,
            &countdown,
            &overlay_box,
        );
        overlay
    }

    fn begin(
        &self,
        mode: &str,
        walk_video: &Path,
        idle_video: &Path,
        break_minutes: i64,
        countdown: &gtk::Label,
        overlay_box: &gtk::Box,
    ) {
        let walk_video = walk_video.to_path_buf();
        let idle_video = idle_video.to_path_buf();

        // GL 上下文尚未建立时不能创建 mpv，也无法真正加载素材。
        // 因此把「载入 + 起播」放进一个轮询，等 player 就绪后再执行一次。
        {
            let ctx = self.ctx.clone();
            let mode = mode.to_string();
            let idle = idle_video.clone();
            let countdown = countdown.clone();
            let overlay_box = overlay_box.clone();
            let events = self.events.clone();
            let bm = break_minutes.max(1) as u64;
            let countdown_finished = Rc::new(RefCell::new(false));
            let elapsed = Rc::new(RefCell::new(0u32));
            let switched = Rc::new(RefCell::new(false));
            let started = Rc::new(RefCell::new(false));

            let ctx_boot = ctx.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || {
                // 等 GL 上下文就绪
                if ctx_boot.player.borrow().is_none() {
                    return glib::ControlFlow::Continue;
                }
                if !*started.borrow() {
                    *started.borrow_mut() = true;
                    // 载入走路素材并起播
                    let mut p = ctx_boot.player.borrow_mut();
                    if let Some(player) = p.as_mut() {
                        match player.load(&walk_video) {
                            Ok(()) => {
                                player.set_loop(false);
                                player.play();
                                if let Some(a) = player.aspect() {
                                    *ctx_boot.aspect.borrow_mut() = a;
                                }
                            }
                            Err(e) => {
                                eprintln!("[purr-pause-linux] 走路素材加载失败：{e}");
                                return glib::ControlFlow::Break;
                            }
                        }
                    }
                    drop(p);

                    // 等窗口拿到真实尺寸后启动位置动画
                    let c2 = ctx_boot.clone();
                    let m2 = mode.clone();
                    glib::timeout_add_local_once(Duration::from_millis(30), move || {
                        let (w, h) = c2.frame.size();
                        if w <= 1 || h <= 1 {
                            return;
                        }
                        init_state(&c2, &m2, w as f64, h as f64);
                        spawn_walk_loop(c2, m2, w as f64, h as f64);
                    });
                }

                // 帧驱动：GL 区域需持续重绘才能推进视频
                ctx_boot.gl_area.queue_render();

                // 走路结束 → 切躺下+ 显示遮罩 + 启动倒计时
                *elapsed.borrow_mut() += 1;
                let ended = ctx_boot
                    .player
                    .borrow_mut()
                    .as_mut()
                    .map(|p| p.pump_events())
                    .unwrap_or(false);
                let timeout = *elapsed.borrow() > (WALK_FALLBACK_MS / 50);
                if (ended || timeout) && !*switched.borrow() {
                    *switched.borrow_mut() = true;
                    {
                        let mut p = ctx_boot.player.borrow_mut();
                        if let Some(player) = p.as_mut() {
                            if player.load(&idle).is_ok() {
                                player.set_loop(true);
                                player.play();
                                if let Some(a) = player.aspect() {
                                    *ctx_boot.aspect.borrow_mut() = a;
                                }
                            }
                        }
                    }
                    overlay_box.set_visible(true);

                    let end = Instant::now() + Duration::from_secs(bm * 60);
                    let cd = countdown.clone();
                    let ev2 = events.clone();
                    let fin = countdown_finished.clone();
                    glib::timeout_add_local(Duration::from_millis(500), move || {
                        let remain =
                            end.saturating_duration_since(Instant::now()).as_secs() as i64;
                        if remain <= 0 {
                            cd.set_text("休息结束！");
                            if !*fin.borrow() {
                                *fin.borrow_mut() = true;
                                let ev3 = ev2.clone();
                                glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                                    ev3.borrow_mut().push(OverlayEvent::Dismissed);
                                });
                            }
                            return glib::ControlFlow::Break;
                        }
                        cd.set_text(&format!("休息中 {:02}:{:02}", remain / 60, remain % 60));
                        glib::ControlFlow::Continue
                    });
                    // 走路动画已结束，停止位置动画的重绘由独立循环负责；
                    // 但视频仍需持续刷新，故保留本循环直到覆盖层关闭。
                }
                glib::ControlFlow::Continue
            });
        }
    }

    pub fn take_events(&self) -> Vec<OverlayEvent> {
        let mut ev = self.events.borrow_mut();
        std::mem::take(&mut *ev)
    }

    pub fn close(&self) {
        self.window.close();
    }
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

fn constrained_scale(aspect: f64, win_w: f64, win_h: f64) -> f64 {
    let base_w = BASE_CAT_HEIGHT * aspect;
    (win_h * MAX_SCREEN_HEIGHT_RATIO / BASE_CAT_HEIGHT)
        .min(win_w * MAX_SCREEN_WIDTH_RATIO / base_w)
        .min(MAX_CAT_SCALE)
}

/// 走路动画循环。mode 由构造时传入，不使用全局状态。
fn spawn_walk_loop(ctx: Rc<Ctx>, mode: String, win_w: f64, win_h: f64) {
    let start = Instant::now();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let elapsed = start.elapsed().as_millis() as f64;
        let p = (elapsed / WALK_DURATION_MS).min(1.0);
        let eased = ease_out_cubic(p);
        let end = constrained_scale(*ctx.aspect.borrow(), win_w, win_h);

        {
            let mut s = ctx.state.borrow_mut();
            match mode.as_str() {
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

        apply_geometry(&ctx, win_w, win_h);
        ctx.gl_area.queue_render();

        if p < 1.0 {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
}

/// 把 CatState 里的几何状态应用到 GL 区域上。
///
/// GL 区域本身尺寸固定为素材原始尺寸的缩放结果，通过在 Fixed 容器内
/// 平移实现「走动」，通过容器整体 opacity 实现淡入淡出。
fn apply_geometry(ctx: &Ctx, _win_w: f64, _win_h: f64) {
    let (x, y, scale, opacity) = {
        let s = *ctx.state.borrow();
        (s.x, s.y, s.scale, s.opacity)
    };
    if opacity <= 0.001 {
        ctx.gl_area.set_opacity(0.0);
        return;
    }
    ctx.gl_area.set_opacity(opacity);
    // Fixed.put 的坐标是子控件左上角，需减去自身尺寸的一半实现居中
    let (w, h) = ctx.gl_area.size();
    let cx = (x - w as f64 / 2.0).max(0.0);
    let cy = (y - h as f64 / 2.0).max(0.0);
    ctx.frame.put(&ctx.gl_area, cx, cy);
}
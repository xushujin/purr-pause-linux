//! libmpv 封装：把webm（VP9 + Alpha）解码后渲染到 GTK 的 GL 区域。
//!
//! ## 为什么必须用 GtkGLArea（重要，不要改回Cairo）
//!
//! 早期设想是走 mpv 的 Cairo render API（MPV_RENDER_API_TYPE_CAIRO），
//! 直接画进 DrawingArea 的 Cairo 上下文以绕开 OpenGL。
//! **该设想不可行**：项目依赖的 `mpv` crate 0.2.3 只提供
//! `MpvHandler::build_with_gl()`（内部设`vo=opengl-cb`）这一条渲染路径，
//! 既没有 `MpvRenderApiType::Cairo`，也不导出 `mpv_opengl_cb_*` 底层符号。
//! 因此改用 GTK4 的 `GtkGLArea`：由 GTK 提供 GL 上下文，
//! 通过 `get_proc_address` 交给 mpv，mpv 直接渲染到当前 FBO。
//!
//! 代价：Wayland 下需要可用的 GL/EGL。GL 上下文建立失败时
//! `VideoPlayer::new` 返回 Err，调用方负责降级（见 overlay.rs）。
//!
//! mpv 原生支持 VP9，且默认保留 alpha 通道
//! （`alpha-mode` 须保持 default/auto，不可设为 disabled）。

use std::path::Path;

use gtk::gdk;
use gtk::prelude::*;
use mpv::{MpvHandlerBuilder, MpvHandlerWithGl};

// `mpv` crate 0.2.3 是纯 FFI 绑定，**没有 build.rs**，
// 因此不会自动链接 libmpv。必须在本项目里显式声明，
// 否则链接阶段会报 undefined reference to `mpv_create' 等错误。
#[link(name = "mpv")]
unsafe extern "C" {}

/// GL 函数地址解析回调。
///
/// mpv 通过它按需加载 GL 扩展函数。GTK4 已提供 `gdk::gl::get_proc_address`，
/// 直接转发即可；ctx 参数不用。
unsafe extern "C" fn get_proc_address(
    _ctx: *mut std::os::raw::c_void,
    name: *const std::os::raw::c_char,
) -> *mut std::os::raw::c_void {
    gdk::gl::get_proc_address(name as *const u8)
}

/// 视频播放器。持有 mpv 的 GL 渲染句柄。
pub struct VideoPlayer {
    mpv: Box<MpvHandlerWithGl>,
    loaded: String,
}

impl VideoPlayer {
    /// 创建播放器。
    ///
    /// 必须在 GTK 的 GL 上下文**已经建立之后**调用（即 GLArea 的
    /// `render` 信号回调内），否则 mpv 无法初始化渲染。
    pub fn new() -> Result<Self, String> {
        let builder =
            MpvHandlerBuilder::new().map_err(|e| format!("mpv 创建失败：{e:?}"))?;

        // 硬件解码不可用时自动回落到软解；禁用音频（猫咪动画无声）
        let opts: Vec<(&str, &str)> = vec![
            ("hwdec", "auto-safe"),
            ("no-audio", "yes"),
            ("really-quiet", "yes"),
            // 保留 alpha 通道（透明背景 webm 必需）
            ("alpha-mode", "default"),
            ("scale", "bilinear"),
            ("keep-open", "yes"),
            ("keep-open-pause", "yes"),
            ("idle", "yes"),
            ("loop-file", "no"),
        ];
        for (k, v) in opts {
            builder
                .set_option(k, v)
                .map_err(|e| format!("设置 mpv 选项 {k} 失败：{e:?}"))?;
        }

        // build_with_gl 内部会 set_option("vo", "opengl-cb") 并绑定 GL 回调
        let mpv = builder
            .build_with_gl(get_proc_address, std::ptr::null_mut())
            .map_err(|e| format!("mpv GL 初始化失败：{e:?}"))?;

        Ok(Self {
            mpv,
            loaded: String::new(),
        })
    }

    /// 载入视频。重复载同一文件时从头播放。
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("素材不存在：{}", path.display()));
        }
        let p = path.to_string_lossy().to_string();
        if self.loaded == p {
            self.seek(0.0);
            self.play();
            return Ok(());
        }
        let url = format!("file://{p}");
        self.mpv
            .command(&["loadfile", &url, "replace"])
            .map_err(|e| format!("loadfile 失败：{e:?}"))?;
        self.loaded = p;
        Ok(())
    }

    pub fn play(&mut self) {
        let _ = self.mpv.set_property("pause", false);
    }

    pub fn pause(&mut self) {
        let _ = self.mpv.set_property("pause", true);
    }

    pub fn set_loop(&mut self, on: bool) {
        let _ = self.mpv.set_property("loop-file", on);
    }

    pub fn seek(&mut self, pos: f64) {
        let _ = self.mpv.command(&["seek", &format!("{pos}"), "absolute"]);
    }

    /// 视频宽高比（宽 / 高）。未就绪时返回 None。
    pub fn aspect(&self) -> Option<f64> {
        let w = self
            .mpv
            .get_property::<i64>("width")
            .ok()
            .filter(|v| *v > 0)? as f64;
        let h = self
            .mpv
            .get_property::<i64>("height")
            .ok()
            .filter(|v| *v > 0)? as f64;
        Some(w / h)
    }

    /// 事件泵：处理 mpv 事件。需在主循环中定期调用，否则事件队列会堆积。
    ///
    /// 返回 true 表示当前文件已播放完毕（EOF）或 mpv 已关闭。
    pub fn pump_events(&mut self) -> bool {
        let mut ended = false;
        while let Some(event) = self.mpv.wait_event(0.0) {
            match event {
                mpv::Event::EndFile(_) | mpv::Event::Shutdown => ended = true,
                _ => {}
            }
        }
        ended
    }

    /// 是否有新帧可渲染。
    pub fn has_frame(&self) -> bool {
        self.mpv.get_property::<i64>("width").unwrap_or(0) > 0
    }

    /// 把当前帧渲染到 GL 上下文的当前帧缓冲。
    ///
    /// 只能在 GTK `render` 信号回调内调用（须持有 GL 上下文）。
    pub fn draw(&mut self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        // fbo = 0 表示渲染到当前帧缓冲
        let _ = self.mpv.draw(0, w, h);
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let _ = self.mpv.command(&["quit"]);
    }
}
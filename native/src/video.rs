//! libmpv 封装：把 webm（VP9 + Alpha）解码后渲染到 GTK 的 GL 区域。
//!
//! ## 为什么必须用 GtkGLArea（重要，不要改回 Cairo）
//!
//! 早期设想是走 mpv 的 Cairo render API（MPV_RENDER_API_TYPE_CAIRO），
//! 直接画进 DrawingArea 的 Cairo 上下文以绕开 OpenGL。
//! **该设想不可行**：项目依赖的 `mpv` crate 0.2.3 只提供
//! `MpvHandlerBuilder::build_with_gl()`（内部设 `vo=opengl-cb`）这一条渲染路径，
//! 既没有 `MpvRenderApiType::Cairo`，也不导出 `mpv_opengl_cb_*` 底层符号。
//! 因此改用 GTK4 的 `GtkGLArea`：由 GTK 提供 GL 上下文，
//! mpv 直接渲染到当前 FBO。
//!
//! ## GL 函数解析为什么用 libloading（不要改成 gdk）
//!
//! mpv 需要一个 `get_proc_address` 回调来按需加载 GL 函数。
//! **GTK4 移除了 `gdk_gl_context_get_proc_address`**，`gdk4` crate 里
//! 搜不到任何 `get_proc_address`；`gtk::gdk::gl` re-export 的是外部
//! `gl` crate 0.14，它只提供 `load_with`，同样没有这个函数。
//! 因此这里直接 dlopen `libGL.so.1` 并 dlsym。
//!
//! 代价：Wayland 下需要系统装有 `libGL.so.1`（mesa 的一部分，几乎总在）。
//! 加载失败时 `VideoPlayer::new` 返回 Err，调用方负责降级（见 overlay.rs）。

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::Path;
use std::ptr;
use std::sync::OnceLock;

use mpv::{MpvHandlerBuilder, MpvHandlerWithGl};

// `mpv` crate 0.2.3 是纯 FFI 绑定，**没有 build.rs**，
// 因此不会自动链接 libmpv。必须在本项目里显式声明，
// 否则链接阶段会报 undefined reference to `mpv_create' 等错误。
#[link(name = "mpv")]
unsafe extern "C" {}

/// 全局持有 dlopen 的 libGL 句柄。
///
/// mpv 会在整个生命周期内反复回调 `get_proc_address`，
/// 因此句柄必须活得比 mpv 久 —— 用 `OnceLock` + `Box::leak` 静态化。
static GL_LIB: OnceLock<usize> = OnceLock::new();

/// 加载 libGL 并返回其裸指针（供 mpv 回调里解引用）。
fn gl_library_ptr() -> Result<usize, String> {
    if let Some(p) = GL_LIB.get() {
        return Ok(*p);
    }
    unsafe {
        let lib = libloading::Library::new("libGL.so.1")
            .map_err(|e| format!("加载 libGL.so.1 失败：{e}"))?;
        // 泄漏以保证生命周期长于所有 mpv 实例
        let leaked: &'static libloading::Library = Box::leak(Box::new(lib));
        let ptr = leaked as *const libloading::Library as usize;
        let _ = GL_LIB.set(ptr);
        Ok(ptr)
    }
}

/// GL 函数地址解析回调（签名必须严格匹配 `mpv_opengl_cb_get_proc_address_fn`）。
///
/// 策略：先走 GL 规范提供的 `glGetProcAddress`（能拿到扩展函数），
/// 失败再回退 dlsym（覆盖 libGL 直接导出的核心符号）。
unsafe extern "C" fn get_proc_address(
    ctx: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if ctx.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let lib = &*(ctx as *const libloading::Library);

    // 1) glGetProcAddress
    let sym_name = match CStr::from_ptr(name).to_bytes_with_nul() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    unsafe {
        let get_proc: Result<
            libloading::Symbol<'_, unsafe extern "C" fn(*const c_char) -> *mut c_void>,
            _,
        > = lib.get(b"glGetProcAddress\0");
        if let Ok(f) = get_proc {
            let p = f(name);
            if !p.is_null() {
                return p;
            }
        }
        // 2) dlsym 兜底
        // 注意 get::<T>() 已返回 Symbol<'_, T>，
        // 不能写成 get::<Symbol<'_, *mut c_void>>（会双重包装导致两臂类型不一致）
        match lib.get::<*mut c_void>(sym_name) {
            Ok(s) => *s,
            Err(_) => ptr::null_mut(),
        }
    }
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
        let gl_ptr = gl_library_ptr()?;

        let mut builder =
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

        // build_with_gl 内部会 set_option("vo", "opengl-cb") 并绑定 GL 回调。
        // 注意第一个参数是 Option<fn>，不是裸 fn。
        let mpv = builder
            .build_with_gl(Some(get_proc_address), gl_ptr as *mut c_void)
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
    /// 只能在 GTK `render` 信号回调内调用（须已 make_current）。
    ///
    /// **height 必须传负值** —— 这是 mpv-rs 的约定（见其 sdl2 示例），
    /// 表示垂直翻转；传正值会导致画面上下颠倒。
    pub fn draw(&mut self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        // fbo = 0 表示渲染到当前帧缓冲
        let _ = self.mpv.draw(0, w, -h);
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let _ = self.mpv.command(&["quit"]);
    }
}

/// 占位，保持 RefCell 在本文件的引用（overlay 用 Rc<RefCell<Option<VideoPlayer>>>）。
#[allow(dead_code)]
type PlayerCell = RefCell<Option<VideoPlayer>>;
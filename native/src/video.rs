//! libmpv 封装：把 webm（VP9 + Alpha）解码后渲染到 GTK 的 GL 区域。
//!
//! ## 为什么不用 `mpv` crate 的 `build_with_gl()`
//!
//! `mpv` crate 0.2.3 的 GL 路径基于 **opengl-cb** 子系统
//! （`mpv_opengl_cb_init_gl` / `mpv_opengl_cb_draw` / `mpv_get_sub_api` 等）。
//! 该子系统在 **mpv 0.35 已被移除**，Ubuntu 24.04 提供的是 mpv 0.37，
//! 其 `libmpv.so.2` 里这些符号**全部不存在**（可用 `nm -D` 核实），
//! 因此 `build_with_gl()` 会直接**链接失败**：
//! `undefined reference to 'mpv_opengl_cb_init_gl'`。
//!
//! 现在改用官方**仍在维护**的 render API（`mpv_render_context_*`，
//! 见 `/usr/include/mpv/render.h` 与 `render_gl.h`），
//! 这部分符号在 libmpv 0.37 中存在：
//!   mpv_render_context_create / _render / _free / _set_update_callback
//!
//! 控制面（command / set_property / get_property / wait_event）仍用 `mpv` crate，
//! 那些符号（`mpv_create` / `mpv_command` …）未被移除，可正常使用。
//!
//! ## GL 函数解析为什么自己 dlopen
//!
//! GTK4 移除了 `gdk_gl_context_get_proc_address`，`gdk4` crate 里也没有该函数
//! （`gtk::gdk::gl` re-export 的是外部 `gl` crate 0.14，只提供 `load_with`）。
//! 因此这里自行加载动态库并按平台解析：
//!   - X11/GLX：`glXGetProcAddressARB`（libGL.so.1）
//!   - Wayland/EGL：`eglGetProcAddress`（libEGL.so.1）
//!   - 兜底：直接 dlsym libGL / libGLESv2 导出的核心入口
//! 三个库句柄一并保存在 `GlLibraries` 里，作为 `get_proc_address` 的 ctx。

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use gtk::glib;
use gtk::prelude::*;
use mpv::{MpvHandler, MpvHandlerBuilder};

// ---------------------------------------------------------------------------
// libmpv render API 的 FFI 绑定（对应 /usr/include/mpv/render.h + render_gl.h）
// ---------------------------------------------------------------------------

/// `mpv_render_param_type`（render.h 中的枚举值，已核对头文件）
const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;

/// `#define MPV_RENDER_API_TYPE_OPENGL "opengl"`（含结尾 NUL）
const MPV_RENDER_API_TYPE_OPENGL: &[u8] = b"opengl\0";

/// `typedef struct mpv_render_param { enum mpv_render_param_type type; void *data; }`
#[repr(C)]
struct MpvRenderParam {
    type_: c_int,
    data: *mut c_void,
}

/// `mpv_opengl_init_params`（render_gl.h）
#[repr(C)]
struct MpvOpenglInitParams {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

/// `mpv_opengl_fbo`（render_gl.h）：fbo, w, h, internal_format
#[repr(C)]
struct MpvOpenglFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

// `mpv` crate 0.2.3 是纯 FFI 绑定，**没有 build.rs**，不会自动链接 libmpv；
// 这里显式声明链接。同时下面这些 render API 函数由本模块自行声明
// （crate 里没有封装 —— 它只有已被移除的 opengl-cb）。
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_render_context_create(
        res: *mut *mut c_void,
        mpv: *mut c_void,
        params: *mut MpvRenderParam,
    ) -> c_int;
    fn mpv_render_context_render(ctx: *mut c_void, params: *mut MpvRenderParam) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut c_void,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
        callback_ctx: *mut c_void,
    );
    fn mpv_render_context_free(ctx: *mut c_void);
}

// ---------------------------------------------------------------------------
// 渲染到默认帧缓冲时是否需要垂直翻转。
//
// mpv 文档（render.h，MPV_RENDER_PARAM_FLIP_Y）：
//   「If the value is set to 0, render normally. Otherwise, render it flipped,
//     which is needed e.g. when rendering to an OpenGL default framebuffer
//     (which has a flipped coordinate system).」
//
// ⚠️ 我们渲染进的是 GTK 的离屏 FBO（非默认帧缓冲），方向约定与直接显示不同，
//    因此该值需要在真机上目视确认：若猫咪上下颠倒，把 1 改成 0 即可。
// ---------------------------------------------------------------------------
const FLIP_Y: c_int = 1;

/// `GL_FRAMEBUFFER_BINDING`
const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
type GlGetIntegerv = unsafe extern "C" fn(u32, *mut c_int);
/// `glXGetProcAddressARB` / `eglGetProcAddress` 的形状相同
type GetProcAddressFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;

/// 已加载的 GL 相关动态库。
///
/// 需要多个候选，因为 GTK 在 X11 下走 GLX（libGL），
/// 在 Wayland 下通常走 EGL（libEGL + libGLESv2）；
/// 而 `get_proc_address` 的解析方式随之不同。
struct GlLibraries {
    gl: Option<libloading::Library>,
    egl: Option<libloading::Library>,
    gles: Option<libloading::Library>,
}

/// 全局持有 —— mpv 会在整个生命周期内反复回调 `get_proc_address`，
/// 因此句柄必须活得比 mpv 久。
static GL_LIBS: OnceLock<&'static GlLibraries> = OnceLock::new();

fn gl_libs() -> Result<&'static GlLibraries, String> {
    if let Some(libs) = GL_LIBS.get() {
        return Ok(libs);
    }
    let libs = GlLibraries {
        gl: unsafe { libloading::Library::new("libGL.so.1").ok() },
        egl: unsafe { libloading::Library::new("libEGL.so.1").ok() },
        gles: unsafe { libloading::Library::new("libGLESv2.so.2").ok() },
    };
    if libs.gl.is_none() && libs.gles.is_none() {
        return Err("无法加载 libGL.so.1 或 libGLESv2.so.2".to_string());
    }
    let leaked: &'static GlLibraries = Box::leak(Box::new(libs));
    let _ = GL_LIBS.set(leaked);
    Ok(leaked)
}

/// GL 函数地址解析回调（签名须匹配 `mpv_opengl_init_params.get_proc_address`）。
///
/// 注意：**OpenGL 规范里没有 `glGetProcAddress` 这个函数**。
/// 各平台的实际入口是：
///   - X11/GLX：`glXGetProcAddressARB`（在 libGL.so.1）
///   - Wayland/EGL：`eglGetProcAddress`（在 libEGL.so.1）
/// 两者都拿不到时，回退到直接 dlsym（覆盖 libGL/libGLESv2 直接导出的核心入口）。
unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    if ctx.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let libs = unsafe { &*(ctx as *const GlLibraries) };
    let sym_name = unsafe { CStr::from_ptr(name).to_bytes_with_nul() };

    unsafe {
        // 1) GLX / X11
        if let Some(gl) = &libs.gl {
            if let Ok(f) = gl.get::<GetProcAddressFn>(b"glXGetProcAddressARB\0") {
                let p = f(name);
                if !p.is_null() {
                    return p;
                }
            }
        }
        // 2) EGL / Wayland
        if let Some(egl) = &libs.egl {
            if let Ok(f) = egl.get::<GetProcAddressFn>(b"eglGetProcAddress\0") {
                let p = f(name);
                if !p.is_null() {
                    return p;
                }
            }
        }
        // 3) 直接 dlsym 兜底
        // get::<T>() 已返回 Symbol<'_, T>，不能再包一层 Symbol
        for lib in [&libs.gl, &libs.gles].into_iter().flatten() {
            if let Ok(s) = lib.get::<*mut c_void>(sym_name) {
                if !(*s).is_null() {
                    return *s;
                }
            }
        }
        ptr::null_mut()
    }
}

/// 读取当前绑定的帧缓冲名。
///
/// GTK 的 `GtkGLArea` 会在自己的离屏 FBO 上渲染（其官方文档示例就是用
/// `glGetIntegerv(GL_FRAMEBUFFER_BINDING, &fbo)` 取当前 FBO），
/// 因此不能硬编码 FBO 0，否则画面会画到错误的缓冲上。
fn current_fbo() -> c_int {
    let Ok(libs) = gl_libs() else {
        return 0;
    };
    for lib in [&libs.gl, &libs.gles].into_iter().flatten() {
        if let Ok(f) = unsafe { lib.get::<GlGetIntegerv>(b"glGetIntegerv\0") } {
            let mut fbo: c_int = 0;
            unsafe { f(GL_FRAMEBUFFER_BINDING, &mut fbo) };
            return fbo;
        }
    }
    0
}

/// mpv 的「有新帧可渲染」回调。
///
/// ⚠️ 该回调可能由 mpv 内部线程调用，**绝不能**在这里碰 GTK 对象，
/// 只置一个原子标志，由主线程的帧驱动定时器读取。
unsafe extern "C" fn on_mpv_update(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let flag = unsafe { &*(ctx as *const AtomicBool) };
    flag.store(true, Ordering::Release);
}

/// 视频播放器。
pub struct VideoPlayer {
    /// 控制面（command / property / event）走 `mpv` crate
    mpv: MpvHandler,
    /// `mpv_render_context*`（不透明指针）
    render_ctx: *mut c_void,
    /// mpv 回调置位用。放在 Box 里以保证地址稳定（回调持有其裸指针）
    update_flag: Box<AtomicBool>,
    /// 持有 GLArea 的弱引用，仅用于日志/调试用途
    gl_area: glib::WeakRef<gtk::GLArea>,
    loaded: String,
}

impl VideoPlayer {
    /// 创建播放器。
    ///
    /// 必须在 GL 上下文**已经建立且 current** 之后调用
    /// （即 GLArea 的 `render` 回调内），否则 render context 创建会失败。
    pub fn new(gl_area: &gtk::GLArea) -> Result<Self, String> {
        // 加载 GL 相关动态库；句柄随进程存活，作为 get_proc_address 的 ctx
        let gl_ctx = gl_libs()? as *const GlLibraries as *mut c_void;

        let mut builder =
            MpvHandlerBuilder::new().map_err(|e| format!("mpv 创建失败：{e:?}"))?;

        // `vo=libmpv` 是使用 render API 的前提（由 vo_libmpv.c 提供渲染后端）。
        // 注意：个别选项名若不被当前 libmpv 支持，**不应**让整个播放器创建失败，
        // 因此下面逐个容错处理。
        let opts: Vec<(&str, &str)> = vec![
            ("vo", "libmpv"),
            // 硬件解码不可用时自动回落到软解
            ("hwdec", "auto-safe"),
            // 禁用音频（猫咪动画无声）
            ("audio", "no"),
            ("really-quiet", "yes"),
            // 保留 alpha 通道（透明背景 webm 必需）
            ("alpha", "yes"),
            ("scale", "bilinear"),
            ("keep-open", "yes"),
            ("keep-open-pause", "yes"),
            ("idle", "yes"),
            ("loop-file", "no"),
            // 不做视频计时等待，避免阻塞 GTK 主线程
            ("video-timing-offset", "0"),
        ];
        for (k, v) in opts {
            if let Err(e) = builder.set_option(k, v) {
                eprintln!("[purr-pause-linux] mpv 选项 {k} 设置失败（已忽略）：{e:?}");
            }
        }

        let mpv = builder
            .build()
            .map_err(|e| format!("mpv 初始化失败：{e:?}"))?;

        // build() 之后才有可用的 handle
        let handle = mpv.raw() as *mut c_void;

        let mut init = MpvOpenglInitParams {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: gl_ctx,
        };

        // ⚠️ MPV_RENDER_PARAM_API_TYPE 的 data 必须**直接是字符串指针本身**。
        // mpv 内部（video/out/gpu/libmpv_gpu.c）是这样用的：
        //     char *api = get_mpv_render_param(params, MPV_RENDER_PARAM_API_TYPE, NULL);
        //     strcmp(context_backends[n]->api_name, api)
        // 而 get_mpv_render_param 只是原样返回 `params[n].data`。
        // 头文件里该参数的 `Type:` 注释也写的是 `char*`（对照 FLIP_Y 是 `int*`，
        // OPENGL_FBO 是 `mpv_opengl_fbo*` —— 那些才需要取地址）。
        //
        // 若误写成 `&api_type`（指向指针的指针），mpv 会拿栈地址去做 strcmp，
        // 必然不匹配 → 返回 MPV_ERROR_NOT_IMPLEMENTED → 播放器创建失败、
        // 视频永远渲染不出来。这个错误编译期查不出来。
        let mut params = [
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut MpvOpenglInitParams as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut render_ctx: *mut c_void = ptr::null_mut();
        let rc = unsafe { mpv_render_context_create(&mut render_ctx, handle, params.as_mut_ptr()) };
        if rc < 0 || render_ctx.is_null() {
            return Err(format!(
                "mpv render context 创建失败（错误码 {rc}）—— GL 上下文是否已 current？"
            ));
        }

        let update_flag = Box::new(AtomicBool::new(false));
        unsafe {
            mpv_render_context_set_update_callback(
                render_ctx,
                Some(on_mpv_update),
                &*update_flag as *const AtomicBool as *mut c_void,
            );
        }

        Ok(Self {
            mpv,
            render_ctx,
            update_flag,
            gl_area: gl_area.downgrade(),
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

    #[allow(dead_code)]
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

    /// 是否有新帧。来自 mpv 的 update 回调（原子标志，可跨线程）。
    pub fn take_update_flag(&self) -> bool {
        self.update_flag.swap(false, Ordering::AcqRel)
    }

    /// 是否有新帧可渲染。
    #[allow(dead_code)]
    pub fn has_frame(&self) -> bool {
        self.mpv.get_property::<i64>("width").unwrap_or(0) > 0
    }

    /// 把当前帧渲染到当前绑定的帧缓冲。
    ///
    /// 只能在 GTK `render` 信号回调内调用（须已 `make_current`）。
    pub fn draw(&mut self, w: c_int, h: c_int) {
        if w <= 0 || h <= 0 || self.render_ctx.is_null() {
            return;
        }
        let mut fbo = MpvOpenglFbo {
            fbo: current_fbo(),
            w,
            h,
            internal_format: 0,
        };
        let mut flip_y: c_int = FLIP_Y;
        let mut params = [
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut fbo as *mut MpvOpenglFbo as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_FLIP_Y,
                data: &mut flip_y as *mut c_int as *mut c_void,
            },
            MpvRenderParam {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        unsafe {
            let _ = mpv_render_context_render(self.render_ctx, params.as_mut_ptr());
        }
    }

    /// 释放 render context。幂等。
    ///
    /// 应在 GL 上下文仍可用时调用（GLArea 的 `unrealize`），
    /// 以便 mpv 正常释放它创建的 GL 资源。
    pub fn shutdown(&mut self) {
        if self.render_ctx.is_null() {
            return;
        }
        if let Some(area) = self.gl_area.upgrade() {
            // 尽量让 GL 上下文 current（unrealize 阶段可能已失效，不保证成功）
            area.make_current();
        }
        unsafe {
            mpv_render_context_free(self.render_ctx);
        }
        self.render_ctx = ptr::null_mut();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // 若 unrealize 已调用过 shutdown，这里是空操作
        self.shutdown();
        let _ = self.mpv.command(&["quit"]);
    }
}
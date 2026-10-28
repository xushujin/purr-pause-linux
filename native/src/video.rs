//! libmpv 封装：把 webm（VP9 + Alpha）解码后渲染到 GTK 的 GL 区域。
//!
//! ## 为什么完全不用 `mpv` crate
//!
//! `mpv` crate 0.2.3 有两个致命问题：
//!
//! 1. 它的 GL 路径基于 **opengl-cb** 子系统（`mpv_opengl_cb_init_gl` /
//!    `mpv_get_sub_api` 等），而该子系统在 **mpv 0.35 已被移除**。
//!    在现代 libmpv 上 `build_with_gl()` 会直接链接失败。
//!    → 所以改用官方仍在维护的 render API（`mpv_render_context_*`）。
//!
//! 2. 它的 `mpv_gen.rs` 里带 `#[link(name = "mpv")] extern "C" { … }`，
//!    **只要用到这个 crate，二进制就会记录 NEEDED = libmpv.so.2**。
//!    Ubuntu 22.04 / Debian 12 提供的是 **libmpv.so.1**（mpv 0.34），
//!    于是包在这些系统上根本起不来。
//!    → 所以控制面（command / property / event）也一并自写 FFI，
//!      并改为**运行时 dlopen**：依次尝试 libmpv.so.2 → libmpv.so.1 → libmpv.so。
//!
//! 两条 render API 与 client API 的 ABI 都以 `/usr/include/mpv/{client,render}.h`
//! 为准逐字段核对过；两代 libmpv 的渲染 ABI 由上游保证兼容（同一个
//! `MPV_RENDER_API_TYPE_OPENGL` 结构体），因此同一份代码对 0.34 / 0.37 都成立。
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

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use gtk::glib;
use gtk::prelude::*;

// ---------------------------------------------------------------------------
// mpv client API 的常量与结构（对照 /usr/include/mpv/client.h 核对）
// ---------------------------------------------------------------------------

/// `mpv_event_id`：无事件（`mpv_wait_event` 用 0 表示队列已空）
const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_END_FILE: c_int = 7;

/// `mpv_format`：`MPV_FORMAT_INT64`
const MPV_FORMAT_INT64: c_int = 4;

/// `typedef struct mpv_event { mpv_event_id event_id; int error;
///                             uint64_t reply_userdata; void *data; }`
///
/// 只读 `event_id`，其余字段为保持布局一致而声明。
#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

// ---------------------------------------------------------------------------
// libmpv render API 的 FFI 类型（对应 /usr/include/mpv/render.h + render_gl.h）
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

// ---------------------------------------------------------------------------
// 函数指针类型（全部对照官方头文件）
// ---------------------------------------------------------------------------

type MpvCreate = unsafe extern "C" fn() -> *mut c_void;
type MpvInitialize = unsafe extern "C" fn(*mut c_void) -> c_int;
type MpvTerminateDestroy = unsafe extern "C" fn(*mut c_void);
type MpvSetOptionString = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type MpvSetPropertyString =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type MpvGetProperty =
    unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int;
type MpvCommand = unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int;
type MpvWaitEvent = unsafe extern "C" fn(*mut c_void, f64) -> *const MpvEvent;
type MpvErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type MpvGetPropertyString = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
type MpvFree = unsafe extern "C" fn(*mut c_void);

type MpvRenderContextCreate =
    unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut MpvRenderParam) -> c_int;
type MpvRenderContextRender =
    unsafe extern "C" fn(*mut c_void, *mut MpvRenderParam) -> c_int;
type MpvRenderContextFree = unsafe extern "C" fn(*mut c_void);
type MpvRenderContextSetUpdateCallback = unsafe extern "C" fn(
    *mut c_void,
    Option<unsafe extern "C" fn(*mut c_void)>,
    *mut c_void,
);

/// 一次 dlopen 解析出来的全部 libmpv 入口。
///
/// `_lib` 必须活到进程结束 —— 下面所有函数指针都指向它。
struct MpvApi {
    _lib: libloading::Library,
    /// 实际 dlopen 成功的库名（`libmpv.so.2` / `libmpv.so.1` / `libmpv.so`），
    /// 自检（`--selftest`）与日志里要报出来，便于判断系统装的是哪一代。
    lib_name: &'static str,
    create: MpvCreate,
    initialize: MpvInitialize,
    terminate_destroy: MpvTerminateDestroy,
    set_option_string: MpvSetOptionString,
    set_property_string: MpvSetPropertyString,
    get_property: MpvGetProperty,
    command: MpvCommand,
    wait_event: MpvWaitEvent,
    error_string: MpvErrorString,
    get_property_string: MpvGetPropertyString,
    free: MpvFree,
    render_context_create: MpvRenderContextCreate,
    render_context_render: MpvRenderContextRender,
    render_context_free: MpvRenderContextFree,
    render_context_set_update_callback: MpvRenderContextSetUpdateCallback,
}

/// 缓存加载结果（含失败），避免每次创建播放器都重复 dlopen。
static MPV_API: OnceLock<Result<&'static MpvApi, String>> = OnceLock::new();

/// 取一个函数符号。`get::<T>()` 返回 `Symbol<'_, T>`，解引用即得函数指针。
///
/// ⚠️ 不能写成 `get::<Symbol<...>>`（双重包装），那样两个分支类型不一致。
unsafe fn need<T: Copy>(lib: &libloading::Library, name: &[u8]) -> Result<T, String> {
    unsafe { lib.get::<T>(name) }
        .map(|s| *s)
        .map_err(|e| {
            let n = String::from_utf8_lossy(name).trim_end_matches('\0').to_string();
            format!("缺少符号 {n}：{e}")
        })
}

fn load_mpv_api() -> Result<&'static MpvApi, String> {
    // 顺序很重要：24.04+ 是 .so.2，22.04 / Debian 12 是 .so.1。
    // `libmpv.so` 只在装了 -dev 时存在，放最后作为兜底。
    let candidates = ["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
    let mut last_err = String::from("未尝试任何候选库");

    for name in candidates {
        let lib = match unsafe { libloading::Library::new(name) } {
            Ok(l) => l,
            Err(e) => {
                last_err = format!("{name}：{e}");
                continue;
            }
        };
        // 先全部解析到局部变量，再整体构造 —— 否则 `lib` 已被借用又被移动
        let resolved: Result<MpvApi, String> = (|| {
            Ok(MpvApi {
                lib_name: name,
                create: unsafe { need(&lib, b"mpv_create\0")? },
                initialize: unsafe { need(&lib, b"mpv_initialize\0")? },
                terminate_destroy: unsafe { need(&lib, b"mpv_terminate_destroy\0")? },
                set_option_string: unsafe { need(&lib, b"mpv_set_option_string\0")? },
                set_property_string: unsafe { need(&lib, b"mpv_set_property_string\0")? },
                get_property: unsafe { need(&lib, b"mpv_get_property\0")? },
                command: unsafe { need(&lib, b"mpv_command\0")? },
                wait_event: unsafe { need(&lib, b"mpv_wait_event\0")? },
                error_string: unsafe { need(&lib, b"mpv_error_string\0")? },
                get_property_string: unsafe { need(&lib, b"mpv_get_property_string\0")? },
                free: unsafe { need(&lib, b"mpv_free\0")? },
                render_context_create: unsafe { need(&lib, b"mpv_render_context_create\0")? },
                render_context_render: unsafe { need(&lib, b"mpv_render_context_render\0")? },
                render_context_free: unsafe { need(&lib, b"mpv_render_context_free\0")? },
                render_context_set_update_callback: unsafe {
                    need(&lib, b"mpv_render_context_set_update_callback\0")?
                },
                _lib: lib,
            })
        })();

        match resolved {
            Ok(api) => return Ok(Box::leak(Box::new(api))),
            Err(e) => last_err = format!("{name}：{e}"),
        }
    }

    Err(format!(
        "无法加载 libmpv（{last_err}）。请安装 libmpv2（Ubuntu 24.04+）或 libmpv1（22.04 / Debian 12）。"
    ))
}

fn mpv_api() -> Result<&'static MpvApi, String> {
    MPV_API.get_or_init(load_mpv_api).clone()
}

/// `mpv_create` 之前必须满足的全局前提：**进程的 LC_NUMERIC 必须是 "C"**。
///
/// 这是 mpv 的硬性要求（它内部用 `strtod` 解析时间/倍速等参数）。不满足时
/// `mpv_create()` 直接返回 **NULL**，只在 stderr 打印：
/// ```text
/// Non-C locale detected. This is not supported.
/// Call 'setlocale(LC_NUMERIC, "C");' in your code.
/// ```
/// 而 GTK 的 `gtk_init()` 会执行 `setlocale(LC_ALL, "")`，把 LC_NUMERIC
/// 设成用户的 locale（zh_CN.UTF-8 之类）—— 所以必须在 GTK 初始化**之后**
/// 再改回 "C"，否则视频永远起不来（v1.0.1 实测就死在这里）。
///
/// glibc 的 locale 分类常量：LC_CTYPE=0、LC_NUMERIC=1、LC_TIME=2 …
const LC_NUMERIC: c_int = 1;

unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

/// 把 LC_NUMERIC 设为 "C"。幂等、开销极低，可以放心重复调用。
pub fn ensure_c_numeric_locale() {
    unsafe {
        // 用字节串而不是 c"..." 字面量，避免对工具链版本的要求
        setlocale(LC_NUMERIC, b"C\0".as_ptr() as *const c_char);
    }
}

/// 自检用：只做「能不能 dlopen 并解析全部符号」的探测，不创建播放器。
///
/// 返回实际加载到的库名；失败时返回可读原因。
pub fn probe() -> Result<String, String> {
    mpv_api().map(|api| api.lib_name.to_string())
}

/// 自检用：GL 相关动态库是否可加载（mpv 的 get_proc_address 依赖它们）。
pub fn probe_gl_libs() -> Result<String, String> {
    let libs = gl_libs()?;
    let mut names: Vec<&str> = Vec::new();
    if libs.gl.is_some() {
        names.push("libGL.so.1");
    }
    if libs.egl.is_some() {
        names.push("libEGL.so.1");
    }
    if libs.gles.is_some() {
        names.push("libGLESv2.so.2");
    }
    Ok(names.join(", "))
}

/// 把 mpv 返回码翻译成可读信息（失败时退回错误码数字）。
fn mpv_err(api: &MpvApi, rc: c_int) -> String {
    let p = unsafe { (api.error_string)(rc) };
    if p.is_null() {
        return format!("错误码 {rc}");
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().to_string();
    format!("{s}（{rc}）")
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
/// `GL_RGBA8` —— 告诉 mpv 目标 FBO 是带 alpha 的 8 位 RGBA。
const GL_RGBA8: c_int = 0x8058;
type GlGetIntegerv = unsafe extern "C" fn(u32, *mut c_int);
type GlClearColor = unsafe extern "C" fn(f32, f32, f32, f32);
type GlClear = unsafe extern "C" fn(u32);
const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;
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
    let Some(p) = gl_proc(b"glGetIntegerv\0") else {
        return 0;
    };
    let f: GlGetIntegerv = unsafe { std::mem::transmute(p) };
    let mut fbo: c_int = 0;
    unsafe { f(GL_FRAMEBUFFER_BINDING, &mut fbo) };
    fbo
}

/// 按当前会话选出的 GL 入口解析顺序。
///
/// ⚠️ 这是 v1.4.0「黑底」的根因：之前 `current_fbo()` / `clear_transparent()`
/// 直接 dlsym libGL 拿 `glGetIntegerv` / `glClear`，而 GTK 在 Wayland（以及
/// 现代 X11）上用的是 **EGL 上下文**。libGL 的入口按它自己的 TLS 当前上下文
/// 分发，对 EGL 上下文取不到 → `GL_FRAMEBUFFER_BINDING` 读回 **0**。
/// 于是我们告诉 mpv「渲染到默认帧缓冲」，mpv 就不再认为目标 FBO 带 alpha，
/// 透明像素被混合成黑色。
///
/// 实测日志：`视频参数: … fbo=0` —— 一眼就能看出是这里。
fn prefer_egl() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

/// 解析一个 GL 入口地址（走上下文所属 API 的 getProcAddress，最后才 dlsym）。
fn gl_proc(name: &[u8]) -> Option<*mut c_void> {
    let libs = gl_libs().ok()?;
    let cname = name.as_ptr() as *const c_char;

    unsafe {
        let try_egl = || -> Option<*mut c_void> {
            let egl = libs.egl.as_ref()?;
            let f = egl.get::<GetProcAddressFn>(b"eglGetProcAddress\0").ok()?;
            let p = f(cname);
            (!p.is_null()).then_some(p)
        };
        let try_glx = || -> Option<*mut c_void> {
            let gl = libs.gl.as_ref()?;
            let f = gl.get::<GetProcAddressFn>(b"glXGetProcAddressARB\0").ok()?;
            let p = f(cname);
            (!p.is_null()).then_some(p)
        };

        // Wayland 优先 EGL，X11 优先 GLX；两条都试一遍再退回 dlsym。
        if prefer_egl() {
            if let Some(p) = try_egl() {
                return Some(p);
            }
            if let Some(p) = try_glx() {
                return Some(p);
            }
        } else {
            if let Some(p) = try_glx() {
                return Some(p);
            }
            if let Some(p) = try_egl() {
                return Some(p);
            }
        }

        // 兜底：核心入口可能被直接导出
        for lib in [&libs.gl, &libs.gles].into_iter().flatten() {
            if let Ok(sym) = lib.get::<*mut c_void>(name) {
                if !(*sym).is_null() {
                    return Some(*sym);
                }
            }
        }
    }
    None
}

/// 把当前 FBO 清成透明黑 (0,0,0,0)。
///
/// mpv 渲染只覆盖视频区域，FBO 其余部分保留上一次的内容；
/// 必须显式清成透明，空隙处透出的才是桌面而非黑块。
fn clear_transparent() {
    let (Some(p_color), Some(p_clear)) = (
        gl_proc(b"glClearColor\0"),
        gl_proc(b"glClear\0"),
    ) else {
        return;
    };
    let clear_color: GlClearColor = unsafe { std::mem::transmute(p_color) };
    let clear: GlClear = unsafe { std::mem::transmute(p_clear) };
    unsafe {
        clear_color(0.0, 0.0, 0.0, 0.0);
        clear(GL_COLOR_BUFFER_BIT);
    }
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
    /// 已解析的 libmpv 入口（进程级共享）
    api: &'static MpvApi,
    /// `mpv_handle *`
    handle: *mut c_void,
    /// `mpv_render_context *`（不透明指针）
    render_ctx: *mut c_void,
    /// mpv 回调置位用。放在 Box 里以保证地址稳定（回调持有其裸指针）
    update_flag: Box<AtomicBool>,
    /// 持有 GLArea 的弱引用，仅用于日志/调试用途
    gl_area: glib::WeakRef<gtk::GLArea>,
    loaded: String,
    /// 是否已经打印过渲染目标（只打一次）
    logged_fbo: bool,
}

impl VideoPlayer {
    /// 创建播放器。
    ///
    /// 必须在 GL 上下文**已经建立且 current** 之后调用
    /// （即 GLArea 的 `render` 回调内），否则 render context 创建会失败。
    pub fn new(gl_area: &gtk::GLArea) -> Result<Self, String> {
        // ⚠️ 必须在 mpv_create 之前把 LC_NUMERIC 设回 "C"，
        //    否则 mpv_create 返回 NULL（详见 ensure_c_numeric_locale 的注释）。
        ensure_c_numeric_locale();

        let api = mpv_api()?;
        // 加载 GL 相关动态库；句柄随进程存活，作为 get_proc_address 的 ctx
        let gl_ctx = gl_libs()? as *const GlLibraries as *mut c_void;

        // 1) 创建 handle（此时选项还没设，先不能发命令）
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("mpv_create 返回空指针".to_string());
        }

        // 2) initialize 之前逐个设置选项。
        //    `vo=libmpv` 是使用 render API 的前提（由 vo_libmpv.c 提供渲染后端）。
        //    个别选项名若不被当前 libmpv 支持，**不应**让整个播放器创建失败，
        //    因此逐个容错处理。
        let opts: Vec<(&str, &str)> = vec![
            ("vo", "libmpv"),
            // ⚠️ **必须软解**。素材是 VP9 + Alpha（Matroska 的 BlockAdditional
            //    透明平面），硬件解码（VAAPI/NVDEC 等）只输出 YUV 主平面，
            //    **会丢掉 alpha 通道** → 实测表现就是「绿幕没去掉」，
            //    猫背后露出一整块绿底。auto-safe 在多数机器上会选中硬解，
            //    所以这里显式关掉。
            ("hwdec", "no"),
            // 禁用音频（猫咪动画无声）
            ("audio", "no"),
            ("really-quiet", "yes"),
            // 保留 alpha 通道（透明背景 webm 必需）
            ("alpha", "yes"),
            // ── 抠像 ────────────────────────────────────────────────
            // 实测内置素材 **没有 alpha 通道**（ffprobe: pix_fmt=yuv420p，
            // 1920×1080 不透明），背景是纯绿 #00AE3E。之前一直以为「绿幕」
            // 是 alpha 丢了，其实是素材本身就是绿底。
            //
            // 这里用 libavfilter 的 colorkey 在解码后把纯绿抠成透明，
            // 输出 yuva420p；配合上面的 alpha=yes，vo_gpu 就会保留 alpha。
            // 选 0x00AE3E 精确匹配 + 较小容差，避免误伤猫身上的颜色。
            // 若用户自备素材本来就是带 alpha 的 webm，这里等于没起作用。
            (
                "vf",
                "lavfi=[colorkey=0x00AE3E:0.25:0.10]",
            ),
            // ⚠️ 显式指定内部 FBO 格式带 alpha。
            //    auto 时 mpv 可能在中间 FBO 上用不带 alpha 的格式，
            //    渲染链路上把透明像素丢掉，最后只剩黑底。
            ("fbo-format", "rgba8"),
            ("scale", "bilinear"),
            ("keep-open", "yes"),
            ("keep-open-pause", "yes"),
            ("idle", "yes"),
            ("loop-file", "no"),
            // 不做视频计时等待，避免阻塞 GTK 主线程
            ("video-timing-offset", "0"),
        ];
        for (k, v) in opts {
            let (Ok(ck), Ok(cv)) = (CString::new(k), CString::new(v)) else {
                continue;
            };
            let rc = unsafe { (api.set_option_string)(handle, ck.as_ptr(), cv.as_ptr()) };
            if rc < 0 {
                eprintln!(
                    "[purr-pause-linux] mpv 选项 {k} 设置失败（已忽略）：{}",
                    mpv_err(api, rc)
                );
            }
        }

        // 3) 初始化核心
        let rc = unsafe { (api.initialize)(handle) };
        if rc < 0 {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!("mpv 初始化失败：{}", mpv_err(api, rc)));
        }

        // 保险：有些 mpv 版本会在 initialize 时重置 VO 选项，
        // 这里用属性接口再确认一次 alpha。失败不致命（选项通常已生效）。
        {
            let (Ok(k), Ok(v)) = (CString::new("alpha"), CString::new("yes")) else {
                unreachable!()
            };
            unsafe { (api.set_property_string)(handle, k.as_ptr(), v.as_ptr()) };
        }

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
        let rc = unsafe { (api.render_context_create)(&mut render_ctx, handle, params.as_mut_ptr()) };
        if rc < 0 || render_ctx.is_null() {
            unsafe { (api.terminate_destroy)(handle) };
            return Err(format!(
                "mpv render context 创建失败（{}）—— GL 上下文是否已 current？",
                mpv_err(api, rc)
            ));
        }

        let update_flag = Box::new(AtomicBool::new(false));
        unsafe {
            (api.render_context_set_update_callback)(
                render_ctx,
                Some(on_mpv_update),
                &*update_flag as *const AtomicBool as *mut c_void,
            );
        }

        Ok(Self {
            api,
            handle,
            render_ctx,
            update_flag,
            gl_area: gl_area.downgrade(),
            loaded: String::new(),
            logged_fbo: false,
        })
    }

    /// 发一条 mpv 命令（参数以 NULL 结尾）。
    fn command(&self, args: &[&str]) -> Result<(), String> {
        let cstrings: Vec<CString> = args
            .iter()
            .filter_map(|a| CString::new(*a).ok())
            .collect();
        if cstrings.len() != args.len() {
            return Err("命令参数含非法字符".to_string());
        }
        let mut ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());

        let rc = unsafe { (self.api.command)(self.handle, ptrs.as_ptr()) };
        if rc < 0 {
            Err(mpv_err(self.api, rc))
        } else {
            Ok(())
        }
    }

    /// 设置字符串型属性（失败只记录，不向上传播 —— 这些都属于"尽力而为"）。
    fn set_prop(&self, name: &str, value: &str) {
        let (Ok(cn), Ok(cv)) = (CString::new(name), CString::new(value)) else {
            return;
        };
        let rc = unsafe { (self.api.set_property_string)(self.handle, cn.as_ptr(), cv.as_ptr()) };
        if rc < 0 {
            eprintln!(
                "[purr-pause-linux] mpv 属性 {name}={value} 设置失败：{}",
                mpv_err(self.api, rc)
            );
        }
    }

    /// 读取字符串型属性（失败返回 None）。
    fn get_str(&self, name: &str) -> Option<String> {
        let cn = CString::new(name).ok()?;
        let p = unsafe { (self.api.get_property_string)(self.handle, cn.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().to_string();
        unsafe { (self.api.free)(p as *mut c_void) };
        Some(s)
    }

    /// 排查用：把与「透明/抠像」相关的关键参数打一行出来。
    ///
    /// 这一行能直接区分两类失败：
    ///   · `源alpha=no`      → 解码后没有 alpha（抠像滤镜没生效）
    ///   · `alpha选项` 不是 yes → mpv 会把透明像素混合成黑底
    pub fn debug_dump(&self) {
        // ⚠️ 这里**不要**读 fbo：本函数由定时器调用，此时 GTK 的 GL 上下文
        //    并没有 current，glGetIntegerv 只会读到 0（曾经因此误判过一轮）。
        //    真实的 fbo 在 draw() 里打印。
        let dec = self.get_str("video-params/pixelformat").unwrap_or_default();
        let out = self.get_str("video-out-params/pixelformat").unwrap_or_default();
        let alpha_opt = self.get_str("alpha").unwrap_or_default();
        let hwdec = self.get_str("hwdec-current").unwrap_or_default();
        eprintln!(
            "[purr-pause-linux] 视频参数: 解码={dec} 滤镜后={out} alpha选项={alpha_opt} hwdec={hwdec}"
        );
        eprintln!(
            "[purr-pause-linux] 会话: type={} wayland={} 用EGL={}",
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "?".into()),
            std::env::var("WAYLAND_DISPLAY").is_ok(),
            prefer_egl(),
        );
    }

    /// 读取 int64 型属性。
    fn get_i64(&self, name: &str) -> Option<i64> {
        let cn = CString::new(name).ok()?;
        let mut v: i64 = 0;
        let rc = unsafe {
            (self.api.get_property)(
                self.handle,
                cn.as_ptr(),
                MPV_FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        if rc < 0 {
            None
        } else {
            Some(v)
        }
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
        self.command(&["loadfile", &url, "replace"])
            .map_err(|e| format!("loadfile 失败：{e}"))?;
        self.loaded = p;
        // ⚠️ 视频输出（VO）是在第一次 loadfile 时才真正创建的。
        //    在 initialize 之前设的 alpha 有可能只进了选项缓存没到 VO，
        //    这里在 VO 已存在之后再设一次（失败会打印，便于排查）。
        self.set_prop("alpha", "yes");
        Ok(())
    }

    pub fn play(&mut self) {
        self.set_prop("pause", "no");
    }

    #[allow(dead_code)]
    pub fn pause(&mut self) {
        self.set_prop("pause", "yes");
    }

    pub fn set_loop(&mut self, on: bool) {
        // 字符串形式最省事：布尔属性接受 yes/no，loop-file 另接受 inf
        self.set_prop("loop-file", if on { "inf" } else { "no" });
    }

    pub fn seek(&mut self, pos: f64) {
        let _ = self.command(&["seek", &format!("{pos}"), "absolute"]);
    }

    /// 视频宽高比（宽 / 高）。未就绪时返回 None。
    pub fn aspect(&self) -> Option<f64> {
        let w = self.get_i64("width").filter(|v| *v > 0)? as f64;
        let h = self.get_i64("height").filter(|v| *v > 0)? as f64;
        Some(w / h)
    }

    /// 事件泵：处理 mpv 事件。需在主循环中定期调用，否则事件队列会堆积。
    ///
    /// 返回 true 表示当前文件已播放完毕（EOF）或 mpv 已关闭。
    pub fn pump_events(&mut self) -> bool {
        let mut ended = false;
        loop {
            let ev = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            if ev.is_null() {
                break;
            }
            let id = unsafe { (*ev).event_id };
            // MPV_EVENT_NONE 表示队列已空
            if id == MPV_EVENT_NONE {
                break;
            }
            if id == MPV_EVENT_END_FILE || id == MPV_EVENT_SHUTDOWN {
                ended = true;
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
        self.get_i64("width").unwrap_or(0) > 0
    }

    /// 把当前帧渲染到当前绑定的帧缓冲。
    ///
    /// 只能在 GTK `render` 信号回调内调用（须已 `make_current`）。
    pub fn draw(&mut self, w: c_int, h: c_int) {
        if w <= 0 || h <= 0 || self.render_ctx.is_null() {
            return;
        }
        let fbo_id = current_fbo();
        if !self.logged_fbo {
            self.logged_fbo = true;
            eprintln!(
                "[purr-pause-linux] 渲染目标: fbo={fbo_id} internal_format={GL_RGBA8}（固定）"
            );
        }
        let mut fbo = MpvOpenglFbo {
            fbo: fbo_id,
            w,
            h,
            // ⚠️ **无条件**告诉 mpv「目标是带 alpha 的 RGBA8」。
            //
            // 之前按 `fbo_id == 0 ? 0 : GL_RGBA8` 来给，实测在这台机器上
            // `fbo_id` 读出来是 0（GTK 的 GLArea FBO 号拿不到），于是
            // internal_format 也变成 0 —— mpv 就把目标当成「没有 alpha 的
            // 默认帧缓冲」，把 colorkey 抠出来的透明像素混合成黑色。
            // 诊断行 `滤镜后=argb` 证明 alpha 是有的，问题只在这一句。
            //
            // GTK4 的 GLArea 离屏纹理固定是 GL_RGBA8（gtkglarea.c 里写死），
            // Wayland 的默认帧缓冲也是 RGBA，所以无条件给 RGBA8 是对的。
            internal_format: GL_RGBA8,
        };
        let mut flip_y: c_int = FLIP_Y;
        // 先把 FBO 清成透明黑：mpv 只覆盖视频区域（宽高比不同时四周留空），
        // 不清的话残留内容会以黑块呈现。
        clear_transparent();
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
            let _ = (self.api.render_context_render)(self.render_ctx, params.as_mut_ptr());
        }
    }

    /// 释放 render context。幂等。
    ///
    /// 应在 GL 上下文仍可用时调用（GLArea 的 `unrealize`），
    /// 以便 mpv 正常释放它创建的 GL 资源。
    /// 注意顺序：必须先释放 render context，再销毁 mpv handle。
    pub fn shutdown(&mut self) {
        if self.render_ctx.is_null() {
            return;
        }
        if let Some(area) = self.gl_area.upgrade() {
            // 尽量让 GL 上下文 current（unrealize 阶段可能已失效，不保证成功）
            area.make_current();
        }
        unsafe {
            (self.api.render_context_free)(self.render_ctx);
        }
        self.render_ctx = ptr::null_mut();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // 若 unrealize 已调用过 shutdown，这里是空操作
        self.shutdown();
        if self.handle.is_null() {
            return;
        }
        let _ = self.command(&["quit"]);
        // 与 mpv crate 的 MpvHandler::drop 行为一致
        unsafe { (self.api.terminate_destroy)(self.handle) };
        self.handle = ptr::null_mut();
    }
}

//! 系统托盘：StatusNotifierItem / AppIndicator。
//!
//! GTK4 本身不含托盘实现，需使用 libayatana-appindicator（AppIndicator3 的活跃分支）。
//! 通过 C FFI 调用，不引入额外 Rust 依赖。

use gtk::prelude::*;
use std::ffi::{c_char, c_int, c_void, CString};

// AppIndicator 部分符号（从 libayatana-appindicator3.so dlsym）
type IndicatorNew = unsafe extern "C" fn(*const c_char, *const c_char, c_int) -> *mut c_void;
type IndicatorSetIcon = unsafe extern "C" fn(*mut c_void, *const c_char);
type IndicatorSetStatus = unsafe extern "C" fn(*mut c_void, c_int);
type IndicatorSetMenu = unsafe extern "C" fn(*mut c_void, *mut c_void);

// GTK / GLib 部分符号。
//
// 注意：这些符号**不能**从 libayatana-appindicator3.so 里 dlsym ——
// AppIndicator 是独立 so，不导出 GTK 符号；必须从 libgtk-4.so / libgobject-2.0.so 取。
// 用 `#[link(name=...)]` 静态链接更稳妥：由系统动态链接器在运行时解析。
unsafe extern "C" {
    #[link(name = "gtk-4")]
    fn gtk_menu_new() -> *mut c_void;
    #[link(name = "gtk-4")]
    fn gtk_menu_shell_append(menu: *mut c_void, child: *mut c_void);
    #[link(name = "gtk-4")]
    fn gtk_menu_item_new(label: *const c_char) -> *mut c_void;
    #[link(name = "gtk-4")]
    fn gtk_menu_item_set_submenu(item: *mut c_void, submenu: *mut c_void);
    #[link(name = "gtk-4")]
    fn gtk_menu_item_set_label(item: *mut c_void, label: *const c_char);
    #[link(name = "gtk-4")]
    fn gtk_menu_item_set_sensitive(item: *mut c_void, sensitive: c_int);

    #[link(name = "gobject-2.0")]
    fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        c_handler: *mut c_void,
        data: *mut c_void,
        destroy_data: *mut c_void,
        connect_flags: c_int,
    );

    #[link(name = "gobject-2.0")]
    fn g_object_unref(obj: *mut c_void) -> *mut c_void;
}

/// AppIndicator 库句柄（仅用于解析 AppIndicator 自身符号）。
struct IndicatorLib {
    _handle: libloading::Library,
    indicator_new: IndicatorNew,
    indicator_set_icon: IndicatorSetIcon,
    indicator_set_status: IndicatorSetStatus,
    indicator_set_menu: IndicatorSetMenu,
}

impl IndicatorLib {
    fn load() -> Option<Self> {
        let candidates = [
            "libayatana-appindicator3.so.1",
            "libayatana-appindicator3.so",
            "libappindicator3.so.1",
            "libappindicator3.so",
        ];
        for name in candidates {
            // unsafe：动态库名来自固定白名单，非用户输入
            if let Ok(h) = unsafe { libloading::Library::new(name) } {
                unsafe {
                    let sym = |s: &[u8]| -> Result<*mut c_void, libloading::Error> { h.get(s) };
                    let Ok(f) = sym(b"app_indicator_new\0") else {
                        continue;
                    };
                    return Some(Self {
                        _handle: h,
                        indicator_new: std::mem::transmute(f),
                        indicator_set_icon: std::mem::transmute(
                            sym(b"app_indicator_set_icon\0").ok()?,
                        ),
                        indicator_set_status: std::mem::transmute(
                            sym(b"app_indicator_set_status\0").ok()?,
                        ),
                        indicator_set_menu: std::mem::transmute(
                            sym(b"app_indicator_set_menu\0").ok()?,
                        ),
                    });
                }
            }
        }
        None
    }
}

/// 一个菜单项及其回调。
pub struct TrayItem {
    pub label: String,
    pub sensitive: bool,
    pub submenu: Option<Vec<TrayItem>>,
    pub action: Option<Box<dyn Fn() + 'static>>,
}

/// 托盘图标封装。
///
/// 生命周期管理要点：
///   - 每次 `set_menu` 会创建新的 GtkMenu，旧的必须 unref，否则泄漏
///   - 每个带回调的菜单项持有一个 `Box<dyn Fn()>` 裸指针；
///     通过 `g_signal_connect_data` 的 destroy_notify 机制回收
///     （传 0 表示不自动销毁，故改为自管：在菜单 unref 时统一清理）
pub struct Tray {
    lib: IndicatorLib,
    indicator: *mut c_void,
    app_name: CString,
    /// 当前挂载的菜单（所有权在本结构，替换时释放旧的）
    menu: *mut c_void,
    /// 当前菜单里所有回调 Box 的裸指针，菜单销毁时一并回收
    callbacks: Vec<*mut c_void>,
}

// AppIndicator/GTK 对象只在 UI 主线程使用。
unsafe impl Send for Tray {}

impl Tray {
    pub fn new(app_name: &str, icon_name: &str) -> Option<Self> {
        let lib = IndicatorLib::load()?;
        let app = CString::new(app_name).ok()?;
        let icon = CString::new(icon_name).ok()?;

        unsafe {
            let indicator = (lib.indicator_new)(app.as_ptr(), icon.as_ptr(), INDICATOR_PASSIVE);
            if indicator.is_null() {
                return None;
            }
            Some(Self {
                lib,
                indicator,
                app_name: app,
                menu: std::ptr::null_mut(),
                callbacks: Vec::new(),
            })
        }
    }

    /// 重建托盘菜单。先释放旧菜单与旧回调，再挂载新菜单。
    pub fn set_menu(&mut self, items: &[TrayItem]) {
        unsafe {
            // 1) 回收上一轮菜单与回调
            self.free_current_menu();

            // 2) 构建新菜单
            let menu = gtk_menu_new();
            if menu.is_null() {
                return;
            }
            for item in items {
                let native = self.build_item(item);
                if !native.is_null() {
                    gtk_menu_shell_append(menu, native);
                }
            }

            // 3) 挂到 indicator
            (self.lib.indicator_set_menu)(self.indicator, menu);
            (self.lib.indicator_set_status)(self.indicator, INDICATOR_ACTIVE);

            self.menu = menu;
        }
    }

    unsafe fn free_current_menu(&mut self) {
        // 回收回调 Box
        for cb in self.callbacks.drain(..) {
            if !cb.is_null() {
                drop(Box::from_raw(cb as *mut Box<dyn Fn()>));
            }
        }
        // 释放菜单（会连带释放其下的菜单项）
        if !self.menu.is_null() {
            g_object_unref(self.menu);
            self.menu = std::ptr::null_mut();
        }
    }

    unsafe fn build_item(&mut self, item: &TrayItem) -> *mut c_void {
        let Ok(label) = CString::new(item.label.as_str()) else {
            return std::ptr::null_mut();
        };
        let mi = gtk_menu_item_new(label.as_ptr());
        if mi.is_null() {
            return std::ptr::null_mut();
        }
        gtk_menu_item_set_sensitive(mi, if item.sensitive { 1 } else { 0 });

        if let Some(sub) = &item.submenu {
            let sm = gtk_menu_new();
            for s in sub {
                let child = self.build_item(s);
                if !child.is_null() {
                    gtk_menu_shell_append(sm, child);
                }
            }
            gtk_menu_item_set_submenu(mi, sm);
        } else if let Some(action) = &item.action {
            // 把 Rust 闭包装箱后转成裸指针，作为 signal 的 user_data。
            // 生命周期由 self.callbacks 管理，菜单销毁时统一 drop。
            let boxed: Box<dyn Fn()> = Box::new(move || action());
            let raw = Box::into_raw(Box::new(boxed)) as *mut c_void;
            self.callbacks.push(raw as *mut *mut c_void as *mut c_void);

            g_signal_connect_data(
                mi,
                c"activate".as_ptr(),
                on_activate as *mut c_void,
                raw,
                std::ptr::null_mut(),
                0,
            );
        }
        mi
    }

    pub fn set_icon(&mut self, icon_name: &str) {
        if let Ok(icon) = CString::new(icon_name) {
            unsafe {
                (self.lib.indicator_set_icon)(self.indicator, icon.as_ptr());
            }
        }
    }
}

/// GTK "activate" 信号回调。user_data 是 `*mut Box<dyn Fn()>`。
extern "C" fn on_activate(_item: *mut c_void, user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    unsafe {
        let f = &*(user_data as *const Box<dyn Fn()>);
        f();
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            self.free_current_menu();
            if !self.indicator.is_null() {
                g_object_unref(self.indicator);
                self.indicator = std::ptr::null_mut();
            }
        }
        let _ = &self.app_name;
    }
}

pub const INDICATOR_PASSIVE: c_int = 1;
pub const INDICATOR_ACTIVE: c_int = 0;

/// 判断当前会话是否有可用的托盘宿主。
pub fn tray_supported() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| {
            let d = d.to_lowercase();
            d.contains("gnome")
                || d.contains("kde")
                || d.contains("plasma")
                || d.contains("xfce")
                || d.contains("lxde")
                || d.contains("lxqt")
                || d.contains("mate")
                || d.contains("unity")
                || d.contains("cinnamon")
                || d.contains("budgie")
        })
        .unwrap_or(false)
}

/// 托盘不可用时的降级提示文案。
pub fn tray_fallback_hint() -> String {
    "当前桌面环境没有可用的系统托盘宿主\n（例如 GNOME 需安装 gnome-shell-extension-appindicator）。\n应用将无法通过托盘菜单访问设置。"
        .to_string()
}

//! 系统托盘：StatusNotifierItem / AppIndicator。
//!
//! GTK4 本身不含托盘实现，需使用 libayatana-appindicator（AppIndicator3 的活跃分支）。
//! 通过 C FFI 调用，不引入额外 Rust 依赖。

use std::ffi::{c_char, c_int, c_void, CString};

// AppIndicator 部分符号（从 libayatana-appindicator3.so dlsym）
type IndicatorNew = unsafe extern "C" fn(*const c_char, *const c_char, c_int) -> *mut c_void;
type IndicatorSetIcon = unsafe extern "C" fn(*mut c_void, *const c_char);
type IndicatorSetStatus = unsafe extern "C" fn(*mut c_void, c_int);
type IndicatorSetMenu = unsafe extern "C" fn(*mut c_void, *mut c_void);

// GObject 符号在 GTK4 环境里确实存在，可以静态链接。
// ⚠️ `#[link]` 属性必须放在 extern **块**上：放在单个 fn 上会被 rustc 警告
//    "the link attribute cannot be used on foreign functions"，
//    且该警告在未来版本会成为硬错误。
#[link(name = "gobject-2.0")]
unsafe extern "C" {
    fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        c_handler: *mut c_void,
        data: *mut c_void,
        destroy_data: *mut c_void,
        connect_flags: c_int,
    );

    fn g_object_unref(obj: *mut c_void) -> *mut c_void;
}

// ---- GTK 菜单符号：绝不能静态链接 -----------------------------------------
//
// GTK4 **移除了 GtkMenu / GtkMenuItem**（由 GMenu + PopoverMenu 取代），
// 因此 libgtk-4.so 里**没有** gtk_menu_new 等符号。
// 若照旧写 `#[link(name = "gtk-4")]`，编译能过，但链接阶段会报
// `undefined reference to 'gtk_menu_new'` —— 这类错误要到 Build 步骤最后才暴露。
//
// 所以这里一律改为**运行时 dlsym**。GTK4 环境下解析必然失败 →
// `GtkMenuApi::load()` 返回 None → `Tray::new()` 返回 None →
// 应用自动降级到 `tray_fallback_hint()`。
//
// 这是当前架构下唯一诚实的行为：libayatana-appindicator3 是基于 GTK3 的库，
// 其 `app_indicator_set_menu` 需要 GTK3 的 GtkMenu；在 GTK4 进程内加载 GTK3
// 会引发符号冲突，故「GTK4 应用 + appindicator 菜单」这条路走不通。
//
// TODO(托盘): 若要真正支持托盘菜单，应改用 D-Bus StatusNotifierItem
//   （org.kde.StatusNotifierItem + com.canonical.dbusmenu），通过
//   gio::DBusConnection 实现，完全不依赖 GTK3。菜单项结构（TrayItem）可直接复用。
type GtkMenuNew = unsafe extern "C" fn() -> *mut c_void;
type GtkMenuShellAppend = unsafe extern "C" fn(*mut c_void, *mut c_void);
type GtkMenuItemNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GtkMenuItemSetSubmenu = unsafe extern "C" fn(*mut c_void, *mut c_void);
type GtkMenuItemSetSensitive = unsafe extern "C" fn(*mut c_void, c_int);

/// 从指定 so 解析一个符号为裸指针（自由函数，避免闭包捕获导致后续 move 冲突）。
unsafe fn resolve_sym(h: &libloading::Library, name: &[u8]) -> Option<*mut c_void> {
    unsafe { h.get::<*mut c_void>(name).ok().map(|sym| *sym) }
}

/// 运行时解析出的 GTK 菜单 API。**在 GTK4 上解析失败是预期行为**。
struct GtkMenuApi {
    /// 持有 so 句柄，保证函数指针在整个生命周期内有效
    _handle: libloading::Library,
    menu_new: GtkMenuNew,
    menu_shell_append: GtkMenuShellAppend,
    menu_item_new: GtkMenuItemNew,
    menu_item_set_submenu: GtkMenuItemSetSubmenu,
    menu_item_set_sensitive: GtkMenuItemSetSensitive,
}

impl GtkMenuApi {
    fn load() -> Option<Self> {
        for name in ["libgtk-4.so.1", "libgtk-4.so.0", "libgtk-4.so"] {
            let Ok(h) = (unsafe { libloading::Library::new(name) }) else {
                continue;
            };
            // 先把全部符号解析到局部变量，最后再 move h（否则 E0505）
            let Some(menu_new) = (unsafe { resolve_sym(&h, b"gtk_menu_new\0") }) else {
                continue;
            };
            let Some(shell_append) =
                (unsafe { resolve_sym(&h, b"gtk_menu_shell_append\0") })
            else {
                continue;
            };
            let Some(item_new) = (unsafe { resolve_sym(&h, b"gtk_menu_item_new\0") }) else {
                continue;
            };
            let Some(item_submenu) =
                (unsafe { resolve_sym(&h, b"gtk_menu_item_set_submenu\0") })
            else {
                continue;
            };
            let Some(item_sensitive) =
                (unsafe { resolve_sym(&h, b"gtk_menu_item_set_sensitive\0") })
            else {
                continue;
            };
            return Some(Self {
                _handle: h,
                menu_new: unsafe { std::mem::transmute(menu_new) },
                menu_shell_append: unsafe { std::mem::transmute(shell_append) },
                menu_item_new: unsafe { std::mem::transmute(item_new) },
                menu_item_set_submenu: unsafe { std::mem::transmute(item_submenu) },
                menu_item_set_sensitive: unsafe { std::mem::transmute(item_sensitive) },
            });
        }
        None
    }
}

/// AppIndicator 库句柄（仅用于解析 AppIndicator 自身符号）。
struct IndicatorLib {
    _handle: libloading::Library,
    indicator_new: IndicatorNew,
    indicator_set_icon: IndicatorSetIcon,
    indicator_set_status: IndicatorSetStatus,
    indicator_set_menu: IndicatorSetMenu,
    /// GTK 菜单 API。GTK4 上解析失败 → 整个托盘不可用。
    gtk: GtkMenuApi,
}

impl IndicatorLib {
    fn load() -> Option<Self> {
        // 先确认 GTK 菜单符号可用。GTK4 已移除 GtkMenu，这里必然失败，
        // 于是整个 load 直接返回 None —— 也就不必白加载 appindicator。
        let gtk = GtkMenuApi::load()?;

        let candidates = [
            "libayatana-appindicator3.so.1",
            "libayatana-appindicator3.so",
            "libappindicator3.so.1",
            "libappindicator3.so",
        ];
        for name in candidates {
            // unsafe：动态库名来自固定白名单，非用户输入
            let Ok(h) = (unsafe { libloading::Library::new(name) }) else {
                continue;
            };
            // 先把 4 个符号全部解析到局部变量，最后再 move h ——
            // 否则闭包/借用会与 `_handle: h` 的 move 冲突（E0505）。
            let Some(new_f) = (unsafe { resolve_sym(&h, b"app_indicator_new\0") }) else {
                continue;
            };
            let Some(icon_f) = (unsafe { resolve_sym(&h, b"app_indicator_set_icon\0") }) else {
                continue;
            };
            let Some(status_f) =
                (unsafe { resolve_sym(&h, b"app_indicator_set_status\0") })
            else {
                continue;
            };
            let Some(menu_f) = (unsafe { resolve_sym(&h, b"app_indicator_set_menu\0") }) else {
                continue;
            };
            return Some(Self {
                _handle: h,
                indicator_new: unsafe { std::mem::transmute(new_f) },
                indicator_set_icon: unsafe { std::mem::transmute(icon_f) },
                indicator_set_status: unsafe { std::mem::transmute(status_f) },
                indicator_set_menu: unsafe { std::mem::transmute(menu_f) },
                gtk,
            });
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
            // 先把函数指针拷到局部变量（fn 指针是 Copy）：这样 `&self.lib.gtk`
            // 的不可变借用立即结束，不会与下面的 `self.build_item(&mut self)` 冲突。
            let menu_new = self.lib.gtk.menu_new;
            let menu_shell_append = self.lib.gtk.menu_shell_append;
            let menu = (menu_new)();
            if menu.is_null() {
                return;
            }
            for item in items {
                let native = self.build_item(item);
                if !native.is_null() {
                    (menu_shell_append)(menu, native);
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
        // 同样先把函数指针拷到局部变量，避免 `&self.lib.gtk` 的不可变借用
        // 跨越后面的 `self.build_item(&mut self)` 与 `self.callbacks.push`（E0502）。
        let menu_new = self.lib.gtk.menu_new;
        let menu_item_new = self.lib.gtk.menu_item_new;
        let menu_item_set_sensitive = self.lib.gtk.menu_item_set_sensitive;
        let menu_shell_append = self.lib.gtk.menu_shell_append;
        let menu_item_set_submenu = self.lib.gtk.menu_item_set_submenu;

        let mi = (menu_item_new)(label.as_ptr());
        if mi.is_null() {
            return std::ptr::null_mut();
        }
        (menu_item_set_sensitive)(mi, if item.sensitive { 1 } else { 0 });

        if let Some(sub) = &item.submenu {
            let sm = (menu_new)();
            for s in sub {
                let child = self.build_item(s);
                if !child.is_null() {
                    (menu_shell_append)(sm, child);
                }
            }
            (menu_item_set_submenu)(mi, sm);
        } else if let Some(action) = &item.action {
            // 把 Rust 闭包装箱后转成裸指针，作为 signal 的 user_data。
            // 生命周期由 self.callbacks 管理，菜单销毁时统一 drop。
            //
            // ⚠️ 必须只装箱一层：若写成 Box::into_raw(Box::new(boxed))，
            //    存进去的是 *mut Box<dyn Fn()>，而 on_activate 按 *mut Box<dyn Fn()>
            //    解引用，层级不匹配会读到错误的 vtable（运行时 UB）。
            let boxed: Box<dyn Fn()> = Box::new(move || action());
            let raw = Box::into_raw(boxed) as *mut c_void;
            self.callbacks.push(raw);

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

    #[allow(dead_code)]
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
#[allow(dead_code)]
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

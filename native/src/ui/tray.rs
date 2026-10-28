//! 系统托盘：D-Bus StatusNotifierItem + com.canonical.dbusmenu。
//!
//! GTK4 无法再加载 GTK3 的 libayatana-appindicator（同进程符号冲突），
//! 因此直接实现 StatusNotifierItem 协议。Ubuntu 桌面自带的 AppIndicators
//! 扩展（`org.kde.StatusNotifierWatcher`）与 KDE Plasma 原生支持该协议。
//!
//! 协议细节均对照参考实现源码核实（Ubuntu 22.04 实际发布的版本）：
//!   - libayatana-appindicator 0.5.4：SNI 属性集与注册方式 ——
//!     `RegisterStatusNotifierItem(s)` 传**对象路径**，watcher 从 D-Bus
//!     sender 反查总线名（gnome-shell-extension-appindicator 42 的
//!     statusNotifierWatcher.js 按「首字符是否为 '/'」区分两种风格）。
//!   - libdbusmenu 18.10：GetLayout 节点结构为 `(ia{sv}av)` —— id 是 i32、
//!     根节点 id 恒为 0、子节点以 v 包装、属性按需返回（空 filter = 全量）。
//!   - 扩展侧消费方式（dbusMenu.js）：`GetLayout(0, -1, ['type',
//!     'children-display'])` 拿骨架；新条目用 `GetGroupProperties(ids, [])`
//!     拿全量属性；点击时 `Event(id, 'clicked', <'i' 0>, 0)`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;

/// 托盘菜单项。结构保持与 electron 版托盘菜单一致，
/// 由 main.rs 的 `build_tray_items` 构造；`label` 为 "---" 表示分隔线。
pub struct TrayItem {
    pub label: String,
    pub sensitive: bool,
    pub submenu: Option<Vec<TrayItem>>,
    pub action: Option<Box<dyn Fn()>>,
}

// ---------------------------------------------------------------------------
// D-Bus 接口描述（与扩展自带的 interfaces-xml 逐字段对齐）
// ---------------------------------------------------------------------------

const SNI_XML: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="OverlayIconName" type="s" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionMovieName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <method name="Activate">
      <arg type="i" name="x" direction="in"/>
      <arg type="i" name="y" direction="in"/>
    </method>
    <method name="SecondaryActivate">
      <arg type="i" name="x" direction="in"/>
      <arg type="i" name="y" direction="in"/>
    </method>
    <method name="ContextMenu">
      <arg type="i" name="x" direction="in"/>
      <arg type="i" name="y" direction="in"/>
    </method>
    <method name="Scroll">
      <arg type="i" name="delta" direction="in"/>
      <arg type="s" name="orientation" direction="in"/>
    </method>
    <signal name="NewIcon"/>
    <signal name="NewTitle"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus">
      <arg type="s" name="status"/>
    </signal>
  </interface>
</node>"#;

const DBUSMENU_XML: &str = r#"<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" name="parentId" direction="in"/>
      <arg type="i" name="recursionDepth" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="u" name="revision" direction="out"/>
      <arg type="(ia{sv}av)" name="layout" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="a(ia{sv})" name="properties" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="name" direction="in"/>
      <arg type="v" name="value" direction="out"/>
    </method>
    <method name="Event">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="eventId" direction="in"/>
      <arg type="v" name="data" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg type="a(isvu)" name="events" direction="in"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg type="i" name="id" direction="in"/>
      <arg type="b" name="needUpdate" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="ai" name="updatesNeeded" direction="out"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <signal name="LayoutUpdated">
      <arg type="u" name="revision"/>
      <arg type="i" name="parent"/>
    </signal>
    <signal name="ItemsPropertiesUpdated">
      <arg type="a(ia{sv})" name="updatedProps"/>
      <arg type="a(ias)" name="removedProps"/>
    </signal>
    <signal name="ItemActivationRequested">
      <arg type="i" name="id"/>
      <arg type="u" name="timestamp"/>
    </signal>
  </interface>
</node>"#;

// ---------------------------------------------------------------------------
// 内部状态
// ---------------------------------------------------------------------------

struct MenuNode {
    label: String,
    enabled: bool,
    is_separator: bool,
    children: Vec<i32>,
    action: Option<Rc<dyn Fn()>>,
}

struct Shared {
    /// SNI 的 Id / Title 属性
    title: String,
    icon_name: String,
    /// dbusmenu 对象路径（SNI 的 Menu 属性指向它）
    menu_path: String,
    /// id（含根节点 0）→ 节点
    items: HashMap<i32, MenuNode>,
    revision: u32,
    next_id: i32,
}

pub struct Tray {
    conn: gio::DBusConnection,
    shared: Rc<RefCell<Shared>>,
    // g_dbus_connection_register_object 会引用接口信息（introspection 与
    // Properties 分发都要用），必须与注册同生命周期。
    _sni_iface: gio::DBusInterfaceInfo,
    _menu_iface: gio::DBusInterfaceInfo,
}

// ---------------------------------------------------------------------------
// 公开 API（与旧版一致，main.rs 无需感知协议细节）
// ---------------------------------------------------------------------------

impl Tray {
    /// 创建托盘：注册两个 D-Bus 对象并向 StatusNotifierWatcher 报到。
    /// 任一步失败（无 session bus / 无 watcher 宿主）返回 None，调用方降级。
    pub fn new(title: &str, icon_name: &str) -> Option<Tray> {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;

        // ⚠️ D-Bus 对象路径的每段只允许 [A-Za-z0-9_]，**不含连字符**。
        //    应用 id "purr-pause-linux" 直接拼进路径会命中
        //    g_dbus_connection_register_object 的 assertion（实测：
        //    GLib-GIO-CRITICAL ... assertion 'object_path != NULL &&
        //    g_variant_is_object_path (object_path)' failed），
        //    且 gio-rs 的 build() 对这种「断言失败但未设 GError」的情况
        //    会静默返回 id=0 —— 图标无声消失。
        //    libappindicator 源码里的 clean_id 就是同款清洗。
        let clean = |s: &str| -> String {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
                .collect()
        };
        let item_path = format!("/org/ayatana/NotificationItem/{}", clean("purr-pause-linux"));
        let menu_path = format!("{item_path}/Menu");
        if !glib::Variant::is_object_path(&item_path) || !glib::Variant::is_object_path(&menu_path) {
            return None;
        }

        let shared = Rc::new(RefCell::new(Shared {
            title: title.to_string(),
            icon_name: icon_name.to_string(),
            menu_path: menu_path.clone(),
            // 根节点 id 恒为 0（libdbusmenu 约定）
            items: HashMap::from([(
                0,
                MenuNode {
                    label: String::new(),
                    enabled: false,
                    is_separator: false,
                    children: Vec::new(),
                    action: None,
                },
            )]),
            revision: 0,
            next_id: 1,
        }));

        let sni_node = gio::DBusNodeInfo::for_xml(SNI_XML).ok()?;
        let sni_iface = sni_node.lookup_interface("org.kde.StatusNotifierItem")?;

        let sh_props = shared.clone();
        conn.register_object(&item_path, &sni_iface)
            .method_call(
                move |_conn, _sender, _path, _iface, method, _params, invocation| {
                    // ItemIsMenu=true 时宿主直接展示菜单，这些方法一般不会被
                    // 调到；仍需逐个应答，否则调用方会挂起到超时。
                    invocation.return_value(Some(&no_reply()));
                    let _ = method;
                },
            )
            .property(move |_conn, _sender, _path, _iface, prop| {
                let sh = sh_props.borrow();
                sni_property(&sh, prop)
            })
            .build()
            .ok()?;

        let menu_node = gio::DBusNodeInfo::for_xml(DBUSMENU_XML).ok()?;
        let menu_iface = menu_node.lookup_interface("com.canonical.dbusmenu")?;

        let sh_menu = shared.clone();
        conn.register_object(&menu_path, &menu_iface)
            .method_call(
                move |_conn, _sender, _path, _iface, method, params, invocation| {
                    menu_method(&sh_menu, method, &params, invocation);
                },
            )
            .property(move |_conn, _sender, _path, _iface, prop| menu_property(prop))
            .build()
            .ok()?;

        // 向 watcher 报到。没有 watcher 宿主（GNOME 未装 AppIndicators 扩展）
        // 时这一步会报错 —— 此时托盘无从展示，按不可用处理。
        conn.call_sync(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
            "RegisterStatusNotifierItem",
            Some(&glib::Variant::tuple_from_iter([item_path.to_variant()])),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
        )
        .ok()?;

        Some(Tray {
            conn,
            shared,
            _sni_iface: sni_iface,
            _menu_iface: menu_iface,
        })
    }

    /// 重建菜单内容。
    ///
    /// 重新分配全部条目 id 并递增 revision，然后广播 `LayoutUpdated` ——
    /// 宿主（GNOME 扩展 / Plasma）收到后会重新拉取布局与属性。
    pub fn set_menu(&mut self, items: Vec<TrayItem>) {
        let (rev, menu_path) = {
            let mut sh = self.shared.borrow_mut();
            sh.items.clear();
            sh.items.insert(
                0,
                MenuNode {
                    label: String::new(),
                    enabled: false,
                    is_separator: false,
                    children: Vec::new(),
                    action: None,
                },
            );
            sh.next_id = 1;
            for it in items {
                flatten_item(it, &mut sh, 0);
            }
            sh.revision += 1;
            (sh.revision, sh.menu_path.clone())
        };

        let params = glib::Variant::tuple_from_iter([rev.to_variant(), 0i32.to_variant()]);
        let _ = self.conn.emit_signal(
            None,
            &menu_path,
            "com.canonical.dbusmenu",
            "LayoutUpdated",
            Some(&params),
        );
    }
    /// 只更新某个条目的 label，并广播 `ItemsPropertiesUpdated`。
    ///
    /// 为什么不直接 `set_menu` 重建：dbusmenu 的宿主（GNOME 扩展）是按
    /// 「布局树 + 条目 id」维护菜单模型的。每次重建都会换一套 id 并广播
    /// `LayoutUpdated`，宿主如果处理不干净就会错位 —— 实测用户看到的是
    /// **菜单项动作整体错开一位、末尾多出一个「退出」**。
    /// 所以结构固定、id 固定，只把会变的那一个 label 推过去。
    pub fn update_label(&mut self, id: i32, label: &str) {
        let (props, menu_path) = {
            let mut sh = self.shared.borrow_mut();
            let Some(node) = sh.items.get_mut(&id) else {
                return;
            };
            if node.label == label {
                return;
            }
            node.label = label.to_string();

            // a{sv} 的 value 必须包一层 v
            let entries = vec![glib::Variant::from_dict_entry(
                &"label".to_variant(),
                &glib::Variant::from_variant(&label.to_variant()),
            )];
            (array_of("{sv}", entries), sh.menu_path.clone())
        };

        let updated = array_of(
            "(ia{sv})",
            vec![glib::Variant::tuple_from_iter([id.to_variant(), props])],
        );
        let removed = array_of("(ias)", Vec::<glib::Variant>::new());
        let params = glib::Variant::tuple_from_iter([updated, removed]);
        let _ = self.conn.emit_signal(
            None,
            &menu_path,
            "com.canonical.dbusmenu",
            "ItemsPropertiesUpdated",
            Some(&params),
        );
    }
}

/// 把单个条目（及其子树）插入 items 表并挂到 parent 下。
///
/// 按值接管 TrayItem：action 是 `Box<dyn Fn()>`，不可克隆，
/// 而 set_menu 每次都是全新构建（所有权归这里正合适）。
fn flatten_item(item: TrayItem, sh: &mut Shared, parent_id: i32) {
    let is_sep = item.label.trim() == "---";
    let id = sh.next_id;
    sh.next_id += 1;
    let mut node = MenuNode {
        label: if is_sep { String::new() } else { item.label },
        enabled: item.sensitive && !is_sep,
        is_separator: is_sep,
        children: Vec::new(),
        action: None,
    };
    if let Some(sub) = item.submenu {
        for sub_item in sub {
            flatten_item(sub_item, sh, id);
        }
    }
    if let Some(a) = item.action {
        node.action = Some(Rc::new(a));
    }
    if let Some(p) = sh.items.get_mut(&parent_id) {
        p.children.push(id);
    }
    sh.items.insert(id, node);
}

// ---------------------------------------------------------------------------
// SNI：属性
// ---------------------------------------------------------------------------

fn sni_property(sh: &Shared, prop: &str) -> glib::Variant {
    match prop {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => sh.title.to_variant(),
        "Title" => sh.title.to_variant(),
        "Status" => "Active".to_variant(),
        "WindowId" => 0i32.to_variant(),
        "IconThemePath" => "".to_variant(),
        // Menu 属性类型是 o（对象路径），必须用 ObjectPath 构造出 "o" 类型
        "Menu" => glib::variant::ObjectPath::try_from(sh.menu_path.clone())
            .expect("菜单对象路径合法")
            .to_variant(),
        // true：宿主点击图标直接展示菜单（与 electron 托盘行为一致）
        "ItemIsMenu" => true.to_variant(),
        "IconName" => sh.icon_name.to_variant(),
        "IconPixmap" | "AttentionIconPixmap" => empty_pixmaps(),
        "OverlayIconName" | "AttentionIconName" | "AttentionMovieName" => "".to_variant(),
        "ToolTip" => glib::Variant::tuple_from_iter([
            sh.icon_name.to_variant(),
            empty_pixmaps(),
            sh.title.to_variant(),
            "胖猫暂停一下：点开菜单可暂停监控、打开设置".to_variant(),
        ]),
        _ => "".to_variant(),
    }
}

/// 空 IconPixmap：`a(iiay)`（宽, 高, ARGB 字节）
fn empty_pixmaps() -> glib::Variant {
    array_of("(iiay)", Vec::new())
}

// ---------------------------------------------------------------------------
// dbusmenu：属性
// ---------------------------------------------------------------------------

fn menu_property(prop: &str) -> glib::Variant {
    match prop {
        "Version" => 3u32.to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "Status" => "normal".to_variant(),
        "IconThemePath" => array_of("s", Vec::<glib::Variant>::new()),
        _ => "".to_variant(),
    }
}

// ---------------------------------------------------------------------------
// dbusmenu：方法分发
// ---------------------------------------------------------------------------

fn menu_method(
    sh: &Rc<RefCell<Shared>>,
    method: &str,
    params: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
) {
    match method {
        "GetLayout" => {
            let parent = params.child_value(0).get::<i32>().unwrap_or(0);
            let depth = params.child_value(1).get::<i32>().unwrap_or(-1);
            let filter = str_filter(&params.child_value(2));
            let sh = sh.borrow();
            if !sh.items.contains_key(&parent) {
                drop(sh);
                invocation.return_dbus_error(
                    "com.canonical.dbusmenu.Error.UnknownParent",
                    "unknown parent id",
                );
                return;
            }
            let rev = sh.revision;
            let node = layout_node(&sh, parent, depth, filter.as_deref());
            drop(sh);
            invocation
                .return_value(Some(&glib::Variant::tuple_from_iter([rev.to_variant(), node])));
        }
        "GetGroupProperties" => {
            let ids = i32_array(&params.child_value(0));
            let filter = str_filter(&params.child_value(1));
            let sh = sh.borrow();
            let tuples: Vec<glib::Variant> = ids
                .iter()
                .filter_map(|id| {
                    sh.items.get(id).map(|it| {
                        glib::Variant::tuple_from_iter([
                            id.to_variant(),
                            item_props(it, filter.as_deref()),
                        ])
                    })
                })
                .collect();
            drop(sh);
            let arr = array_of("(ia{sv})", tuples);
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([arr])));
        }
        "GetProperty" => {
            let id = params.child_value(0).get::<i32>().unwrap_or(-1);
            let name = params.child_value(1).str().unwrap_or("").to_string();
            let sh = sh.borrow();
            let value = sh
                .items
                .get(&id)
                .map(|it| item_props(it, None))
                .and_then(|dict| dict_lookup(&dict, &name))
                // 字典条目里存的就是包好的 v，缺属性时补一个空串的 v
                .unwrap_or_else(|| glib::Variant::from_variant(&"".to_variant()));
            drop(sh);
            // 返回类型 (v)：value 本身已是 v，直接放入元组
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([value])));
        }
        "Event" => {
            let id = params.child_value(0).get::<i32>().unwrap_or(-1);
            let event = params.child_value(1).str().unwrap_or("").to_string();
            let action = sh.borrow().items.get(&id).and_then(|n| n.action.clone());
            // 始终打印：托盘菜单是唯一入口，出问题时必须能看出
            // 「宿主没把点击发过来」还是「发了但动作没执行」。
            // 只在用户点击时输出，不会刷屏。
            eprintln!(
                "[purr-pause-linux] 托盘点击: id={id} event={event} 有动作={}",
                action.is_some()
            );
            // 先应答再执行动作：动作里可能开窗口（重入主循环）
            invocation.return_value(Some(&no_reply()));
            if event == "clicked" {
                if let Some(a) = action {
                    // ⚠️ 延后到下一轮主循环再执行。
                    //    动作里通常会 refresh_tray → set_menu → emit LayoutUpdated，
                    //    在当前 D-Bus 方法调用栈里做这些容易踩重入，
                    //    实测出现过「点了没反应」。
                    glib::idle_add_local_once(move || a());
                }
            }
        }
        "EventGroup" => {
            let events = params.child_value(0);
            let mut errors: Vec<glib::Variant> = Vec::new();
            let mut actions: Vec<Rc<dyn Fn()>> = Vec::new();
            {
                let sh = sh.borrow();
                for i in 0..events.n_children() {
                    let ev = events.child_value(i);
                    let id = ev.child_value(0).get::<i32>().unwrap_or(-1);
                    let event = ev.child_value(1).str().unwrap_or("").to_string();
                    match sh.items.get(&id).and_then(|n| n.action.clone()) {
                        Some(a) if event == "clicked" => actions.push(a),
                        Some(_) => {}
                        None => errors.push(id.to_variant()),
                    }
                }
            }
            invocation
                .return_value(Some(&glib::Variant::tuple_from_iter([array_of("i", errors)])));
            for a in actions {
                glib::idle_add_local_once(move || a());
            }
        }
        "AboutToShow" => {
            // ⚠️ 必须返回 true（needUpdate）。
            //
            // 返回 false 的意思是「我的布局是最新的，别来取」—— 宿主于是用它
            // 自己缓存的那份。而我们的 set_menu 之后宿主并不一定老实处理
            // LayoutUpdated，结果就是：用户点了「暂停 30 分钟」，程序确实暂停了
            // （日志里 `已暂停监控 30 分钟` 有打），但菜单看起来毫无变化。
            //
            // 每次打开菜单重新拉一次布局只有几次 D-Bus 往返，代价极低，
            // 换来的是「菜单永远显示真实状态」。
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([true.to_variant()])));
        }
        "AboutToShowGroup" => {
            let empty = array_of("i", Vec::<glib::Variant>::new());
            invocation
                .return_value(Some(&glib::Variant::tuple_from_iter([empty.clone(), empty])));
        }
        _ => {
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                "unknown dbusmenu method",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 布局与属性构造
// ---------------------------------------------------------------------------

/// 递归构造 `(ia{sv}av)` 节点。
fn layout_node(sh: &Shared, id: i32, depth: i32, filter: Option<&[String]>) -> glib::Variant {
    let (props, children) = match sh.items.get(&id) {
        Some(it) => (item_props(it, filter), it.children.clone()),
        None => (dict_of(vec![]), Vec::new()),
    };
    // depth 语义（dbusmenu 规范）：-1 = 全部递归；0 = 不含子节点；n = 递归 n 层。
    // 非负 depth 逐层递减，叶子节点自然终止。
    let child_variants: Vec<glib::Variant> = if depth == 0 {
        Vec::new()
    } else {
        children
            .iter()
            .map(|c| glib::Variant::from_variant(&layout_node(sh, *c, depth - 1, filter)))
            .collect()
    };
    glib::Variant::tuple_from_iter([id.to_variant(), props, array_of("v", child_variants)])
}

/// 节点属性字典 `a{sv}`。filter 为 None（空 propertyNames）时返回全量。
fn item_props(item: &MenuNode, filter: Option<&[String]>) -> glib::Variant {
    let mut entries: Vec<glib::Variant> = Vec::new();
    let mut push = |name: &str, value: glib::Variant| {
        let wanted = match filter {
            Some(names) => names.iter().any(|n| n == name),
            None => true,
        };
        if wanted {
            // ⚠️ a{sv} 的 v 表示 value 必须是**包了一层的 variant**。
            //    from_dict_entry 不会自动包装 —— 直接传裸值会得到 {ss} 之类
            //    的错型条目，array_from_iter_with_type 断言失败（实测核心转储）。
            entries.push(glib::Variant::from_dict_entry(
                &name.to_variant(),
                &glib::Variant::from_variant(&value),
            ));
        }
    };
    if item.is_separator {
        push("type", "separator".to_variant());
    } else {
        // 规范约定：只返回非默认值。enabled 默认 true，false 才需要下发。
        if !item.label.is_empty() {
            push("label", item.label.to_variant());
        }
        if !item.enabled {
            push("enabled", false.to_variant());
        }
        if !item.children.is_empty() {
            push("children-display", "submenu".to_variant());
        }
    }
    dict_of(entries)
}

fn dict_of(entries: Vec<glib::Variant>) -> glib::Variant {
    array_of("{sv}", entries)
}

/// 构造数组。注意 gio 的 `array_from_iter_with_type` 的参数是**元素类型**
/// （内部会对它做 as_array），所以传 "{sv}" 得到的是 "a{sv}"。
fn array_of(elem_type: &str, children: Vec<glib::Variant>) -> glib::Variant {
    let ty = glib::VariantTy::new(elem_type).expect("合法的 Variant 类型串");
    glib::Variant::array_from_iter_with_type(&ty, children)
}

/// 无返回值方法的标准应答（空元组 "()"）。
fn no_reply() -> glib::Variant {
    glib::Variant::tuple_from_iter(Vec::<glib::Variant>::new())
}

/// 在 `a{sv}` 字典里按 key 查找（glib 0.22 的 Variant 没有 lookup_value）。
fn dict_lookup(dict: &glib::Variant, name: &str) -> Option<glib::Variant> {
    (0..dict.n_children())
        .map(|i| dict.child_value(i))
        .find(|e| e.child_value(0).str() == Some(name))
        .map(|e| e.child_value(1))
}

/// 解析 propertyNames（"as"）：空数组 → None（表示全量）。
fn str_filter(v: &glib::Variant) -> Option<Vec<String>> {
    if v.n_children() == 0 {
        return None;
    }
    Some(
        (0..v.n_children())
            .filter_map(|i| v.child_value(i).get::<String>())
            .collect(),
    )
}

fn i32_array(v: &glib::Variant) -> Vec<i32> {
    (0..v.n_children())
        .filter_map(|i| v.child_value(i).get::<i32>())
        .collect()
}

// ---------------------------------------------------------------------------

pub fn tray_fallback_hint() -> String {
    "当前桌面环境没有可用的系统托盘宿主\n（例如 GNOME 需安装 gnome-shell-extension-appindicator）。\n应用将无法通过托盘菜单访问设置。"
        .to_string()
}

//! 待办消息列表窗口：彩色来源标签、点击跳转并本地清理。
//!
//! 待办消息列表：
//!   - 按创建时间倒序展示所有启用源的待办
//!   - 每条带来源彩色标签
//!   - 点击条目用系统默认浏览器打开 targetUrl，成功后本地清理（角标 -1）
//!   - 仅允许 http(s) 链接

// adw 的方法（AdwWindow::set_content 等）来自 libadwaita 自己的 Ext trait，
// gtk::prelude 里没有，必须单独引入。
use adw::prelude::*;
// gio 是 gtk4 的依赖，须经 gtk 路径引入（Rust 2018 起不能用裸 crate 名）
use gtk::gio;
use std::cell::RefCell;
use std::rc::Rc;

use crate::message::{self, AggregatedState, TodoItem};
use crate::AppState;

/// 来源调色板（与 JS 版一致）。
const SOURCE_PALETTE: [&str; 6] = ["#2f80ed", "#27ae60", "#f2994a", "#9b51e0", "#eb5757", "#00a7a7"];

/// 按源 id 哈希取颜色。
fn source_color(source_id: &str) -> &'static str {
    let mut hash: i32 = 0;
    for c in source_id.chars() {
        hash = hash.wrapping_shl(5).wrapping_sub(hash).wrapping_add(c as i32);
    }
    let idx = hash.unsigned_abs() as usize % SOURCE_PALETTE.len();
    SOURCE_PALETTE[idx]
}

/// 打开待办消息窗口。state 用于点击后回写清理结果与角标。
pub fn present(state_snapshot: AggregatedState, app_state: Rc<RefCell<AppState>>) {
    let window = adw::Window::builder()
        .title("待办消息")
        .default_width(560)
        .default_height(620)
        .build();

    let header = adw::HeaderBar::new();
    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("刷新"));
    header.pack_end(&refresh_btn);
    window.set_titlebar(Some(&header));

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // 状态提示条
    let status_bar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    status_bar.set_margin_top(8);
    status_bar.set_margin_bottom(4);
    status_bar.set_margin_start(12);
    status_bar.set_margin_end(12);
    let status_label = gtk::Label::new(Some(""));
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    status_bar.append(&status_label);
    outer.append(&status_bar);

    // 列表
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    scroller.set_child(Some(&list));
    outer.append(&scroller);

    window.set_child(Some(&outer));

    // render 闭包会 move 走 app_state / window，
    // 因此先各克隆一份给它用，外层继续持有原件（否则后面 refresh 用到已移动的值）。
    let app_state_c = app_state.clone();
    let window_c = window.clone();

    // 渲染一份快照
    let render = {
        let list = list.clone();
        let status = status_label.clone();
        move |snap: &AggregatedState| {
            // 清空旧条目
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            status.set_text(&status_text(snap));

            if !snap.enabled {
                add_placeholder(&list, "消息提醒未启用\n请在托盘菜单中开启");
                return;
            }
            if snap.sources.iter().all(|s| !s.enabled) {
                add_placeholder(&list, "尚未启用任何接口源\n请在配置文件的 messageSources 中添加并启用");
                return;
            }
            if snap.items.is_empty() {
                add_placeholder(&list, "没有待办消息");
                return;
            }

            for item in &snap.items {
                let row = build_row(item, &app_state_c, &window_c);
                list.append(&row);
            }
        }
    };

    render(&state_snapshot);

    // 刷新：重新拉取一次
    let st_refresh = app_state.clone();
    let render_refresh = render;
    let win_refresh = window.clone();
    refresh_btn.connect_clicked(move |_| {
        let snapshot = {
            let s = st_refresh.borrow();
            message::aggregate(&s.sources, s.config.message_notify_enabled)
        };
        render_refresh(&snapshot);
        win_refresh.present();
    });

    window.present();
}

fn status_text(snap: &AggregatedState) -> String {
    if !snap.enabled {
        return "消息提醒已关闭".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("共 {} 条待办", snap.badge_count));
    if snap.backoff {
        parts.push("部分接口连续失败，已降低轮询频率".to_string());
    }
    if !snap.last_error.is_empty() {
        parts.push(snap.last_error.clone());
    }
    parts.join("　·　")
}

fn add_placeholder(list: &gtk::ListBox, text: &str) {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_margin_top(24);
    label.set_margin_bottom(24);
    label.add_css_class("dim-label");
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&label));
    row.set_activatable(false);
    list.append(&row);
}

/// 构建单条待办行。
fn build_row(
    item: &TodoItem,
    app_state: &Rc<RefCell<AppState>>,
    window: &adw::Window,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("message-row");

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    box_.set_margin_top(8);
    box_.set_margin_bottom(8);
    box_.set_margin_start(12);
    box_.set_margin_end(12);

    // 标题 + 来源标签
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(&item.title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    head.append(&title);

    let src_label = gtk::Label::new(Some(&item.origin_id));
    src_label.add_css_class("caption");
    src_label.add_css_class("source-chip");
    src_label.set_valign(gtk::Align::Center);
    // 用内联 CSS 上色（Adw 未提供 set_color）
    let css_provider = gtk::CssProvider::new();
    css_provider.load_from_data(&format!(
        "label.source-chip {{ color: {}; font-weight: 500; }}",
        source_color(&item.origin_id)
    ));
    // 注意：provider 需要被持有，label_add_provider 内部会leak 一份
    label_add_provider(&src_label, &css_provider);
    head.append(&src_label);
    box_.append(&head);

    // 摘要
    if !item.summary.is_empty() {
        let summary = gtk::Label::new(Some(&item.summary));
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        summary.set_max_width_chars(60);
        summary.add_css_class("dim-label");
        box_.append(&summary);
    }

    // 时间
    if !item.created_at.is_empty() {
        let time = gtk::Label::new(Some(&format_time(&item.created_at)));
        time.set_xalign(0.0);
        time.add_css_class("caption");
        time.add_css_class("dim-label");
        box_.append(&time);
    }

    row.set_child(Some(&box_));

    // 点击 → 打开浏览器并本地清理
    let item_id = item.id.clone();
    let origin = item.origin_id.clone();
    let st = app_state.clone();
    let win = window.clone();
    let win_parent = window.clone();
    // 注意：gtk 的行激活信号叫 connect_activate（无 d）；
    // connect_activated（有 d）是 libadwaita 的 ActionRow 专用信号，
    // ListBoxRow 不是 ActionRow，加了 adw::prelude 后会「找到方法但不满足 trait bound」。
    row.connect_activate(move |_| {
        let url = item_id_target(&st, &origin, &item_id);
        if !message::is_http_url(&url) {
            return;
        }
        // 用 GTK4 的 UriLauncher 调起系统默认处理器。
        // （早期版本用的 gio::AppLauncher 在 gio 0.22 中已不存在，会编译失败。）
        let launcher = gtk::UriLauncher::new(&url);
        let st2 = st.clone();
        let win2 = win.clone();
        // 外层是 Fn 闭包（connect_activate），不能把捕获的 String move 出去，
        // 故为内层 FnOnce 闭包各克隆一份（同名遮蔽）。
        let item_id = item_id.clone();
        let origin = origin.clone();
        launcher.launch(Some(&win_parent), gio::Cancellable::NONE, move |res| {
            if res.is_err() {
                // 打开失败不做本地清理，保留待办供用户重试
                st2.borrow()
                    .log
                    .warn(&format!("打开待办链接失败 id={item_id}"));
                return;
            }
            // 打开成功后本地清理
            let mut s = st2.borrow_mut();
            if let Some(rt) = s.sources.iter_mut().find(|r| r.origin_id() == origin) {
                if message::clear_item(rt, &item_id) {
                    s.badge_count = s
                        .sources
                        .iter()
                        .filter(|r| r.enabled)
                        .map(|r| r.badge_count)
                        .sum();
                    s.log
                        .info(&format!("本地清理待办 id={item_id}"));
                }
            }
            drop(s);
            win2.close();
        });
    });

    row
}

/// 从当前快照里取该条目的 targetUrl。
fn item_id_target(state: &Rc<RefCell<AppState>>, origin: &str, id: &str) -> String {
    let s = state.borrow();
    s.sources
        .iter()
        .find(|r| r.origin_id() == origin)
        .and_then(|r| r.snapshot.iter().find(|i| i.id == id))
        .map(|i| i.target_url.clone())
        .unwrap_or_default()
}

/// 把 ISO 时间串格式化为易读形式（本地时区）。
fn format_time(iso: &str) -> String {
    if iso.len() < 16 {
        return iso.to_string();
    }
    // 简单截断到分钟，去掉秒与时区偏移
    let date = &iso[0..10];
    let hm = &iso[11..16];
    format!("{date} {hm}")
}

// 附加 CssProvider（来源标签着色）。
//
// GTK4 里 `Widget::style_context()` 已被废弃，正确做法是把 provider
// 挂到 display 上（全局生效）。provider 需保持存活，否则样式丢失 ——
// 这里用 Box::leak 让它随进程存活。
fn label_add_provider(_label: &gtk::Label, provider: &gtk::CssProvider) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let leaked: &'static gtk::CssProvider = Box::leak(Box::new(provider.clone()));
    gtk::style_context_add_provider_for_display(
        &display,
        leaked,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
//! 自实现的目录选择器。
//!
//! ## 为什么不用 GTK 自带的
//!
//! 用户实测（Ubuntu + GNOME）：点「浏览」报 **「无法显示文件夹内容」**，
//! 终端里伴随 `GtkDialog mapped without a transient parent`。
//! `GtkFileChooserNative` 在没有 xdg-desktop-portal / gvfs 的环境里会退回
//! GTK 内置对话框，而内置对话框依赖 GIO 的虚拟文件系统去列目录 ——
//! 那条链路一断就什么也列不出来。属于环境问题，改参数救不回来。
//!
//! ## 为什么用普通 GtkWindow + 底部按钮，而不是 AdwWindow + HeaderBar
//!
//! v1.4.0 第一版用了 `adw::Window` + 自绘 `HeaderBar`，用户反馈
//! **「窗口弹出来了但关不掉」**。为了不再依赖自绘标题栏里的按钮，
//! 这里改用普通 `gtk::Window`：**窗口管理器会自带标题栏和关闭按钮**，
//! 用户永远有一个系统级的「×」可以关掉它；底部再放一组按钮兜底。
//!
//! 列表只做「目录」：`std::fs::read_dir` 逐级进入，不碰 GIO / portal / 缩略图。

use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

thread_local! {
    /// 当前打开的目录选择窗口（强引用持有 + 单例）。
    static OPEN: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// 弹出目录选择器。用户确认时回调 `on_pick(选中的目录)`。
pub fn pick<F>(parent: &adw::Window, start: Option<PathBuf>, on_pick: F)
where
    F: Fn(PathBuf) + 'static,
{
    if let Some(w) = OPEN.with(|s| s.borrow().clone()) {
        w.present();
        return;
    }

    // 起始目录：传入的（存在的话）→ 家目录 → 根目录
    let start_dir = start
        .filter(|p| p.is_dir())
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));

    // 普通 GtkWindow：交给窗口管理器画标题栏（含关闭按钮）。
    // ⚠️ 不要 modal(true)：模态会抓走输入，一旦这个窗口本身出问题，
    //    后面的设置窗口就整片点不动（用户反馈「设置窗口无法关闭」多半是这个）。
    let window = gtk::Window::builder()
        .title("选择素材目录")
        .transient_for(parent)
        .resizable(true)
        .default_width(520)
        .default_height(440)
        .build();

    // 当前目录 + 上级 / 主目录
    let path_label = gtk::Label::new(None);
    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    path_label.set_hexpand(true);
    path_label.add_css_class("dim-label");
    path_label.set_selectable(true);

    let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
    up_btn.set_tooltip_text(Some("上一级"));
    let home_btn = gtk::Button::with_label("主目录");

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.set_margin_start(12);
    top.set_margin_end(12);
    top.set_margin_top(10);
    top.set_margin_bottom(6);
    top.append(&up_btn);
    top.append(&path_label);
    top.append(&home_btn);

    // 子目录列表
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&list));
    scrolled.set_vexpand(true);
    scrolled.set_margin_start(12);
    scrolled.set_margin_end(12);

    // 底部按钮
    let cancel_btn = gtk::Button::with_label("取消");
    let ok_btn = gtk::Button::with_label("选择此目录");
    ok_btn.add_css_class("suggested-action");
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bottom.set_halign(gtk::Align::End);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);
    bottom.set_margin_top(10);
    bottom.set_margin_bottom(12);
    bottom.append(&cancel_btn);
    bottom.append(&ok_btn);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&top);
    content.append(&scrolled);
    content.append(&bottom);
    window.set_child(Some(&content));

    let current: Rc<RefCell<PathBuf>> = Rc::new(RefCell::new(start_dir));
    refill(&list, &path_label, &current);

    // 上级 / 主目录
    {
        let cur = current.clone();
        let list2 = list.clone();
        let label2 = path_label.clone();
        up_btn.connect_clicked(move |_| {
            if let Some(p) = cur.borrow().parent().map(|p| p.to_path_buf()) {
                *cur.borrow_mut() = p;
                refill(&list2, &label2, &cur);
            }
        });
    }
    {
        let cur = current.clone();
        let list2 = list.clone();
        let label2 = path_label.clone();
        home_btn.connect_clicked(move |_| {
            if let Some(h) = dirs::home_dir() {
                *cur.borrow_mut() = h;
                refill(&list2, &label2, &cur);
            }
        });
    }

    // 取消：close() 之后再加一道 destroy 兜底。
    // 上一版出现过「窗口关不掉」，这里不赌 close_request 一定生效。
    {
        let win = window.clone();
        cancel_btn.connect_clicked(move |_| close_now(&win));
    }

    let on_pick = Rc::new(on_pick);
    {
        let cur = current.clone();
        let win = window.clone();
        ok_btn.connect_clicked(move |_| {
            let chosen = cur.borrow().clone();
            (on_pick)(chosen);
            close_now(&win);
        });
    }

    window.connect_destroy(|_| {
        OPEN.with(|s| *s.borrow_mut() = None);
    });
    OPEN.with(|s| *s.borrow_mut() = Some(window.clone()));

    window.present();
}

/// 关窗：直接 destroy()。
///
/// 不用 close() —— 它依赖 close_request 信号与窗口管理器协作，
/// 实测在这台机器上「点了没反应」。destroy() 无条件拆掉顶层窗口。
fn close_now(win: &gtk::Window) {
    win.destroy();
}

/// 重建列表内容（进入子目录 / 返回上级都走这里）。
fn refill(list: &gtk::ListBox, path_label: &gtk::Label, current: &Rc<RefCell<PathBuf>>) {
    let dir = current.borrow().clone();
    path_label.set_text(&dir.display().to_string());

    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(e) => {
            let row = adw::ActionRow::new();
            row.set_title("无法读取该目录");
            row.set_subtitle(&e.to_string());
            list.append(&row);
            return;
        }
    };

    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        // 隐藏目录（.config 之类）不显示，减少噪音
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| !n.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();
    dirs.sort();

    if dirs.is_empty() {
        let row = adw::ActionRow::new();
        row.set_title("（没有子目录）");
        row.set_subtitle("可以直接点右下角「选择此目录」");
        list.append(&row);
        return;
    }

    for d in dirs {
        let name = d
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let row = adw::ActionRow::new();
        row.set_title(&name);
        row.set_activatable(true);
        row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

        let cur2 = current.clone();
        let list2 = list.clone();
        let label2 = path_label.clone();
        row.connect_activated(move |_| {
            let target = d.clone();
            *cur2.borrow_mut() = target;
            // ⚠️ 延后一拍再重建：这一行马上要被 remove 掉，
            //    在它自己的 activated 回调里直接删自己容易踩 GTK 重入。
            let list3 = list2.clone();
            let label3 = label2.clone();
            let cur3 = cur2.clone();
            glib::idle_add_local_once(move || refill(&list3, &label3, &cur3));
        });
        list.append(&row);
    }
}

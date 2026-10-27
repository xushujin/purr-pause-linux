//! 自实现的目录选择器。
//!
//! ## 为什么不用 GTK 自带的
//!
//! 用户实测（Ubuntu + GNOME）：点「浏览」报 **「无法显示文件夹内容」**，
//! 并且终端里伴随两条
//! ```text
//! Gtk-Message: GtkDialog mapped without a transient parent. This is discouraged.
//! Cannot load libcuda.so.1
//! ```
//! `GtkFileChooserNative` 在没有 xdg-desktop-portal / gvfs 的环境里会退回到
//! GTK 内置对话框，而内置对话框依赖 GIO 的虚拟文件系统去列目录 ——
//! 那条链路一断就什么也列不出来，属于环境问题，改参数救不回来。
//!
//! 所以这里只做**目录**选择这一件事：`std::fs::read_dir` 逐级进入，
//! 不碰 GIO、不碰 portal、不碰缩略图，任何桌面环境下都能用。

use adw::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    /// 当前打开的目录选择窗口（强引用持有 + 单例，理由同 settings.rs）。
    static OPEN: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
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

    let window = adw::Window::builder()
        .title("选择素材目录")
        .modal(true)
        .resizable(true)
        .default_width(520)
        .default_height(420)
        .build();
    window.set_transient_for(Some(parent));

    let header = adw::HeaderBar::new();
    let cancel_btn = gtk::Button::with_label("取消");
    let ok_btn = gtk::Button::with_label("选择此目录");
    ok_btn.add_css_class("suggested-action");
    header.pack_start(&cancel_btn);
    header.pack_end(&ok_btn);

    // 当前目录显示 + 上级按钮
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
    list.set_margin_start(12);
    list.set_margin_end(12);
    list.set_margin_bottom(12);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&list));
    scrolled.set_vexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&top);
    content.append(&scrolled);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&content);
    window.set_content(Some(&root));

    // 当前选中的目录
    let current: std::rc::Rc<RefCell<PathBuf>> = std::rc::Rc::new(RefCell::new(start_dir));

    // 首次填充
    refill(&list, &path_label, &current);

    // 上级 / 主目录 / 取消 / 确定
    {
        let cur = current.clone();
        let list2 = list.clone();
        let label2 = path_label.clone();
        up_btn.connect_clicked(move |_| {
            let parent_dir = cur.borrow().parent().map(|p| p.to_path_buf());
            if let Some(p) = parent_dir {
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

    let win_cancel = window.clone();
    cancel_btn.connect_clicked(move |_| win_cancel.close());

    let on_pick = std::rc::Rc::new(on_pick);
    {
        let cur = current.clone();
        let win = window.clone();
        ok_btn.connect_clicked(move |_| {
            let chosen = cur.borrow().clone();
            (on_pick)(chosen);
            win.close();
        });
    }

    window.connect_destroy(|_| {
        OPEN.with(|s| *s.borrow_mut() = None);
    });
    OPEN.with(|s| *s.borrow_mut() = Some(window.clone()));

    window.present();
}

/// 重建列表内容（进入子目录 / 返回上级都走这里）。
///
/// 单独抽成函数是为了避免「闭包需要调用自身」的 Rc 自引用问题。
fn refill(
    list: &gtk::ListBox,
    path_label: &gtk::Label,
    current: &std::rc::Rc<RefCell<PathBuf>>,
) {
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
        row.set_subtitle("可以直接点右上角「选择此目录」");
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
            *cur2.borrow_mut() = d.clone();
            refill(&list2, &label2, &cur2);
        });
        list.append(&row);
    }
}

/// 把 `~` 展开成家目录（设置里允许用户手填 `~/webm` 这种路径）。
pub fn expand_tilde(input: &str) -> PathBuf {
    let s = input.trim();
    if s.is_empty() {
        return PathBuf::new();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    Path::new(s).to_path_buf()
}

//! 设置窗口：阈值、休息时长、动画模式、素材目录、自启、日志。

// adw 的方法（PreferencesPage::add / ActionRow::set_title 等）来自
// libadwaita 自己的 Ext trait，gtk::prelude 里没有，必须单独引入。
use adw::prelude::*;
// gio 是 gtk4 的依赖，须经 gtk 路径引入（Rust 2018 起不能用裸 crate 名）
use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::core::config::Config;

/// 打开设置窗口。回调在用户点击「保存」或关闭窗口时触发。
pub fn present<F>(current: Config, activated: bool, idle_method: String, on_save: F)
where
    F: Fn(Config) + 'static,
{
    let window = adw::Window::builder()
        .title("胖猫暂停一下（PurrPause Linux） 设置")
        .modal(false)
        .resizable(true)
        .default_width(460)
        .build();

    let header = adw::HeaderBar::new();
    let save_btn = gtk::Button::with_label("保存");
    save_btn.add_css_class("suggested-action");
    let cancel_btn = gtk::Button::with_label("关闭");
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);
    window.set_titlebar(Some(&header));

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    group.set_title("计时规则");
    page.add(&group);

    // 屏幕使用时间
    let threshold_row = adw::ActionRow::new();
    threshold_row.set_title("屏幕使用时间");
    threshold_row.set_subtitle("连续使用多久后提醒（1-480 分钟）");
    let threshold_adj = gtk::Adjustment::new(
        current.threshold_minutes as f64,
        1.0,
        480.0,
        1.0,
        10.0,
        0.0,
    );
    let threshold_spin = gtk::SpinButton::new(Some(&threshold_adj), 1.0, 0.0);
    threshold_spin.set_valign(gtk::Align::Center);
    threshold_row.add_suffix(&threshold_spin);
    group.add(&threshold_row);

    // 休息时长
    let break_row = adw::ActionRow::new();
    break_row.set_title("休息时长");
    break_row.set_subtitle("每次休息持续多久（1-60 分钟）");
    let break_adj =
        gtk::Adjustment::new(current.break_minutes as f64, 1.0, 60.0, 1.0, 5.0, 0.0);
    let break_spin = gtk::SpinButton::new(Some(&break_adj), 1.0, 0.0);
    break_spin.set_valign(gtk::Align::Center);
    break_row.add_suffix(&break_spin);
    group.add(&break_row);

    // 动画模式
    let mode_group = adw::PreferencesGroup::new();
    mode_group.set_title("猫咪动画");
    page.add(&mode_group);

    let mode_row = adw::ActionRow::new();
    mode_row.set_title("动画模式");
    mode_row.set_subtitle(if activated {
        "选择猫咪出现的方式"
    } else {
        "切换动画模式需要激活"
    });
    let modes = ["中央走出（默认）", "右下角走出（放大）", "中央淡入", "右侧走出"];
    let current_mode = match current.animation_mode.as_str() {
        "walk-zoom" => 1,
        "fade-center" => 2,
        "walk-flat" => 3,
        _ => 0,
    };
    let mode_drop = gtk::DropDown::from_strings(&modes);
    mode_drop.set_selected(current_mode as u32);
    mode_drop.set_valign(gtk::Align::Center);
    mode_drop.set_sensitive(activated);
    mode_row.add_suffix(&mode_drop);
    mode_group.add(&mode_row);

    // 素材目录
    let asset_group = adw::PreferencesGroup::new();
    asset_group.set_title("素材");
    page.add(&asset_group);

    let asset_row = adw::ActionRow::new();
    asset_row.set_title("素材目录");
    asset_row.set_subtitle(if current.custom_webm_dir.is_empty() {
        "使用内置素材".to_string()
    } else {
        current.custom_webm_dir.clone()
    });
    let pick_btn = gtk::Button::with_label("选择…");
    pick_btn.set_valign(gtk::Align::Center);
    pick_btn.set_sensitive(activated);
    let clear_btn = gtk::Button::with_label("清除");
    clear_btn.set_valign(gtk::Align::Center);
    clear_btn.set_sensitive(activated && !current.custom_webm_dir.is_empty());
    asset_row.add_suffix(&pick_btn);
    asset_row.add_suffix(&clear_btn);
    asset_group.add(&asset_row);

    let dir_holder: Rc<RefCell<PathBuf>> = Rc::new(RefCell::new(PathBuf::new()));
    let subtitle_widget = asset_row.clone();
    let pick2 = pick_btn.clone();
    pick_btn.connect_clicked(move |_| {
        let dlg = gtk::FileDialog::new();
        let win = pick2
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok());
        let h = dir_holder.clone();
        let sub = subtitle_widget.clone();
        // gtk4 0.11 的回调签名是 Result<gio::File, glib::Error>
        // （早期版本曾用 gtk::FileDialogError，该类型并不存在）
        let done = move |result: Result<gtk::gio::File, gtk::glib::Error>| {
            if let Ok(f) = result {
                if let Some(path) = f.path() {
                    *h.borrow_mut() = path.clone();
                    sub.set_subtitle(&path.to_string_lossy());
                }
            }
        };
        if let Some(w) = win {
            dlg.select_folder(Some(&w), gio::Cancellable::NONE, done);
        }
    });
    let clear2 = clear_btn.clone();
    let dh2 = dir_holder.clone();
    clear_btn.connect_clicked(move |_| {
        dh2.borrow_mut().clear();
        clear2.set_sensitive(false);
    });

    // 系统
    let sys_group = adw::PreferencesGroup::new();
    sys_group.set_title("系统");
    page.add(&sys_group);

    let autostart_row = adw::ActionRow::new();
    autostart_row.set_title("开机自动启动");
    autostart_row.set_subtitle("系统启动时自动运行");
    let autostart_switch = gtk::Switch::new();
    autostart_switch.set_valign(gtk::Align::Center);
    autostart_switch.set_active(crate::core::autostart::is_enabled());
    autostart_row.add_suffix(&autostart_switch);
    sys_group.add(&autostart_row);

    let log_row = adw::ActionRow::new();
    log_row.set_title("写入日志");
    log_row.set_subtitle("记录计时、激活与消息提醒过程");
    let log_switch = gtk::Switch::new();
    log_switch.set_valign(gtk::Align::Center);
    log_switch.set_active(current.log_enabled);
    log_row.add_suffix(&log_switch);
    sys_group.add(&log_row);

    let idle_row = adw::ActionRow::new();
    idle_row.set_title("空闲检测方式");
    idle_row.set_subtitle(&idle_method);
    sys_group.add(&idle_row);

    let about = adw::PreferencesGroup::new();
    let about_row = adw::ActionRow::new();
    about_row.set_title("版本");
    about_row.set_subtitle(&format!("v{}", env!("CARGO_PKG_VERSION")));
    about.add(&about_row);
    page.add(&about);

    window.set_content(Some(&page));

    // 保存
    let cfg_cell: Rc<RefCell<Config>> = Rc::new(RefCell::new(current.clone()));
    let save2 = save_btn.clone();
    let win2 = window.clone();
    let dir2 = dir_holder.clone();
    save_btn.connect_clicked(move |_| {
        let mut c = cfg_cell.borrow_mut();
        c.threshold_minutes = threshold_adj.value() as i64;
        c.break_minutes = break_adj.value() as i64;
        c.animation_mode = match mode_drop.selected() {
            1 => "walk-zoom",
            2 => "fade-center",
            3 => "walk-flat",
            _ => "walk-center",
        }
        .to_string();
        c.custom_webm_dir = dir2.borrow().to_string_lossy().to_string();
        c.log_enabled = log_switch.is_active();
        on_save(c.clone());
        win2.close();
    });

    let win3 = window.clone();
    cancel_btn.connect_clicked(move |_| win3.close());

    window.present();
}
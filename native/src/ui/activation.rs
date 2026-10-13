//! 激活窗口：展示机器码、输入序列号、显示授权状态。

// glib 是 gtk4 的依赖，须经 gtk 路径引入（Rust 2018 起不能用裸 crate 名）
use gtk::glib;
use gtk::prelude::*;

use crate::license::{self, LicenseStatus};

pub fn present<F>(on_activated: F)
where
    F: Fn() + 'static,
{
    let window = adw::Window::builder()
        .title("激活 胖猫暂停一下（PurrPause Linux）")
        .modal(true)
        .resizable(false)
        .default_width(560)
        .build();

    let header = adw::HeaderBar::new();
    window.set_titlebar(Some(&header));

    let page = adw::PreferencesPage::new();
    let status = license::check_status();

    // 状态
    let status_group = adw::PreferencesGroup::new();
    page.add(&status_group);
    let status_row = adw::ActionRow::new();
    status_row.set_title("授权状态");
    let status_text = match status.status {
        LicenseStatus::Active if status.is_permanent => "已激活（永久）".to_string(),
        LicenseStatus::Active => format!("已激活（剩余 {} 天）", status.days_left),
        LicenseStatus::Trial => format!("试用中（剩余 {} 天）", status.days_left),
        LicenseStatus::Expired => "未激活 / 已过期".to_string(),
    };
    status_row.set_subtitle(&status_text);
    status_group.add(&status_row);

    // 机器码
    let mid_group = adw::PreferencesGroup::new();
    mid_group.set_title(Some("第1 步：获取机器码"));
    mid_group.set_description(Some("把下面的机器码发给开发者，获取对应的序列号。"));
    page.add(&mid_group);

    let mid = license::machine_id();
    let mid_row = adw::ActionRow::new();
    mid_row.set_title("机器码");
    mid_row.set_subtitle(&mid);
    let copy_btn = gtk::Button::with_label("复制");
    copy_btn.set_valign(gtk::Align::Center);
    mid_row.add_suffix(&copy_btn);
    mid_group.add(&mid_row);

    // 序列号
    let serial_group = adw::PreferencesGroup::new();
    serial_group.set_title(Some("第 2 步：输入序列号"));
    page.add(&serial_group);

    let serial_row = adw::ActionRow::new();
    serial_row.set_title("序列号");
    let serial_entry = gtk::Entry::new();
    serial_entry.set_hexpand(true);
    serial_entry.set_valign(gtk::Align::Center);
    serial_entry.set_placeholder_text(Some("PP-XXXXXXXX-XXXXXXXX-..."));
    serial_entry.set_max_width_chars(40);
    serial_row.add_suffix(&serial_entry);
    serial_group.add(&serial_row);

    let result_label = gtk::Label::new(Some(""));
    result_label.set_wrap(true);
    result_label.set_margin_top(8);
    result_label.set_margin_bottom(8);
    result_label.set_margin_start(12);
    result_label.set_margin_end(12);

    // 激活按钮
    let action_row = adw::PreferencesGroup::new();
    page.add(&action_row);
    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    btn_box.set_halign(gtk::Align::Center);
    btn_box.set_margin_top(8);
    btn_box.set_margin_bottom(16);

    let activate_btn = gtk::Button::with_label("激活");
    activate_btn.add_css_class("suggested-action");
    activate_btn.add_css_class("pill");
    btn_box.append(&activate_btn);

    let skip_btn = gtk::Button::with_label("暂不激活");
    btn_box.append(&skip_btn);
    action_row.add(&btn_box);

    // 结果提示直接放进 page 底部
    result_label.set_halign(gtk::Align::Center);
    result_label.set_margin_bottom(12);
    page.add(&result_label);

    window.set_content(Some(&page));

    // 复制机器码
    let mid2 = mid.clone();
    copy_btn.connect_clicked(move |_| {
        mid2.copy();
    });

    // 激活
    let win2 = window.clone();
    let entry2 = serial_entry.clone();
    let label2 = result_label.clone();
    activate_btn.connect_clicked(move |_| {
        let serial = entry2.text().to_string();
        if serial.trim().is_empty() {
            label2.set_text("请输入序列号");
            label2.remove_css_class("success");
            label2.add_css_class("error");
            return;
        }
        let res = license::activate(&serial);
        if res.success {
            label2.set_text("激活成功");
            label2.remove_css_class("error");
            label2.add_css_class("success");
            on_activated();
            let w = win2.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                w.close();
            });
        } else {
            label2.set_text(&format!(
                "激活失败：{}",
                res.error.unwrap_or_else(|| "未知错误".to_string())
            ));
            label2.remove_css_class("success");
            label2.add_css_class("error");
        }
    });

    let win3 = window.clone();
    skip_btn.connect_clicked(move |_| win3.close());

    window.present();
}
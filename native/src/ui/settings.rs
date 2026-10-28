//! 设置窗口。
//!
//! 字段、顺序、文案与 electron 版 `renderer/settings.html` **逐字对齐**：
//!   屏幕使用时间 / 休息时长 / 动画模式 / 素材目录 / 开机自动启动 / 运行日志
//!
//! 与 electron 版一致的两点行为（之前 native 版没做到，用户体感就是「设置无效」）：
//!   1. **点保存后窗口不关**，并弹出「设置已保存」提示条 ——
//!      旧实现保存后立刻 close()，界面直接消失，用户完全看不出保存成功没有。
//!   2. 保存失败时把错误**显示在窗口里**，而不是只往 stderr 打一行。

// adw 的方法（PreferencesPage::add / ActionRow::set_title 等）来自
// libadwaita 自己的 Ext trait，gtk::prelude 里没有，必须单独引入。
use adw::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::core::config::Config;

thread_local! {
    /// 当前打开的设置窗口。
    ///
    /// ⚠️ 必须强引用持有：present() 之后本函数返回、局部句柄析构，
    /// 若无其他引用，窗口可能随之销毁（表现为看不到窗口 / 闪一下就没了）。
    /// 同时用于单例：已打开时直接前置聚焦，不再开第二个。
    static OPEN_WIN: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// 打开设置窗口。
///
/// - `on_save`：用户点「保存」时触发（已含新配置）。
/// - `on_test`：用户点「立即测试」时触发 —— 不依赖托盘，任何环境下都能验证屏保。
/// - `on_open_log`：用户点「打开日志目录」时触发。
pub fn present<F, T, L>(
    app: &adw::Application,
    current: Config,
    _idle_method: String,
    on_save: F,
    on_test: T,
    on_open_log: L,
) where
    F: Fn(Config) + 'static,
    T: Fn() + 'static,
    L: Fn() + 'static,
{
    // 已有设置窗口打开 → 提到前台即可（electron 版也是单窗口复用）
    if let Some(w) = OPEN_WIN.with(|s| s.borrow().clone()) {
        w.present();
        return;
    }

    // ⚠️ 用普通 gtk::Window，**不要**在内容里塞自绘 HeaderBar。
    //
    // 之前用 adw::Window + 内容顶部 HeaderBar：标题栏区域和窗口管理器画出来的
    // 装饰叠在一起，用户点「关闭」完全没反应（v1.9.x 反复出现）。
    // 现在交给窗口管理器画标题栏 —— 系统自带的「×」在任何桌面上都能关窗口，
    // 这是最不容易出错的一条路；页面底部再放一组「关闭 / 保存」。
    let window = gtk::Window::builder()
        .title("胖猫暂停一下（PurrPause） 设置")
        .resizable(true)
        .default_width(480)
        .build();
    // 关联到应用：窗口才会进入应用的窗口列表（任务栏分组、再次激活时前置）。
    // 未关联的裸窗口在 GTK4 里能显示，但不被 GtkApplication 管理。
    // ⚠️ 不能用 `gtk::Application::default()` —— 那是 `Default::default()`，
    //    会新建一个空 id 的 Application 实例，不是「取当前应用」。
    window.set_application(Some(app));

    let save_btn = gtk::Button::with_label("保存");
    save_btn.add_css_class("suggested-action");
    let cancel_btn = gtk::Button::with_label("关闭");

    // 底部按钮条
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bottom.set_halign(gtk::Align::End);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);
    bottom.set_margin_top(8);
    bottom.set_margin_bottom(12);
    bottom.append(&cancel_btn);
    bottom.append(&save_btn);

    let page = adw::PreferencesPage::new();

    // ── 0. 快捷操作 ────────────────────────────────────────────────
    let quick_group = adw::PreferencesGroup::new();
    quick_group.set_title("快捷操作");
    page.add(&quick_group);

    let test_row = adw::ActionRow::new();
    test_row.set_title("立即测试猫咪");
    test_row.set_subtitle("马上弹出一次休息提醒，检查动画是否正常");
    let test_btn = gtk::Button::with_label("测试");
    test_btn.set_valign(gtk::Align::Center);
    test_row.add_suffix(&test_btn);
    quick_group.add(&test_row);
    {
        let win = window.clone();
        let on_test = Rc::new(on_test);
        test_btn.connect_clicked(move |_| {
            // 先关掉设置窗口，否则全屏覆盖层会盖在它上面，
            // 用户测完还得自己找回设置窗口。
            win.close();
            (on_test)();
        });
    }

    let log_row = adw::ActionRow::new();
    log_row.set_title("日志目录");
    log_row.set_subtitle("排查问题时把这里的日志发给开发者");
    let log_btn = gtk::Button::with_label("打开");
    log_btn.set_valign(gtk::Align::Center);
    log_row.add_suffix(&log_btn);
    quick_group.add(&log_row);
    {
        let on_open_log = Rc::new(on_open_log);
        log_btn.connect_clicked(move |_| (on_open_log)());
    }

    // 「退出应用」：常驻进程必须有一个显式退出入口。
    // 放在这里而不是标题栏 —— 标题栏挤三个按钮会把窗口标题压没。
    let quit_row = adw::ActionRow::new();
    quit_row.set_title("退出应用");
    quit_row.set_subtitle("完全退出应用，停止后台的休息提醒");
    let quit_btn = gtk::Button::with_label("退出");
    quit_btn.add_css_class("destructive-action");
    quit_btn.set_valign(gtk::Align::Center);
    quit_row.add_suffix(&quit_btn);
    quick_group.add(&quit_row);
    quit_btn.connect_clicked(move |_| {
        // 与托盘菜单的「退出」一致：直接结束进程。
        std::process::exit(0);
    });

    // 未激活时把「动画模式 / 素材目录」灰掉是 electron 版的旧设计，
    // 但实测体感极差：用户打开设置发现一半控件点不动，只会认为「设置坏了」。
    // 现在全部可改可保存 —— 试用期也应该能体验全部动画模式。

    // electron 版是一张无标题卡片，6 行依次排列
    let group = adw::PreferencesGroup::new();
    page.add(&group);

    // ── 1. 屏幕使用时间 ──────────────────────────────────────────
    let threshold_row = adw::ActionRow::new();
    threshold_row.set_title("屏幕使用时间");
    threshold_row.set_subtitle("连续使用多久后提醒休息");
    let threshold_adj =
        gtk::Adjustment::new(current.threshold_minutes as f64, 1.0, 480.0, 1.0, 10.0, 0.0);
    let threshold_spin = gtk::SpinButton::new(Some(&threshold_adj), 1.0, 0);
    threshold_spin.set_valign(gtk::Align::Center);
    threshold_row.add_suffix(&spin_with_unit(&threshold_spin, "分钟"));
    group.add(&threshold_row);

    // ── 2. 休息时长 ─────────────────────────────────────────────
    let break_row = adw::ActionRow::new();
    break_row.set_title("休息时长");
    break_row.set_subtitle("每次休息持续多久");
    let break_adj = gtk::Adjustment::new(current.break_minutes as f64, 1.0, 60.0, 1.0, 5.0, 0.0);
    let break_spin = gtk::SpinButton::new(Some(&break_adj), 1.0, 0);
    break_spin.set_valign(gtk::Align::Center);
    break_row.add_suffix(&spin_with_unit(&break_spin, "分钟"));
    group.add(&break_row);

    // ── 3. 动画模式 ─────────────────────────────────────────────
    let mode_row = adw::ActionRow::new();
    mode_row.set_title("动画模式");
    mode_row.set_subtitle("猫咪出现的方式");
    // ⚠️ 文案与顺序必须与 electron 版 settings.html 的下拉框逐字一致，
    //    索引映射见下方 match —— 改文案时务必同步改。
    let modes = ["右下角走出", "中央淡入", "右侧走出", "中央走出"];
    let current_mode = match current.animation_mode.as_str() {
        "walk-zoom" => 0,
        "fade-center" => 1,
        "walk-flat" => 2,
        _ => 3, // walk-center 是默认值
    };
    let mode_drop = gtk::DropDown::from_strings(&modes);
    mode_drop.set_selected(current_mode as u32);
    mode_drop.set_valign(gtk::Align::Center);
    mode_row.add_suffix(&mode_drop);
    group.add(&mode_row);

    // ── 4. 素材目录 ─────────────────────────────────────────────
    //
    // 只有「浏览 / 重置」两个后缀按钮。之前还塞了一个路径输入框，
    // 结果它被 PreferencesGroup 排到整组的最底部（截图里那个孤零零的输入框），
    // 既不像样又容易被误认为「多出来的控件」。选择器本身已经能用，输入框去掉。
    let asset_row = adw::ActionRow::new();
    asset_row.set_title("素材目录");
    // ActionRow::set_subtitle 收 &str（不是 Option<&str>、也不是 String）
    asset_row.set_subtitle(if current.custom_webm_dir.is_empty() {
        "使用默认目录"
    } else {
        &current.custom_webm_dir
    });

    let pick_btn = gtk::Button::with_label("浏览");
    pick_btn.set_valign(gtk::Align::Center);
    let clear_btn = gtk::Button::with_label("重置");
    clear_btn.set_valign(gtk::Align::Center);
    clear_btn.set_sensitive(!current.custom_webm_dir.is_empty());
    asset_row.add_suffix(&pick_btn);
    asset_row.add_suffix(&clear_btn);
    group.add(&asset_row);

    // ⚠️ 必须用当前值初始化，不能给空路径。
    // 历史 bug：这里曾是 `PathBuf::new()`，而保存回调会把 holder 的内容当作
    // 新配置写回 —— 于是用户只要打开设置点一下「保存」，已配置的素材目录就被清空了。
    let dir_holder: Rc<RefCell<PathBuf>> = Rc::new(RefCell::new(PathBuf::from(
        &current.custom_webm_dir,
    )));
    let dir_holder_save = dir_holder.clone();

    // 浏览：用自实现的目录选择器（GTK 自带的在缺 portal/gvfs 时列不出目录）
    {
        let win = window.clone();
        let holder = dir_holder.clone();
        let sub = asset_row.clone();
        let clear = clear_btn.clone();
        pick_btn.connect_clicked(move |_| {
            let start = holder.borrow().clone();
            let h = holder.clone();
            let s2 = sub.clone();
            let c2 = clear.clone();
            crate::ui::dir_picker::pick(&win, Some(start), move |picked| {
                let text = picked.to_string_lossy().to_string();
                *h.borrow_mut() = picked;
                s2.set_subtitle(&text);
                c2.set_sensitive(true);
            });
        });
    }

    // 重置
    {
        let holder = dir_holder.clone();
        let sub = asset_row.clone();
        let clear = clear_btn.clone();
        clear_btn.connect_clicked(move |_| {
            holder.borrow_mut().clear();
            sub.set_subtitle("使用默认目录");
            clear.set_sensitive(false);
        });
    }

    // ── 5. 开机自动启动 ─────────────────────────────────────────
    let autostart_row = adw::ActionRow::new();
    autostart_row.set_title("开机自动启动");
    autostart_row.set_subtitle("系统启动时自动运行 胖猫暂停一下（PurrPause）");
    let autostart_switch = gtk::Switch::new();
    autostart_switch.set_valign(gtk::Align::Center);
    autostart_switch.set_active(crate::core::autostart::is_enabled());
    autostart_row.add_suffix(&autostart_switch);
    group.add(&autostart_row);

    // ── 6. 运行日志 ─────────────────────────────────────────────
    let log_switch_row = adw::ActionRow::new();
    log_switch_row.set_title("运行日志");
    log_switch_row.set_subtitle("记录运行信息到日志文件，用于排查问题（默认关闭）");
    let log_switch = gtk::Switch::new();
    log_switch.set_valign(gtk::Align::Center);
    log_switch.set_active(current.log_enabled);
    log_switch_row.add_suffix(&log_switch);
    group.add(&log_switch_row);

    // ── 保存结果提示 ────────────────────────────────────────────
    // 用 AdwToastOverlay 包住页面：保存成功/失败都能在窗口里看见，
    // 而不是「点一下窗口就没了」。
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&page));

    // ⚠️ AdwWindow **不支持** gtk_window_set_titlebar() —— libadwaita 会直接
    //    Adwaita-ERROR abort（核心转储）。v1.0.0「一打开设置窗口就崩」
    //    （Adwaita-ERROR: gtk_window_set_titlebar() is not supported for
    //    AdwWindow）就是这个原因。
    //    标题栏必须作为内容顶部的工具条：垂直 GtkBox 包 HeaderBar + 内容。
    //    这是 libadwaita 1.1 起的标准写法（1.2+ 也可用 ToolbarView，取兼容写法）。
    page.set_vexpand(true);
    toast_overlay.set_vexpand(true);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&toast_overlay);
    root.append(&bottom);
    window.set_child(Some(&root));

    // ── 保存 ────────────────────────────────────────────────────
    let cfg_cell: Rc<RefCell<Config>> = Rc::new(RefCell::new(current.clone()));
    let toast = toast_overlay.clone();
    let dir2 = dir_holder_save;
    save_btn.connect_clicked(move |_| {
        let mut c = cfg_cell.borrow_mut();
        c.threshold_minutes = threshold_adj.value() as i64;
        c.break_minutes = break_adj.value() as i64;
        c.animation_mode = match mode_drop.selected() {
            0 => "walk-zoom",
            1 => "fade-center",
            2 => "walk-flat",
            _ => "walk-center",
        }
        .to_string();
        c.custom_webm_dir = dir2.borrow().to_string_lossy().to_string();
        c.log_enabled = log_switch.is_active();

        // 开机自启：**不是** config 字段，而是写 ~/.config/autostart/*.desktop。
        // 历史 bug：这个开关早就画在界面上，但保存回调从没读过它 —— 拨了完全没用。
        let want_autostart = autostart_switch.is_active();
        if let Err(e) = crate::core::autostart::set_enabled(want_autostart) {
            eprintln!("[purr-pause-linux] 设置开机自启失败：{e}");
            toast.add_toast(adw::Toast::new(&format!("开机自启设置失败：{e}")));
        }

        let saved = c.clone();
        drop(c);
        on_save(saved);

        // 不关闭窗口：给用户明确的「已保存」反馈，也方便连续调整。
        toast.add_toast(adw::Toast::new("设置已保存"));
    });

    // ⚠️ 用 destroy() 而不是 close()：close() 走的是 close_request 信号 +
    //    WM 协作，实测在这台机器上「点关闭没反应」。destroy() 直接把顶层窗口
    //    拆掉，不依赖任何外部条件。
    let win3 = window.clone();
    cancel_btn.connect_clicked(move |_| win3.destroy());

    // Esc 也能关（第二条逃生通道）
    {
        let win = window.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                win.destroy();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
    }

    // 存入槽（强引用持有）+ 销毁时清槽：
    // 用户点标题栏 X / 关闭后窗口销毁，下次打开能拿到最新配置重建。
    window.connect_destroy(|_| {
        OPEN_WIN.with(|s| *s.borrow_mut() = None);
    });
    OPEN_WIN.with(|s| *s.borrow_mut() = Some(window.clone()));

    window.present();
}

/// 把数值控件和单位标签打包成一个后缀（界面上显示成「45 分钟」，
/// 与 electron 版 input 后面的单位文案一致）。
fn spin_with_unit(spin: &gtk::SpinButton, unit: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.set_valign(gtk::Align::Center);
    b.append(spin);
    let label = gtk::Label::new(Some(unit));
    label.add_css_class("dim-label");
    label.set_valign(gtk::Align::Center);
    b.append(&label);
    b
}

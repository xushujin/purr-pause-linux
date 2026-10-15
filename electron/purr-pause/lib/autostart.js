const fs = require('fs');
const path = require('path');

// 开机自启动：跨平台开关。非 Linux 走 Electron 的 LoginItemSettings；
// Linux 写/删 ~/.config/autostart/purr-pause.desktop。
// 通过工厂注入 app/logger，避免直接耦合 electron 与全局 logger 单例。
function createAutostart({ app, logger }) {
  function quoteDesktopExecPart(value) {
    return '"' + String(value).replace(/(["\\$`])/g, '\\$1') + '"';
  }

  function getLinuxAutostartPath() {
    return path.join(app.getPath('home'), '.config', 'autostart', 'purr-pause.desktop');
  }

  function getLinuxLaunchCommand() {
    // AppImage 运行时 process.execPath 指向临时挂载点（/tmp/.mount_*），重启后失效；
    // 必须用 process.env.APPIMAGE（.AppImage 文件自身的真实路径）写自启 Exec。
    if (process.env.APPIMAGE) {
      return quoteDesktopExecPart(process.env.APPIMAGE);
    }
    if (app.isPackaged) {
      return quoteDesktopExecPart(process.execPath);
    }
    return quoteDesktopExecPart(process.execPath) + ' ' + quoteDesktopExecPart(app.getAppPath());
  }

  function buildLinuxAutostartEntry() {
    return [
      '[Desktop Entry]',
      'Type=Application',
      'Name=胖猫暂停一下（PurrPause）',
      'Comment=胖猫暂停一下 - 屏幕休息提醒',
      'Exec=' + getLinuxLaunchCommand(),
      'Icon=purr-pause',
      'Terminal=false',
      'Hidden=false',
      'NoDisplay=false',
      'X-GNOME-Autostart-enabled=true',
      'Categories=Utility;',
      ''
    ].join('\n');
  }

  function isEnabled() {
    if (process.platform !== 'linux') {
      return app.getLoginItemSettings().openAtLogin;
    }

    const autostartPath = getLinuxAutostartPath();
    if (!fs.existsSync(autostartPath)) return false;

    try {
      const content = fs.readFileSync(autostartPath, 'utf-8');
      if (/^Hidden\s*=\s*true\s*$/im.test(content)) return false;
      if (/^X-GNOME-Autostart-enabled\s*=\s*false\s*$/im.test(content)) return false;
      return true;
    } catch (e) {
      logger.warn('读取 Linux 自启动配置失败: ' + e.message);
      return false;
    }
  }

  function setEnabled(enabled) {
    if (process.platform !== 'linux') {
      app.setLoginItemSettings({ openAtLogin: !!enabled });
      return;
    }

    const autostartPath = getLinuxAutostartPath();
    try {
      if (enabled) {
        fs.mkdirSync(path.dirname(autostartPath), { recursive: true });
        fs.writeFileSync(autostartPath, buildLinuxAutostartEntry(), { mode: 0o644 });
        logger.info('Linux 自启动已启用: ' + autostartPath);
      } else if (fs.existsSync(autostartPath)) {
        fs.unlinkSync(autostartPath);
        logger.info('Linux 自启动已禁用: ' + autostartPath);
      }
    } catch (e) {
      logger.error('设置 Linux 自启动失败: ' + e.message);
    }
  }

  return { isEnabled, setEnabled };
}

module.exports = createAutostart;

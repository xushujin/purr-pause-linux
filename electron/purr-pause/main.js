const { app, BrowserWindow, Tray, Menu, ipcMain, nativeImage, screen, dialog, powerMonitor, shell } = require('electron');
const path = require('path');
const { pathToFileURL } = require('url');
const { execFileSync, execFile } = require('child_process');
const fs = require('fs');
const license = require('./lib/license');
const logger = require('./lib/logger');
const createMessageNotify = require('./lib/message-notify');
const createAutostart = require('./lib/autostart');
const createAssets = require('./lib/assets');
const configLib = require('./lib/config');

app.setName('胖猫暂停一下');
app.setPath('userData', path.join(app.getPath('appData'), 'purr-pause'));

const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.exit(0);
  process.exit(0);
}

app.on('second-instance', () => {
  logger.info('检测到第二个实例启动请求, 已阻止');
});

app.commandLine.appendSwitch('enable-gpu-rasterization');
app.commandLine.appendSwitch('enable-zero-copy');
app.commandLine.appendSwitch('disable-software-rasterizer');

const IS_PACKAGED = app.isPackaged;
const RESOURCES_PATH = IS_PACKAGED
  ? path.join(process.resourcesPath, 'assets')
  : path.join(__dirname, 'assets');

const USER_CONFIG_PATH = path.join(app.getPath('userData'), 'config.json');
const DEFAULT_CONFIG_PATH = path.join(__dirname, 'config.json');
const MESSAGE_API_SPEC_PATH = path.join(__dirname, 'docs', 'message-api-spec.md');

const autostart = createAutostart({ app, logger });

let config = { ...configLib.DEFAULT_CONFIG };
const assets = createAssets({ resourcesPath: RESOURCES_PATH, userDataPath: app.getPath('userData'), defaultNotifyVideo: configLib.DEFAULT_CONFIG.messageNotifyVideo });
let wins = [];
let settingsWin = null;
let messageSettingsWin = null;
let activationWin = null;
let rulesWin = null;
let rocketDemoWin = null;
let rocketDemoTimer = null;
let messageNotify = null;
let messageBadgeCount = 0;
let tray = null;
let activeSeconds = 0;
let monitorInterval = null;
let isOverlayShowing = false;
let lastDismissTime = 0;
let idleMethod = null;
let isPaused = false;
let pauseTimer = null;
let snoozeCount = 0;
let snoozeThreshold = 0;

const ROCKET_DEMO_SIZE = 240;
const ROCKET_DEMO_DURATION_MS = 7000;
const ROCKET_DEMO_LABEL = '绘家科技';

function detectIdleMethod() {
  if (process.platform === 'darwin') {
    idleMethod = 'ioreg';
    return;
  }
  if (process.platform === 'win32') {
    idleMethod = 'powerShell';
    return;
  }
  try {
    execFileSync('xprintidle', [], { timeout: 3000 });
    idleMethod = 'xprintidle';
    return;
  } catch (e) {}
  try {
    execFileSync('gdbus', [
      'call', '--session',
      '--dest', 'org.gnome.Mutter.IdleMonitor',
      '--object-path', '/org/gnome/Mutter/IdleMonitor/Core',
      '--method', 'org.gnome.Mutter.IdleMonitor.GetIdletime'
    ], { timeout: 3000 });
    idleMethod = 'gdbus';
  } catch (e) {
    idleMethod = null;
  }
}

function getIdleTimeMs(callback) {
  if (idleMethod === 'ioreg') {
    execFile('/usr/sbin/ioreg', ['-c', 'IOHIDSystem', '-d', '4'], { timeout: 5000 }, (err, stdout) => {
      if (err) { callback(0); return; }
      const match = stdout.match(/"HIDIdleTime"\s*=\s*(\d+)/);
      if (match) {
        callback(Math.floor(parseInt(match[1], 10) / 1000000));
      } else {
        callback(0);
      }
    });
  } else if (idleMethod === 'powerShell') {
    const script = `Add-Type @'
using System;
using System.Runtime.InteropServices;
public class IdleTime {
    [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);
    [StructLayout(LayoutKind.Sequential)] struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    public static int Get() {
        LASTINPUTINFO lii = new LASTINPUTINFO();
        lii.cbSize = (uint)Marshal.SizeOf(lii);
        GetLastInputInfo(ref lii);
        return (int)(Environment.TickCount - lii.dwTime);
    }
}
'@
[IdleTime]::Get()`;
    execFile('powershell', ['-NoProfile', '-Command', script], { timeout: 5000 }, (err, stdout) => {
      if (err) { callback(0); return; }
      const val = parseInt(stdout.trim(), 10);
      callback(isNaN(val) ? 0 : val);
    });
  } else if (idleMethod === 'xprintidle') {
    execFile('xprintidle', [], { timeout: 5000 }, (err, stdout) => {
      if (err) { callback(0); return; }
      const val = parseInt(stdout.trim(), 10);
      callback(isNaN(val) ? 0 : val);
    });
  } else if (idleMethod === 'gdbus') {
    execFile('gdbus', [
      'call', '--session',
      '--dest', 'org.gnome.Mutter.IdleMonitor',
      '--object-path', '/org/gnome/Mutter/IdleMonitor/Core',
      '--method', 'org.gnome.Mutter.IdleMonitor.GetIdletime'
    ], { timeout: 5000 }, (err, stdout) => {
      if (!err && stdout) {
        const match = stdout.trim().match(/\(uint64 (\d+),?\)/);
        if (match) { callback(parseInt(match[1], 10)); return; }
      }
      callback(0);
    });
  } else {
    // No idle detection available — assume user is active
    callback(0);
  }
}

let idleCheckPending = false;
let isScreenLocked = false;
let screenLockedAt = 0;
let sleepStartedAt = 0;
let powerMonitorStarted = false;

function handleLockStateChange(locked) {
  if (locked === isScreenLocked) return;
  isScreenLocked = locked;
  if (locked) {
    screenLockedAt = Date.now();
    logger.info('屏幕已锁定');
  } else {
    const lockedDuration = Date.now() - screenLockedAt;
    logger.info('屏幕已解锁 (锁定了' + Math.round(lockedDuration / 1000) + '秒)');
    if (screenLockedAt > 0 && lockedDuration > 10 * 60 * 1000) {
      if (activeSeconds > 0) {
        logger.info('锁屏超过10分钟, 计时归零 (之前累计' + activeSeconds + '秒)');
      }
      activeSeconds = 0;
      updateTray();
    }
    screenLockedAt = 0;
  }
}

function resetActiveTimeAfterLongBreak(reason, durationMs) {
  if (durationMs <= 10 * 60 * 1000) return;
  if (activeSeconds > 0) {
    logger.info(reason + '超过10分钟, 计时归零 (之前累计' + activeSeconds + '秒)');
  }
  activeSeconds = 0;
  snoozeThreshold = 0;
  updateTray();
}

function checkScreenLockMac() {
  execFile('/usr/bin/python3', [
    '-c',
    "import Quartz; print(Quartz.CGSessionCopyCurrentDictionary().get('CGSSessionScreenIsLocked', 0))"
  ], { timeout: 3000 }, (err, stdout) => {
    if (!err) {
      handleLockStateChange(stdout.trim() === '1');
      return;
    }

    execFile('/usr/sbin/ioreg', ['-n', 'Root', '-d1', '-a'], { timeout: 3000 }, (err2, stdout2) => {
      if (err2) {
        logger.warn('macOS 锁屏状态检测失败: ' + err2.message);
        return;
      }
      handleLockStateChange(stdout2.includes('CGSSessionScreenLockedTime'));
    });
  });
}

function checkScreenLockWin() {
  execFile('powershell', [
    '-NoProfile',
    '-Command',
    'Get-Process LogonUI -ErrorAction SilentlyContinue | Measure-Object | Select-Object -ExpandProperty Count'
  ], { timeout: 3000 }, (err, stdout) => {
    if (err) {
      logger.warn('Windows 锁屏状态检测失败: ' + err.message);
      return;
    }
    handleLockStateChange(parseInt(stdout.trim(), 10) > 0);
  });
}

function refreshScreenLockState() {
  if (process.platform === 'darwin') {
    checkScreenLockMac();
  } else if (process.platform === 'win32') {
    checkScreenLockWin();
  }
}

function setupPowerMonitor() {
  if (powerMonitorStarted) return;
  powerMonitorStarted = true;

  powerMonitor.on('suspend', () => {
    sleepStartedAt = Date.now();
    idleCheckPending = false;
    logger.info('系统即将休眠');
  });

  powerMonitor.on('resume', () => {
    const sleptMs = sleepStartedAt > 0 ? Date.now() - sleepStartedAt : 0;
    logger.info('系统已唤醒' + (sleptMs > 0 ? ' (休眠' + Math.round(sleptMs / 1000) + '秒)' : ''));
    idleCheckPending = false;
    detectIdleMethod();
    resetActiveTimeAfterLongBreak('休眠', sleptMs);
    refreshScreenLockState();
    setTimeout(refreshScreenLockState, 3000);
    sleepStartedAt = 0;
  });

  if (process.platform === 'darwin' || process.platform === 'win32') {
    powerMonitor.on('lock-screen', () => handleLockStateChange(true));
    powerMonitor.on('unlock-screen', () => handleLockStateChange(false));
  }
}

function detectScreenLock() {
  if (process.platform === 'darwin') {
    detectScreenLockMac();
    return;
  }
  if (process.platform === 'win32') {
    detectScreenLockWin();
    return;
  }
  const { spawn } = require('child_process');
  const monitor = spawn('gdbus', [
    'monitor', '--session',
    '--dest', 'org.gnome.ScreenSaver',
    '--object-path', '/org/gnome/ScreenSaver'
  ]);

  monitor.stdout.on('data', (data) => {
    const str = data.toString();
    if (str.includes('ActiveChanged')) {
      if (str.includes('true')) {
        handleLockStateChange(true);
      } else if (str.includes('false')) {
        handleLockStateChange(false);
      }
    }
  });

  monitor.on('error', () => {
    isScreenLocked = false;
    logger.warn('锁屏检测进程出错, 重置为未锁定');
  });
  monitor.on('close', () => {
    isScreenLocked = false;
    logger.warn('锁屏检测进程退出, 5秒后重连');
    setTimeout(() => detectScreenLock(), 5000);
  });
  monitor.unref();
}

function detectScreenLockMac() {
  checkScreenLockMac();
  setInterval(() => {
    checkScreenLockMac();
  }, 5000);
}

function detectScreenLockWin() {
  checkScreenLockWin();
  setInterval(() => {
    checkScreenLockWin();
  }, 5000);
}
function startMonitoring() {
  if (monitorInterval) return;
  detectScreenLock();
  monitorInterval = setInterval(() => {
    if (isOverlayShowing) return;
    if (isPaused) return;
    if (isScreenLocked) return;
    if (idleCheckPending) return;
    idleCheckPending = true;

    getIdleTimeMs((idleMs) => {
      idleCheckPending = false;
      const idleResetMs = 10 * 60 * 1000;
      if (idleMs > idleResetMs) {
        if (activeSeconds > 0) {
          logger.info('空闲超过10分钟, 计时归零 (空闲' + Math.round(idleMs / 1000) + '秒, 之前累计' + activeSeconds + '秒)');
        }
        activeSeconds = 0;
        updateTray();
        return;
      }

      if (idleMs < 180000) {
        activeSeconds += 10;
      }

      updateTray();

      if (activeSeconds >= (snoozeThreshold || config.thresholdMinutes * 60)) {
        snoozeThreshold = 0;
        logger.info('达到阈值, 触发动画 (累计' + activeSeconds + '秒)');
        triggerCat();
      }
    });
  }, 10000);
}

function triggerCat(manual) {
  if (isOverlayShowing) return;
  if (!manual && Date.now() - lastDismissTime < 60000) return;
  isOverlayShowing = true;
  if (messageNotify) messageNotify.handleRestOverlayChanged(true);

  const displays = screen.getAllDisplays();

  wins.forEach(w => { if (w && !w.isDestroyed()) w.destroy(); });
  wins = [];

  const videos = assets.getVideoPaths(config);

  displays.forEach((display) => {
    const { x, y, width, height } = display.workArea;

    const w = new BrowserWindow({
      x, y, width, height,
      transparent: true,
      frame: false,
      alwaysOnTop: true,
      skipTaskbar: true,
      focusable: false,
      hasShadow: false,
      show: false,
      resizable: false,
      webPreferences: {
        preload: path.join(__dirname, 'preload.js'),
        contextIsolation: true,
        nodeIntegration: false,
        backgroundThrottling: false
      }
    });

    w.setVisibleOnAllWorkspaces(true);
    w.loadFile(path.join(__dirname, 'renderer', 'index.html'));

    w.webContents.on('did-finish-load', () => {
      w.setIgnoreMouseEvents(true, { forward: true });
      w.setAlwaysOnTop(true, 'screen-saver');
      w.setBackgroundColor('#00000000');
      w.show();
      w.moveTop();
      setTimeout(() => {
        if (w && !w.isDestroyed()) {
          w.webContents.send('start-animation', { ...config, videos, snoozeCount });
        }
      }, 50);
    });

    wins.push(w);
  });
}

function closeRocketDemo() {
  if (rocketDemoTimer) {
    clearTimeout(rocketDemoTimer);
    rocketDemoTimer = null;
  }
  if (rocketDemoWin && !rocketDemoWin.isDestroyed()) {
    const win = rocketDemoWin;
    rocketDemoWin = null;
    win.hide();
    setTimeout(() => {
      if (win && !win.isDestroyed()) win.destroy();
    }, 30);
    return;
  }
  rocketDemoWin = null;
}

function scheduleRocketDemoClose(win) {
  if (rocketDemoTimer) clearTimeout(rocketDemoTimer);
  rocketDemoTimer = setTimeout(() => {
    if (win && !win.isDestroyed()) closeRocketDemo();
  }, ROCKET_DEMO_DURATION_MS + 160);
}

function triggerRocketDemo(options = {}) {
  closeRocketDemo();

  const display = screen.getPrimaryDisplay();
  const area = display.workArea;
  const margin = 24;
  const label = String(options.label || ROCKET_DEMO_LABEL);
  const count = String(options.count || 3);
  const mediaUrl = options.mediaUrl || assets.getRocketMediaUrl(config.messageNotifyVideo, config);
  const startX = Math.round(Math.max(area.x + margin, area.x + area.width - ROCKET_DEMO_SIZE - margin));
  const startY = Math.round(Math.max(area.y + margin, area.y + area.height - ROCKET_DEMO_SIZE - margin));
  const endY = Math.round(Math.max(area.y + margin, area.y + area.height * 0.14));
  const localStartY = Math.max(0, startY - endY);
  const pathHeight = Math.max(ROCKET_DEMO_SIZE, localStartY + ROCKET_DEMO_SIZE);

  rocketDemoWin = new BrowserWindow({
    x: startX,
    y: endY,
    width: ROCKET_DEMO_SIZE,
    height: pathHeight,
    transparent: true,
    backgroundColor: '#00000000',
    frame: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    focusable: false,
    hasShadow: false,
    show: false,
    paintWhenInitiallyHidden: true,
    resizable: false,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false
    }
  });

  const win = rocketDemoWin;
  win.setVisibleOnAllWorkspaces(true);
  win.setIgnoreMouseEvents(true, { forward: true });
  win.setBackgroundColor('#00000000');
  win.loadFile(path.join(__dirname, 'renderer', 'rocket-demo.html'), {
    query: {
      label,
      count,
      mediaUrl,
      startY: String(localStartY),
      endY: '0',
      durationMs: String(ROCKET_DEMO_DURATION_MS)
    }
  });

  win.webContents.on('did-finish-load', () => {
    if (!win || win.isDestroyed()) return;
    win.setAlwaysOnTop(true, 'screen-saver');
    win.setBackgroundColor('#00000000');
    win.webContents.executeJavaScript('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve))))')
      .catch(() => {})
      .finally(() => {
        if (!win || win.isDestroyed()) return;
        win.showInactive();
        win.moveTop();
        scheduleRocketDemoClose(win);
      });
  });

  win.on('closed', () => {
    if (rocketDemoWin === win) {
      if (rocketDemoTimer) {
        clearTimeout(rocketDemoTimer);
        rocketDemoTimer = null;
      }
      rocketDemoWin = null;
    }
  });
}

function dismissCat() {
  isOverlayShowing = false;
  if (messageNotify) messageNotify.handleRestOverlayChanged(false);
  activeSeconds = 0;
  snoozeCount = 0;
  snoozeThreshold = 0;
  lastDismissTime = Date.now();
  wins.forEach(w => { if (w && !w.isDestroyed()) w.destroy(); });
  wins = [];
  updateTray();
  logger.info('用户关闭提醒, 计时归零');
}

let lastRemainingMins = -1;

function updateTray() {
  if (!tray) return;
  if (isPaused || isOverlayShowing) return;
  const threshold = snoozeThreshold || config.thresholdMinutes * 60;
  const remaining = Math.max(0, threshold - activeSeconds);
  const mins = Math.ceil(remaining / 60);
  if (mins !== lastRemainingMins) {
    lastRemainingMins = mins;
    rebuildTrayMenu();
  }
}

function pauseMonitoring(minutes) {
  isPaused = true;
  if (pauseTimer) clearTimeout(pauseTimer);
  if (minutes > 0) {
    logger.info('暂停监控 ' + minutes + ' 分钟');
    pauseTimer = setTimeout(() => {
      resumeMonitoring();
    }, minutes * 60 * 1000);
  } else {
    logger.info('暂停监控 (手动恢复)');
    pauseTimer = null;
  }
  rebuildTrayMenu();
}

function resumeMonitoring() {
  isPaused = false;
  if (pauseTimer) {
    clearTimeout(pauseTimer);
    pauseTimer = null;
  }
  logger.info('恢复监控');
  rebuildTrayMenu();
}

function rebuildTrayMenu() {
  if (!tray) return;
  const licenseStatus = license.checkStatus(app.getPath('userData'));
  let statusLabel = '';
  if (licenseStatus.status === 'trial') {
    statusLabel = `试用中（剩余 ${licenseStatus.daysLeft} 天）`;
  } else if (licenseStatus.status === 'active') {
    statusLabel = licenseStatus.type === 'permanent' ? '已激活（永久）' : `已激活（剩余 ${licenseStatus.daysLeft} 天）`;
  } else {
    statusLabel = '未激活';
  }

  let remainingLabel = '';
  if (isPaused) {
    remainingLabel = '监控已暂停';
  } else if (!isOverlayShowing) {
    const threshold = snoozeThreshold || config.thresholdMinutes * 60;
    const remaining = Math.max(0, threshold - activeSeconds);
    const mins = Math.ceil(remaining / 60);
    remainingLabel = `距下次休息: ${mins} 分钟`;
  }

  const contextMenu = Menu.buildFromTemplate([
    { label: statusLabel, enabled: false },
    ...(remainingLabel ? [{ label: remainingLabel, enabled: false }] : []),
    { label: `待办消息（${messageBadgeCount}）`, click: () => { if (messageNotify) messageNotify.openMessageList(); } },
    { label: '消息提醒设置', click: () => openMessageSettings() },
    { type: 'separator' },
    ...(isPaused ? [
      { label: '恢复监控', click: () => resumeMonitoring() }
    ] : [
      { label: '暂停监控', submenu: [
        { label: '30 分钟', click: () => pauseMonitoring(30) },
        { label: '1 小时', click: () => pauseMonitoring(60) },
        { label: '2 小时', click: () => pauseMonitoring(120) },
        { label: '直到手动恢复', click: () => pauseMonitoring(0) }
      ]}
    ]),
    { label: '激活/续期', click: () => showActivationWindow(licenseStatus) },
    { label: '设置', click: () => openSettings() },
    { label: '计时规则', click: () => openRules() },
    { label: '查看日志', click: () => { shell.openPath(logger.getLogPath()); } },
    { label: '小火箭演示', click: () => triggerRocketDemo() },
    { label: '立即测试', click: () => triggerCat(true) },
    { label: '重置计时', click: () => { activeSeconds = 0; lastRemainingMins = -1; rebuildTrayMenu(); logger.info('手动重置计时'); } },
    { type: 'separator' },
    { label: 'v' + require('./package.json').version, enabled: false },
    { label: '退出', click: () => app.quit() }
  ]);
  tray.setContextMenu(contextMenu);
}

function openRules() {
  if (rulesWin) {
    rulesWin.focus();
    return;
  }

  const display = screen.getPrimaryDisplay();
  const { width: sw, height: sh } = display.workArea;
  const winW = Math.min(900, Math.max(720, sw - 48));
  const maxH = Math.round(sh * 0.8);

  rulesWin = new BrowserWindow({
    x: Math.round(display.workArea.x + (sw - winW) / 2),
    y: Math.round(display.workArea.y + (sh - maxH) / 2),
    width: winW,
    height: maxH,
    show: false,
    resizable: true,
    minimizable: false,
    maximizable: false,
    frame: true,
    autoHideMenuBar: true,
    title: '计时规则说明',
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  rulesWin.setMenuBarVisibility(false);
  rulesWin.loadFile(path.join(__dirname, 'renderer', 'rules.html'));
  rulesWin.webContents.on('did-finish-load', () => {
    rulesWin.webContents.executeJavaScript('document.body.scrollHeight').then((contentH) => {
      const frameExtra = rulesWin.getSize()[1] - rulesWin.getContentSize()[1];
      const winH = Math.min(contentH + frameExtra, maxH);
      rulesWin.setSize(winW, winH);
      rulesWin.setPosition(
        Math.round(display.workArea.x + (sw - winW) / 2),
        Math.round(display.workArea.y + (sh - winH) / 2)
      );
      rulesWin.show();
    });
  });
  rulesWin.on('closed', () => { rulesWin = null; });
}

function openSettings() {
  if (settingsWin) {
    settingsWin.focus();
    return;
  }

  const display = screen.getPrimaryDisplay();
  const { width: sw, height: sh } = display.workArea;
  const winW = 400;
  const maxH = Math.round(sh * 0.8);

  settingsWin = new BrowserWindow({
    x: Math.round(display.workArea.x + (sw - winW) / 2),
    y: Math.round(display.workArea.y + (sh - maxH) / 2),
    width: winW,
    height: maxH,
    show: false,
    resizable: true,
    minimizable: false,
    maximizable: false,
    frame: true,
    autoHideMenuBar: true,
    title: '胖猫暂停一下（PurrPause） 设置',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  settingsWin.setMenuBarVisibility(false);
  settingsWin.loadFile(path.join(__dirname, 'renderer', 'settings.html'));
  settingsWin.webContents.on('did-finish-load', () => {
    const licenseStatus = license.checkStatus(app.getPath('userData'));
    const isActivated = licenseStatus.status === 'active';
    settingsWin.webContents.send('load-config', { ...config, _isActivated: isActivated, _autoLaunch: autostart.isEnabled() });
    settingsWin.webContents.executeJavaScript('document.body.scrollHeight').then((contentH) => {
      const frameExtra = settingsWin.getSize()[1] - settingsWin.getContentSize()[1];
      const winH = Math.min(contentH + frameExtra, maxH);
      settingsWin.setSize(winW, winH);
      settingsWin.setPosition(
        Math.round(display.workArea.x + (sw - winW) / 2),
        Math.round(display.workArea.y + (sh - winH) / 2)
      );
      settingsWin.show();
    });
  });

  settingsWin.on('closed', () => { settingsWin = null; });
}

function openMessageSettings() {
  if (messageSettingsWin) {
    messageSettingsWin.focus();
    return;
  }

  const display = screen.getPrimaryDisplay();
  const { width: sw, height: sh } = display.workArea;
  const winW = Math.min(540, Math.max(360, sw - 80));
  const maxH = Math.round(sh * 0.8);

  messageSettingsWin = new BrowserWindow({
    x: Math.round(display.workArea.x + (sw - winW) / 2),
    y: Math.round(display.workArea.y + (sh - maxH) / 2),
    width: winW,
    height: maxH,
    show: false,
    resizable: true,
    minimizable: false,
    maximizable: false,
    frame: true,
    autoHideMenuBar: true,
    title: '消息提醒设置',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  messageSettingsWin.setMenuBarVisibility(false);
  messageSettingsWin.loadFile(path.join(__dirname, 'renderer', 'message-settings.html'));
  messageSettingsWin.webContents.on('did-finish-load', () => {
    messageSettingsWin.webContents.send('load-config', { ...config });
    messageSettingsWin.webContents.executeJavaScript('document.body.scrollHeight').then((contentH) => {
      const frameExtra = messageSettingsWin.getSize()[1] - messageSettingsWin.getContentSize()[1];
      const winH = Math.min(contentH + frameExtra, maxH);
      messageSettingsWin.setSize(winW, winH);
      messageSettingsWin.setPosition(
        Math.round(display.workArea.x + (sw - winW) / 2),
        Math.round(display.workArea.y + (sh - winH) / 2)
      );
      messageSettingsWin.show();
    });
  });

  messageSettingsWin.on('closed', () => { messageSettingsWin = null; });
}

function createTray() {
  const iconPath = path.join(RESOURCES_PATH, 'images', 'tray-icon.png');
  let icon;
  try {
    icon = nativeImage.createFromPath(iconPath).resize({ width: 22, height: 22 });
  } catch (e) {
    icon = nativeImage.createEmpty();
  }

  tray = new Tray(icon);
  tray.setToolTip('胖猫暂停一下（PurrPause）');
  rebuildTrayMenu();
}

app.whenReady().then(() => {
  config = configLib.load({ userConfigPath: USER_CONFIG_PATH, defaultConfigPath: DEFAULT_CONFIG_PATH });
  logger.init(app.getPath('userData'), config.logEnabled);
  logger.info('应用启动, 版本 ' + require('./package.json').version);
  logger.info('配置加载: thresholdMinutes=' + config.thresholdMinutes + ', breakMinutes=' + config.breakMinutes + ', animationMode=' + config.animationMode);
  detectIdleMethod();
  logger.info('空闲检测方式: ' + (idleMethod || '无'));
  setupPowerMonitor();

  const userWebmDir = path.join(app.getPath('userData'), 'webm');
  if (!fs.existsSync(userWebmDir)) fs.mkdirSync(userWebmDir, { recursive: true });
  assets.generateReadmeInDir(userWebmDir);
  if (config.messageNotifyDir) {
    assets.generateMessageNotifyReadmeInDir(config.messageNotifyDir);
  }

  createTray();
  const licenseStatus = license.checkStatus(app.getPath('userData'));
  logger.info('许可证状态: ' + licenseStatus.status + ', 类型: ' + (licenseStatus.type || ''));
  const initialMessageConfig = licenseStatus.status === 'expired'
    ? { ...config, messageNotifyEnabled: false }
    : config;
  messageNotify = createMessageNotify(initialMessageConfig, {
    BrowserWindow,
    screen,
    shell,
    preloadPath: path.join(__dirname, 'preload.js'),
    rendererDir: path.join(__dirname, 'renderer'),
    logger,
    getNotifyMediaUrl: (filename) => assets.getRocketMediaUrl(filename, config),
    isRestOverlayShowing: () => isOverlayShowing,
    onBadgeChange: (count) => {
      messageBadgeCount = count;
      rebuildTrayMenu();
    }
  });
  messageBadgeCount = messageNotify.getBadgeCount();
  rebuildTrayMenu();

  if (process.argv.includes('--rocket-demo')) {
    setTimeout(() => triggerRocketDemo(), 800);
  }

  if (licenseStatus.status === 'expired') {
    showActivationWindow(licenseStatus);
  } else {
    startMonitoring();
    logger.info('监控已启动');
  }

  if (process.argv.includes('--dev')) {
    config.thresholdMinutes = 0.15;
    activeSeconds = 0;
    logger.info('开发模式: thresholdMinutes=0.15');
  }
});

ipcMain.on('overlay-shown', (event) => {
  const w = BrowserWindow.fromWebContents(event.sender);
  if (!w || w.isDestroyed()) return;
  w.setIgnoreMouseEvents(false);
  w.setFocusable(true);
});

ipcMain.on('dismissed', () => {
  dismissCat();
});

ipcMain.on('snooze', () => {
  if (snoozeCount >= 2) return;
  snoozeCount++;
  isOverlayShowing = false;
  if (messageNotify) messageNotify.handleRestOverlayChanged(false);
  wins.forEach(w => { if (w && !w.isDestroyed()) w.destroy(); });
  wins = [];
  activeSeconds = 0;
  snoozeThreshold = 300;
  updateTray();
  logger.info('用户延后休息, 第' + snoozeCount + '次');
});

ipcMain.on('save-config', (event, newConfig) => {
  newConfig = newConfig || {};
  const hasMessageConfig = Object.prototype.hasOwnProperty.call(newConfig, 'messageNotifyEnabled')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageApiBaseUrl')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageAuthHeaders')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageSources')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messagePollInterval')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageMaxCacheItems')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageNotifyDir')
    || Object.prototype.hasOwnProperty.call(newConfig, 'messageNotifyVideo');
  if (hasMessageConfig) {
    const messageResult = configLib.applyMessageConfig(config, newConfig, assets.generateMessageNotifyReadmeInDir);
    if (!messageResult.success) {
      event.sender.send('save-config-result', messageResult);
      return;
    }
  }

  if (newConfig.thresholdMinutes !== undefined) {
    config.thresholdMinutes = Math.max(1, Math.min(480, parseInt(newConfig.thresholdMinutes) || 45));
  }
  if (newConfig.breakMinutes !== undefined) {
    config.breakMinutes = Math.max(1, Math.min(60, parseInt(newConfig.breakMinutes) || 5));
  }
  const validModes = ['walk-zoom', 'fade-center', 'walk-flat', 'walk-center'];
  if (newConfig.animationMode !== undefined && validModes.includes(newConfig.animationMode)) {
    const licStatus = license.checkStatus(app.getPath('userData'));
    if (licStatus.status === 'active') {
      config.animationMode = newConfig.animationMode;
    }
  }
  if (newConfig.autoLaunch !== undefined) {
    autostart.setEnabled(!!newConfig.autoLaunch);
  }
  if (newConfig.customWebmDir !== undefined) {
    const licenseStatus = license.checkStatus(app.getPath('userData'));
    if (licenseStatus.status === 'active') {
      if (newConfig.customWebmDir) {
        try {
          const stat = fs.statSync(newConfig.customWebmDir);
          if (stat.isDirectory()) {
            config.customWebmDir = newConfig.customWebmDir;
            assets.generateReadmeInDir(newConfig.customWebmDir);
          }
        } catch (e) {}
      } else {
        config.customWebmDir = '';
      }
    }
  }
  if (newConfig.logEnabled !== undefined) {
    config.logEnabled = !!newConfig.logEnabled;
    logger.setEnabled(config.logEnabled);
  }
  configLib.save({ userConfigPath: USER_CONFIG_PATH, config, isDev: process.argv.includes('--dev') });
  if (messageNotify) messageNotify.updateConfig(config);
  messageBadgeCount = messageNotify ? messageNotify.getBadgeCount() : 0;
  updateTray();
  rebuildTrayMenu();
  logger.info('配置已保存: thresholdMinutes=' + config.thresholdMinutes + ', breakMinutes=' + config.breakMinutes + ', animationMode=' + config.animationMode);
  event.sender.send('save-config-result', { success: true });
});

ipcMain.on('close-settings', (event) => {
  const win = BrowserWindow.fromWebContents(event.sender);
  if (win && !win.isDestroyed()) win.close();
});

ipcMain.on('download-message-api-spec', async (event) => {
  const ownerWin = BrowserWindow.fromWebContents(event.sender);
  if (!ownerWin || ownerWin.isDestroyed()) return;
  try {
    const result = await dialog.showSaveDialog(ownerWin, {
      title: '下载消息提醒接口规范',
      defaultPath: 'PurrPause-消息提醒接口规范-v1.md',
      filters: [
        { name: 'Markdown', extensions: ['md'] },
        { name: 'All Files', extensions: ['*'] }
      ]
    });
    if (result.canceled || !result.filePath) {
      ownerWin.webContents.send('download-message-api-spec-result', { success: false, canceled: true });
      return;
    }
    const content = fs.readFileSync(MESSAGE_API_SPEC_PATH, 'utf-8');
    fs.writeFileSync(result.filePath, content);
    ownerWin.webContents.send('download-message-api-spec-result', { success: true, filePath: result.filePath });
  } catch (e) {
    ownerWin.webContents.send('download-message-api-spec-result', { success: false, error: '下载失败: ' + e.message });
  }
});

ipcMain.on('refresh-messages', () => {
  if (messageNotify) messageNotify.refreshList({ manual: true, suppressNotification: true }).catch(() => {});
});

ipcMain.on('open-message-target', (event, id) => {
  if (messageNotify) messageNotify.openMessageTarget(String(id || ''));
});

ipcMain.on('dismiss-message-notification', () => {
  if (messageNotify) messageNotify.dismissNotification();
});

ipcMain.on('test-message-connection', async (event, formConfig) => {
  const ownerWin = BrowserWindow.fromWebContents(event.sender);
  if (!ownerWin || ownerWin.isDestroyed()) return;
  try {
    const result = await createMessageNotify.testConnection(formConfig || {}, logger);
    if (ownerWin.isDestroyed()) return;
    ownerWin.webContents.send('test-message-connection-result', result);
  } catch (e) {
    if (ownerWin.isDestroyed()) return;
    ownerWin.webContents.send('test-message-connection-result', {
      ok: false,
      error: e && e.message ? e.message : String(e)
    });
  }
});

ipcMain.on('pick-webm-dir', async (event) => {
  const ownerWin = BrowserWindow.fromWebContents(event.sender);
  if (!ownerWin || ownerWin.isDestroyed()) return;
  const result = await dialog.showOpenDialog(ownerWin, {
    title: '选择素材目录',
    properties: ['openDirectory']
  });
  if (!result.canceled && result.filePaths.length > 0) {
    ownerWin.webContents.send('webm-dir-picked', result.filePaths[0]);
  }
});

function showActivationWindow(status) {
  if (activationWin) {
    activationWin.focus();
    return;
  }

  const display = screen.getPrimaryDisplay();
  const { width: sw, height: sh } = display.workArea;
  const winW = 540;
  const maxH = Math.round(sh * 0.8);

  activationWin = new BrowserWindow({
    x: Math.round(display.workArea.x + (sw - winW) / 2),
    y: Math.round(display.workArea.y + (sh - maxH) / 2),
    width: winW,
    height: maxH,
    show: false,
    resizable: true,
    minimizable: false,
    maximizable: false,
    frame: true,
    autoHideMenuBar: true,
    title: '激活 胖猫暂停一下（PurrPause）',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  activationWin.setMenuBarVisibility(false);
  activationWin.loadFile(path.join(__dirname, 'renderer', 'activation.html'));
  activationWin.webContents.on('did-finish-load', () => {
    activationWin.webContents.send('license-status', {
      ...status,
      machineId: license.getMachineId()
    });
    activationWin.webContents.executeJavaScript('document.body.scrollHeight').then((contentH) => {
      const frameExtra = activationWin.getSize()[1] - activationWin.getContentSize()[1];
      const winH = Math.min(contentH + frameExtra, maxH);
      activationWin.setSize(winW, winH);
      activationWin.setPosition(
        Math.round(display.workArea.x + (sw - winW) / 2),
        Math.round(display.workArea.y + (sh - winH) / 2)
      );
      activationWin.show();
    });
  });

  activationWin.on('closed', () => {
    activationWin = null;
    const currentStatus = license.checkStatus(app.getPath('userData'));
    if (currentStatus.status === 'expired') {
      app.quit();
    }
  });
}

ipcMain.on('activate', (event, serial) => {
  try {
    const result = license.activate(app.getPath('userData'), serial.trim());
    if (activationWin) {
      activationWin.webContents.send('activation-result', result);
    }
    if (result.success) {
      rebuildTrayMenu();
      if (!monitorInterval) startMonitoring();
      if (messageNotify) messageNotify.updateConfig(config);
    }
  } catch (e) {
    console.error('[purr-pause] Activation error:', e);
    if (activationWin) {
      activationWin.webContents.send('activation-result', { success: false, error: '激活失败: ' + e.message });
    }
  }
});

ipcMain.on('skip-activation', () => {
  const status = license.checkStatus(app.getPath('userData'));
  if (status.status === 'expired') return;
  if (activationWin) activationWin.close();
  if (!monitorInterval) startMonitoring();
});

app.on('window-all-closed', () => {
  // Keep app running — managed by tray
});

app.on('before-quit', () => {
  if (messageNotify) messageNotify.destroy();
  closeRocketDemo();
});

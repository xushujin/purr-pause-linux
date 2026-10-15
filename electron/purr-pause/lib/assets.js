const path = require('path');
const { pathToFileURL } = require('url');
const fs = require('fs');

// 资产文件名安全校验：只允许纯文件名、禁止路径分隔符（防目录穿越）。
// 纯函数，作为静态方法导出供 config.js 复用。
function isSafeAssetFilename(filename) {
  if (typeof filename !== 'string') return false;
  const value = filename.trim();
  return !!value && !/[\\/]/.test(value) && value === path.basename(value);
}

// 素材解析与目录说明文件生成。
// 通过工厂注入 resourcesPath（内置素材根）、userDataPath、defaultNotifyVideo（小火箭默认文件名）；
// config 一律以参数传入当前对象，模块内不缓存其引用。
function createAssets({ resourcesPath, userDataPath, defaultNotifyVideo }) {
  function getVideoPaths(config) {
    const customDir = config.customWebmDir || '';
    const userDir = path.join(userDataPath, 'webm');
    if (!fs.existsSync(userDir)) fs.mkdirSync(userDir, { recursive: true });
    const builtinDir = path.join(resourcesPath, 'webm');

    const walkFile = config.walkVideo || 'cat-walk.webm';
    const idleFile = config.idleVideo || 'cat-rest.webm';

    function findFile(filename) {
      if (customDir) {
        const customPath = path.join(customDir, filename);
        if (fs.existsSync(customPath)) return customPath;
      }
      const userPath = path.join(userDir, filename);
      if (fs.existsSync(userPath)) return userPath;
      return path.join(builtinDir, filename);
    }

    return {
      walk: findFile(walkFile),
      idle: findFile(idleFile)
    };
  }

  function findMediaFile(filename, config, preferredDirs = []) {
    if (!isSafeAssetFilename(filename)) return null;

    const customDir = config.customWebmDir || '';
    const userDir = path.join(userDataPath, 'webm');
    if (!fs.existsSync(userDir)) fs.mkdirSync(userDir, { recursive: true });
    const builtinDir = path.join(resourcesPath, 'webm');

    const searchDirs = [...preferredDirs, customDir, userDir, builtinDir].filter(Boolean);
    for (const dir of searchDirs) {
      const candidate = path.join(dir, filename);
      if (fs.existsSync(candidate)) return candidate;
    }

    return null;
  }

  function getRocketMediaUrl(filename, config) {
    const mediaPath = findMediaFile(filename || defaultNotifyVideo, config, [config.messageNotifyDir || '']);
    return mediaPath ? pathToFileURL(mediaPath).toString() : '';
  }

  function generateReadmeInDir(dir) {
    try {
      if (!fs.existsSync(dir)) return;
      const readmePath = path.join(dir, 'purr-pause-素材说明.txt');
      if (fs.existsSync(readmePath)) return;
      fs.writeFileSync(readmePath, [
        '=== 胖猫暂停一下（PurrPause） 自定义素材说明 ===',
        '',
        '将你的 .webm 视频文件放在此目录下即可替换内置素材。',
        '',
        '文件要求：',
        '  - 格式：WebM（VP9 编码，带 Alpha 通道实现透明背景）',
        '  - 背景：必须透明，否则会遮挡桌面',
        '  - 建议尺寸：宽高 200~500px',
        '',
        '需要提供两个文件：',
        '  - cat-walk.webm  → 猫走路动画（从右往左走，播放一次）',
        '  - cat-rest.webm  → 猫躺下/休息动画（循环播放）',
        '  - notify-rocket.webm → 小火箭消息提醒动画（可选，找不到时使用内置 SVG）',
        '',
        '如需使用其他文件名，请在设置中的 config.json 添加：',
        '  "walkVideo": "你的走路文件.webm"',
        '  "idleVideo": "你的躺下文件.webm"',
        '  "messageNotifyVideo": "你的小火箭文件.webm"',
        '',
        '制作建议：',
        '  - 使用 FFmpeg 导出带 Alpha 通道的 WebM：',
        '    ffmpeg -i input.mov -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 2M output.webm',
        '  - 走路动画建议 2~4 秒，猫从画面右侧走到左侧',
        '  - 休息动画可以是猫趴着、打呼噜等循环动作',
        '  - 小火箭动画建议 240x240 左右，透明背景，火箭主体保持在画面中间',
        ''
      ].join('\n'));
    } catch (e) {
      console.error('[purr-pause] Failed to generate readme:', e.message);
    }
  }

  function generateMessageNotifyReadmeInDir(dir) {
    try {
      if (!fs.existsSync(dir)) return;
      const readmePath = path.join(dir, 'purr-pause-消息提醒素材说明.txt');
      if (fs.existsSync(readmePath)) return;
      fs.writeFileSync(readmePath, [
        '=== 胖猫暂停一下（PurrPause） 消息提醒素材说明 ===',
        '',
        '将你的小火箭提醒 .webm 视频文件放在此目录下即可替换消息提醒动画素材。',
        '',
        '默认文件名：',
        '  - notify-rocket.webm → 小火箭消息提醒动画',
        '',
        '文件要求：',
        '  - 格式：WebM（VP9 编码，带 Alpha 通道实现透明背景）',
        '  - 背景：必须透明，否则会遮挡桌面',
        '  - 建议尺寸：240x240 左右',
        '  - 建议时长：2~4 秒，火箭主体保持在画面中间',
        '',
        '查找顺序：',
        '  1. 消息提醒设置中的“小火箭素材目录”',
        '  2. 通用素材目录',
        '  3. 用户默认素材目录 ~/.config/purr-pause/webm/',
        '  4. 应用内置素材目录',
        '',
        '如需使用其他文件名，请在“消息提醒设置”中修改“小火箭素材”。',
        '文件名只能填写文件名，例如 notify-rocket.webm，不能包含目录分隔符。',
        '',
        '制作建议：',
        '  - 使用 FFmpeg 导出带 Alpha 通道的 WebM：',
        '    ffmpeg -i input.mov -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 2M notify-rocket.webm',
        ''
      ].join('\n'));
    } catch (e) {
      console.error('[purr-pause] Failed to generate message notify readme:', e.message);
    }
  }

  return { getVideoPaths, findMediaFile, getRocketMediaUrl, generateReadmeInDir, generateMessageNotifyReadmeInDir };
}

createAssets.isSafeAssetFilename = isSafeAssetFilename;
module.exports = createAssets;

//! 配置管理：默认值定义、读写与取值校验。
//!
//! 与 `lib/config.js` 等价。配置文件位置保持兼容：
//!   用户配置：~/.local/share/purr-pause-linux/config.json
//!   回退内置：<安装目录>/config.json

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub threshold_minutes: i64,
    pub break_minutes: i64,
    #[serde(rename = "customWebmDir")]
    pub custom_webm_dir: String,
    #[serde(rename = "walkVideo")]
    pub walk_video: String,
    #[serde(rename = "idleVideo")]
    pub idle_video: String,
    #[serde(rename = "animationMode")]
    pub animation_mode: String,
    #[serde(rename = "logEnabled")]
    pub log_enabled: bool,
    #[serde(rename = "messageNotifyEnabled")]
    pub message_notify_enabled: bool,
    #[serde(rename = "messageSources")]
    pub message_sources: Vec<MessageSource>,
    #[serde(rename = "messagePollInterval")]
    pub message_poll_interval: i64,
    #[serde(rename = "messageMaxCacheItems")]
    pub message_max_cache_items: i64,
    #[serde(rename = "messageNotifyDir")]
    pub message_notify_dir: String,
    #[serde(rename = "messageNotifyVideo")]
    pub message_notify_video: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSource {
    pub id: String,
    pub name: String,
    #[serde(rename = "listUrl")]
    pub list_url: String,
    #[serde(rename = "authHeaders", default)]
    pub auth_headers: String,
    #[serde(default)]
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            threshold_minutes: 45,
            break_minutes: 5,
            custom_webm_dir: String::new(),
            walk_video: "cat-walk.webm".to_string(),
            idle_video: "cat-rest.webm".to_string(),
            animation_mode: "walk-center".to_string(),
            log_enabled: false,
            message_notify_enabled: false,
            message_sources: Vec::new(),
            message_poll_interval: 60,
            message_max_cache_items: 30,
            message_notify_dir: String::new(),
            message_notify_video: "notify-rocket.webm".to_string(),
        }
    }
}

/// 应用数据目录名。
pub const APP_DIR_NAME: &str = "purr-pause-linux";

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join(APP_DIR_NAME)
}

pub fn user_config_path() -> PathBuf {
    data_dir().join("config.json")
}

/// 素材查找顺序：自定义目录 → 用户 webm 目录 → 内置目录。
pub fn resolve_asset(filename: &str, config: &Config, builtin_dir: &Path) -> PathBuf {
    let user_dir = data_dir().join("webm");
    let _ = fs::create_dir_all(&user_dir);

    if !config.custom_webm_dir.is_empty() {
        let p = Path::new(&config.custom_webm_dir).join(filename);
        if p.exists() {
            return p;
        }
    }
    let p = user_dir.join(filename);
    if p.exists() {
        return p;
    }
    builtin_dir.join(filename)
}

/// 解析视频素材路径（走路 + 躺下）。
pub fn video_paths(config: &Config, builtin_dir: &Path) -> (PathBuf, PathBuf) {
    (
        resolve_asset(&config.walk_video, config, builtin_dir),
        resolve_asset(&config.idle_video, config, builtin_dir),
    )
}

/// 小火箭素材查找：消息目录 → 通用目录 → 用户 webm → 内置。
pub fn resolve_rocket(filename: &str, config: &Config, builtin_dir: &Path) -> Option<PathBuf> {
    let user_dir = data_dir().join("webm");
    let mut dirs: Vec<PathBuf> = Vec::new();
    if !config.message_notify_dir.is_empty() {
        dirs.push(PathBuf::from(&config.message_notify_dir));
    }
    if !config.custom_webm_dir.is_empty() {
        dirs.push(PathBuf::from(&config.custom_webm_dir));
    }
    dirs.push(user_dir);
    dirs.push(builtin_dir.to_path_buf());
    for d in dirs {
        let p = d.join(filename);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn load(builtin_config: &Path) -> Config {
    let user = user_config_path();
    let path = if user.exists() { user } else { builtin_config.to_path_buf() };
    let mut cfg = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Config>(&raw).unwrap_or_default(),
        Err(_) => Config::default(),
    };
    cfg.message_max_cache_items = clamp(cfg.message_max_cache_items, 1, 30, 30);
    cfg.message_poll_interval = clamp(cfg.message_poll_interval, 10, 3600, 60);
    cfg.threshold_minutes = clamp(cfg.threshold_minutes, 1, 480, 45);
    cfg.break_minutes = clamp(cfg.break_minutes, 1, 60, 5);
    cfg
}

pub fn save(config: &Config) -> std::io::Result<()> {
    let p = user_config_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    fs::write(p, json)
}

pub fn clamp(v: i64, min: i64, max: i64, fallback: i64) -> i64 {
    let n = parse_int(v);
    match n {
        Some(x) => x.clamp(min, max),
        None => fallback,
    }
}

/// i64 → parseInt 语义：无法解析则 None。
pub fn parse_int(v: i64) -> Option<i64> {
    Some(v)
}
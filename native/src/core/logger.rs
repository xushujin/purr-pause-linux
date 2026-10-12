//! 滚动日志（与 lib/logger.js 等价）：单文件 128KB × 5。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_SIZE: u64 = 128 * 1024;
const MAX_LOG_FILES: usize = 5;

pub struct Logger {
    dir: Option<PathBuf>,
    enabled: bool,
    lock: Mutex<()>,
}

impl Logger {
    pub fn new(dir: PathBuf, enabled: bool) -> Self {
        Self {
            dir: Some(dir),
            enabled,
            lock: Mutex::new(()),
        }
    }

    fn log_file(&self) -> PathBuf {
        self.dir
            .as_ref()
            .map(|d| d.join("purr-pause-linux.log"))
            .unwrap_or_default()
    }

    pub fn path(&self) -> PathBuf {
        self.log_file()
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn rotate_if_needed(&self) {
        let log_file = self.log_file();
        let Ok(meta) = fs::metadata(&log_file) else { return };
        if meta.len() < MAX_LOG_SIZE {
            return;
        }
        let Some(dir) = &self.dir else { return };
        for i in (1..MAX_LOG_FILES).rev() {
            let older = dir.join(format!("purr-pause-linux.{i}.log"));
            let newer = if i == 1 {
                log_file.clone()
            } else {
                dir.join(format!("purr-pause-linux.{}.log", i - 1))
            };
            if newer.exists() {
                let _ = fs::rename(&newer, &older);
            }
        }
    }

    pub fn write(&self, level: &str, msg: &str) {
        if !self.enabled {
            return;
        }
        let _guard = self.lock.lock();
        let Some(dir) = &self.dir else { return };
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
        self.rotate_if_needed();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(self.log_file()) {
            let _ = writeln!(f, "[{}] [{}] {}", now_str(), level, msg);
        }
    }

    pub fn info(&self, msg: &str) {
        self.write("INFO", msg);
    }
    pub fn warn(&self, msg: &str) {
        self.write("WARN", msg);
    }
    pub fn error(&self, msg: &str) {
        self.write("ERROR", msg);
    }
}

fn now_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y, m, d, sod / 3600, (sod % 3600) / 60, sod % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

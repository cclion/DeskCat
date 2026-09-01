// DeskCat — AI 感知型 macOS 桌面伙伴
// Copyright (C) 2026 DeskCat contributors
//
// 本程序是自由软件:你可以依据自由软件基金会发布的 GNU 通用公共许可证第三版
// (或你选择的任何更新版本)的条款重新分发和/或修改它。
//
// 分发本程序是希望它有用,但**不作任何担保**;甚至不含适销性或特定用途适用性
// 的默示担保。详见 GNU 通用公共许可证。
//
// 你应当已随本程序收到一份 GNU 通用公共许可证副本(见 LICENSE 文件);
// 若没有,请见 <https://www.gnu.org/licenses/>。

//! 配置持久化:~/Library/Application Support/DeskCat/config.json
//! 铁律:损坏不致命(回退默认并重建)、未知字段保留(前向兼容)、原子写。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DEFAULT_SIZE: u32 = 130;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// 形象窗口左上角位置(逻辑点);None = 用默认右下角
    pub pos_x: Option<i32>,
    pub pos_y: Option<i32>,
    /// 形象显示边长(点)
    pub size: u32,
    pub autostart: bool,
    pub hide_on_fullscreen: bool,
    pub chime: bool,
    pub bubble_remark: bool,
    pub input_sensing: bool,
    /// 无操作多久判定离开(分钟)
    pub away_minutes: u32,
    /// busy 无新事件多久回落 idle(分钟)
    pub idle_minutes: u32,
    pub pack_id: String,
    pub first_run: bool,
    pub hidden: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            pos_x: None,
            pos_y: None,
            size: DEFAULT_SIZE,
            autostart: false,
            hide_on_fullscreen: true,
            chime: true,
            bubble_remark: false,
            input_sensing: true,
            away_minutes: 3,
            idle_minutes: 10,
            pack_id: "calico".into(),
            first_run: true,
            hidden: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/Application Support/DeskCat")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// 已知字段解析成结构体;整份 JSON 也留一份,写回时保留未知字段
pub struct Store {
    path: PathBuf,
    inner: Mutex<(Config, Map<String, Value>)>,
}

fn load_raw(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => m,
        // 损坏或顶层非对象:回退默认,不致命
        _ => Map::new(),
    }
}

/// 逐字段解析:某字段类型不符时只回退该字段,其余保留
fn parse_lenient(raw: &Map<String, Value>) -> Config {
    let mut cfg = Config::default();
    let d = Config::default();
    macro_rules! take {
        ($field:ident, $key:literal) => {
            if let Some(v) = raw.get($key) {
                cfg.$field = serde_json::from_value(v.clone()).unwrap_or(d.$field.clone());
            }
        };
    }
    take!(pos_x, "pos_x");
    take!(pos_y, "pos_y");
    take!(size, "size");
    take!(autostart, "autostart");
    take!(hide_on_fullscreen, "hide_on_fullscreen");
    take!(chime, "chime");
    take!(bubble_remark, "bubble_remark");
    take!(input_sensing, "input_sensing");
    take!(away_minutes, "away_minutes");
    take!(idle_minutes, "idle_minutes");
    take!(pack_id, "pack_id");
    take!(first_run, "first_run");
    take!(hidden, "hidden");
    cfg.clamp();
    cfg
}

impl Config {
    /// 数值兜底,防止手改配置把应用弄进不可用状态
    fn clamp(&mut self) {
        self.size = self.size.clamp(60, 220);
        self.away_minutes = self.away_minutes.clamp(1, 120);
        self.idle_minutes = self.idle_minutes.clamp(1, 120);
        if self.pack_id.trim().is_empty() {
            self.pack_id = "calico".into();
        }
    }
}

impl Store {
    pub fn load() -> Self {
        let path = config_path();
        let raw = load_raw(&path);
        let cfg = parse_lenient(&raw);
        let store = Self {
            path,
            inner: Mutex::new((cfg, raw)),
        };
        // 首次启动 / 损坏重建:立刻落盘一份完整默认配置
        let _ = store.persist();
        store
    }

    pub fn get(&self) -> Config {
        self.inner.lock().unwrap().0.clone()
    }

    /// 单字段写入;key 未知则拒绝,值类型不符则拒绝(不静默吞掉)
    pub fn set(&self, key: &str, value: Value) -> Result<Config, String> {
        let mut guard = self.inner.lock().unwrap();
        let (cfg, raw) = &mut *guard;
        let mut probe = raw.clone();
        probe.insert(key.to_string(), value.clone());
        let as_value = Value::Object(probe.clone());
        // 用严格解析验证这个字段能被接受
        let known: Config = serde_json::from_value(as_value).map_err(|e| format!("配置写入失败({key}): {e}"))?;
        if serde_json::to_value(&Config::default())
            .ok()
            .and_then(|d| d.as_object().map(|o| !o.contains_key(key)))
            .unwrap_or(true)
        {
            return Err(format!("未知配置项: {key}"));
        }
        let mut known = known;
        known.clamp();
        *cfg = known.clone();
        *raw = probe;
        drop(guard);
        self.persist()?;
        Ok(known)
    }

    fn persist(&self) -> Result<(), String> {
        let guard = self.inner.lock().unwrap();
        let (cfg, raw) = &*guard;
        // 已知字段以结构体为准,未知字段原样保留
        let mut out = raw.clone();
        if let Ok(Value::Object(known)) = serde_json::to_value(cfg) {
            for (k, v) in known {
                out.insert(k, v);
            }
        }
        let text = serde_json::to_string_pretty(&Value::Object(out)).map_err(|e| e.to_string())?;
        drop(guard);

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("写配置失败: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("替换配置失败: {e}"))?;
        Ok(())
    }
}

// ---------- Tauri 命令 ----------

#[tauri::command]
pub fn get_config(store: tauri::State<'_, Store>) -> Config {
    store.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(name: &str, content: Option<&str>) -> Store {
        let dir = std::env::temp_dir().join(format!("deskcat-cfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        if let Some(c) = content {
            std::fs::write(&path, c).unwrap();
        }
        let raw = load_raw(&path);
        let cfg = parse_lenient(&raw);
        let s = Store { path, inner: Mutex::new((cfg, raw)) };
        s.persist().unwrap();
        s
    }

    #[test]
    fn defaults_match_requirements() {
        let c = Config::default();
        assert!(!c.autostart, "开机自启默认关(07 §7-8)");
        assert!(c.hide_on_fullscreen, "全屏隐藏默认开");
        assert!(c.chime, "提示音默认开");
        assert!(!c.bubble_remark, "气泡备注默认关(07 §7-2)");
        assert!(c.input_sensing, "键鼠感知默认开");
        assert_eq!(c.away_minutes, 3, "判定离开默认 3 分钟(07 §7-4)");
        assert_eq!(c.idle_minutes, 10, "判定闲置默认 10 分钟(07 §7-3)");
        assert_eq!(c.pack_id, "calico");
        assert!(c.first_run);
    }

    #[test]
    fn fresh_start_creates_file() {
        let s = tmp_store("fresh", None);
        assert!(s.path.exists());
        assert_eq!(s.get(), Config::default());
    }

    #[test]
    fn corrupt_file_falls_back_and_rebuilds() {
        let s = tmp_store("corrupt", Some("{broken json"));
        assert_eq!(s.get(), Config::default());
        let text = std::fs::read_to_string(&s.path).unwrap();
        serde_json::from_str::<Value>(&text).expect("应已重建为合法 JSON");
    }

    #[test]
    fn unknown_fields_preserved() {
        let s = tmp_store("unknown", Some(r#"{"future_flag": 123, "size": 88}"#));
        assert_eq!(s.get().size, 88);
        s.set("chime", Value::Bool(false)).unwrap();
        let text = std::fs::read_to_string(&s.path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["future_flag"], 123, "未知字段应保留");
        assert_eq!(v["chime"], false);
        assert_eq!(v["size"], 88);
    }

    #[test]
    fn wrong_type_field_falls_back_only_itself() {
        let s = tmp_store("wrongtype", Some(r#"{"size": "big", "chime": false}"#));
        let c = s.get();
        assert_eq!(c.size, DEFAULT_SIZE, "类型不符的字段回退默认");
        assert!(!c.chime, "其余字段保留");
    }

    #[test]
    fn set_persists_immediately() {
        let s = tmp_store("persist", None);
        s.set("size", Value::from(180)).unwrap();
        let text = std::fs::read_to_string(&s.path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["size"], 180);
    }

    #[test]
    fn set_rejects_unknown_key_and_bad_type() {
        let s = tmp_store("reject", None);
        assert!(s.set("nope", Value::Bool(true)).is_err(), "未知 key 应拒绝");
        assert!(s.set("size", Value::String("big".into())).is_err(), "类型不符应拒绝");
        assert_eq!(s.get(), Config::default(), "拒绝的写入不应改变状态");
    }

    #[test]
    fn values_are_clamped() {
        let s = tmp_store("clamp", Some(r#"{"size": 9999, "away_minutes": 0}"#));
        let c = s.get();
        assert_eq!(c.size, 220);
        assert_eq!(c.away_minutes, 1);
    }
}

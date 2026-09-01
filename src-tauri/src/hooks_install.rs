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

//! Claude Code hooks 安装/卸载:merge 写入 ~/.claude/settings.json。
//! 铁律:绝不覆盖用户已有条目;写入前备份;损坏 JSON 拒绝写入;原子写(临时文件 + rename)。
#![allow(dead_code)] // P2 接入「一键连接」前,仅单元测试使用

use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

/// 识别自己条目的标记:命令里含此 URL 即视为 DeskCat 安装的 hook
const MARKER: &str = "127.0.0.1:43917/deskcat";

const EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

fn hook_command() -> String {
    // -m 1:最多等 1s,DeskCat 未启动时不拖慢 Claude Code;|| true:永不让 hook 报错
    format!(
        "curl -s -m 1 -X POST http://{MARKER} -H 'Content-Type: application/json' --data-binary @- >/dev/null 2>&1 || true"
    )
}

fn load(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("读取 settings.json 失败: {e}"))?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("settings.json 顶层不是对象,拒绝写入".into()),
        Err(e) => Err(format!("settings.json 解析失败,拒绝写入: {e}")),
    }
}

fn backup(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let bak = path.with_extension(format!("json.deskcat-backup-{ts}"));
    fs::copy(path, &bak).map_err(|e| format!("备份失败,中止写入: {e}"))?;
    Ok(())
}

fn atomic_write(path: &Path, root: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(root.clone()))
        .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.deskcat-tmp");
    fs::write(&tmp, text).map_err(|e| format!("写临时文件失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("原子替换失败: {e}"))?;
    Ok(())
}

fn entry_is_ours(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(MARKER))
            })
        })
        .unwrap_or(false)
}

/// 安装:七个事件各追加一条 DeskCat 条目。幂等;返回是否发生写入。
pub fn install(path: &Path) -> Result<bool, String> {
    let mut root = load(path)?;
    let hooks_val = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks_val
        .as_object_mut()
        .ok_or("hooks 字段不是对象,拒绝写入")?;

    let mut changed = false;
    for ev in EVENTS {
        let arr_val = hooks.entry(ev).or_insert_with(|| json!([]));
        let arr = arr_val
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{ev} 不是数组,拒绝写入"))?;
        if !arr.iter().any(entry_is_ours) {
            arr.push(json!({ "hooks": [{ "type": "command", "command": hook_command() }] }));
            changed = true;
        }
    }
    if changed {
        backup(path)?;
        atomic_write(path, &root)?;
    }
    Ok(changed)
}

/// 卸载:仅移除带 MARKER 的条目;清理因此变空的数组/对象;别人的分毫不动。
pub fn uninstall(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut root = load(path)?;
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(false);
    };

    let mut changed = false;
    let mut empty_events = Vec::new();
    for (ev, arr_val) in hooks.iter_mut() {
        if let Some(arr) = arr_val.as_array_mut() {
            let before = arr.len();
            arr.retain(|e| !entry_is_ours(e));
            if arr.len() != before {
                changed = true;
            }
            if arr.is_empty() {
                empty_events.push(ev.clone());
            }
        }
    }
    if changed {
        for ev in empty_events {
            hooks.remove(&ev);
        }
        if hooks.is_empty() {
            root.remove("hooks");
        }
        backup(path)?;
        atomic_write(path, &root)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskcat-hooks-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    fn read_json(p: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn fresh_install_creates_valid_file() {
        let p = tmp("fresh");
        assert!(install(&p).unwrap());
        let v = read_json(&p);
        for ev in EVENTS {
            let arr = v["hooks"][ev].as_array().unwrap();
            assert_eq!(arr.len(), 1);
            assert!(entry_is_ours(&arr[0]));
        }
    }

    #[test]
    fn merge_preserves_existing_entries_and_fields() {
        let p = tmp("merge");
        fs::write(
            &p,
            r#"{
  "model": "opus",
  "statusLine": { "type": "command", "command": "my-status" },
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "other-tool-guard" }] }
    ]
  }
}"#,
        )
        .unwrap();
        assert!(install(&p).unwrap());
        let v = read_json(&p);
        assert_eq!(v["model"], "opus");
        assert_eq!(v["statusLine"]["command"], "my-status");
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["matcher"], "Bash");
        assert_eq!(pre[0]["hooks"][0]["command"], "other-tool-guard");
        assert!(entry_is_ours(&pre[1]));
    }

    #[test]
    fn uninstall_restores_semantics() {
        let p = tmp("uninstall");
        let original = r#"{
  "model": "opus",
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "other-tool-guard" }] }
    ]
  }
}"#;
        fs::write(&p, original).unwrap();
        install(&p).unwrap();
        assert!(uninstall(&p).unwrap());
        let after = read_json(&p);
        let before: Value = serde_json::from_str(original).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn uninstall_after_fresh_install_leaves_clean_object() {
        let p = tmp("uninstall-fresh");
        install(&p).unwrap();
        uninstall(&p).unwrap();
        let v = read_json(&p);
        assert!(v.get("hooks").is_none());
    }

    #[test]
    fn backup_created_on_install() {
        let p = tmp("backup");
        fs::write(&p, r#"{"model":"opus"}"#).unwrap();
        install(&p).unwrap();
        let dir = p.parent().unwrap();
        let baks: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("deskcat-backup"))
            .collect();
        assert_eq!(baks.len(), 1);
        let bak: Value =
            serde_json::from_str(&fs::read_to_string(baks[0].path()).unwrap()).unwrap();
        assert_eq!(bak, serde_json::json!({"model":"opus"}));
    }

    #[test]
    fn corrupt_json_refused_and_untouched() {
        let p = tmp("corrupt");
        fs::write(&p, "{invalid json").unwrap();
        assert!(install(&p).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "{invalid json");
        assert!(!p.with_extension("json.deskcat-tmp").exists());
    }

    #[test]
    fn install_is_idempotent() {
        let p = tmp("idempotent");
        install(&p).unwrap();
        let first = fs::read_to_string(&p).unwrap();
        assert!(!install(&p).unwrap());
        assert!(!install(&p).unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), first);
    }
}

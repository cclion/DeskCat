//! ClaudeCodeHookSource:本地 HTTP server 接收 Claude Code 的 hook 回调,
//! 翻译成语义事件投递到总线。事件源不碰状态机(架构硬规则)。
//!
//! 安全边界:只绑回环、只收 POST、body 限 8KB、解析失败静默丢弃。

use crate::state_machine::Event;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    /// 等你批准某个操作(最紧迫:不点它就一直卡着)
    Permission,
    /// 只是等你回话
    Input,
}

/// Claude Code 的 Notification 有多种;含这些词的才是"等你批准"
fn is_permission_request(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["permission", "approve", "allow", "confirm", "授权", "批准", "允许"]
        .iter()
        .any(|k| m.contains(k))
}

pub const PORT: u16 = 43917;
pub const PATH: &str = "/deskcat";
const MAX_BODY: usize = 8 * 1024;

/// hook 事件名 → 语义事件。未知事件名返回 None(静默丢弃)。
pub fn map_hook(payload: &Value) -> Option<Event> {
    let name = payload.get("hook_event_name")?.as_str()?;
    // Claude Code 的 session_id 才是唯一标识:同一个项目目录下可以同时开多个会话,
    // 拿目录名当 key 会把它们合并成一个,"N 个会话活跃"就永远显示 1。
    let id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        // 老版本 hook 没有 session_id 时退回 cwd,至少按项目区分
        .or_else(|| payload.get("cwd").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    // cwd 的目录名只作展示(气泡要点名"哪个项目在等你")
    let session = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .and_then(|p| p.rsplit('/').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("Claude Code")
        .to_string();
    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Some(match name {
        "SessionStart" => Event::SessionStart { id, session },
        "UserPromptSubmit" => Event::Busy { id, session, action: None },
        "PreToolUse" | "PostToolUse" => Event::Busy { id, session, action: tool },
        // Notification = Claude 在等用户——本产品最有用的一刻。
        // 但它有两类:等你批准某个操作 / 只是等你回话,紧迫度不同,文案要分开。
        "Notification" => {
            let msg = payload.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let kind = if is_permission_request(msg) {
                WaitKind::Permission
            } else {
                WaitKind::Input
            };
            let detail = match (kind, tool.as_deref()) {
                (WaitKind::Permission, Some(t)) => Some(t.to_string()),
                _ if !msg.is_empty() => Some(msg.to_string()),
                _ => tool,
            };
            Event::Waiting { id, session, detail, permission: kind == WaitKind::Permission }
        }
        // Stop 带错误标记时是"出错了",不是"跑完了"
        "Stop" => {
            let errored = payload
                .get("error")
                .map(|v| !v.is_null())
                .unwrap_or(false);
            if errored {
                let detail = payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                Event::Alert { id, session, detail }
            } else {
                Event::Done { id, session }
            }
        }
        "SessionEnd" => Event::SessionEnd { id },
        _ => return None,
    })
}

/// 读一个极简 HTTP 请求;只认回环来的 POST <PATH>。
fn handle(stream: TcpStream, tx: &Sender<Event>) {
    // 非回环来源直接拒绝
    if !stream
        .peer_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false)
    {
        return;
    }
    // 本地短请求,1 秒足够;半开连接不会长期占着线程
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    let mut reader = BufReader::new(&stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    // 读 headers,取 Content-Length
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            Err(_) => return,
        }
    }

    let ok = method == "POST" && path.starts_with(PATH) && len <= MAX_BODY;
    if ok && len > 0 {
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_ok() {
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                if let Some(ev) = map_hook(&v) {
                    let _ = tx.send(ev);
                }
            }
            // 解析失败静默丢弃:绝不能因为一条坏 payload 影响 Claude Code
        }
    }

    // 无论如何都快速回一个 204,别让 Claude Code 等
    let mut s = stream;
    let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = s.flush();
}

/// 同时在处理的连接数上限。Claude Code 的 hook 是短平快的 POST,
/// 正常并发远低于此;超限直接丢连接,宁可漏一个事件也不让线程失控。
const MAX_INFLIGHT: usize = 32;

/// 启动监听。端口被占用时返回 Err,调用方降级为"未连接",应用其余功能照常。
///
/// 每个连接单独起线程:accept 循环绝不能被单个慢连接堵住——
/// 那会让后面所有 hook 事件排队,猫的反应延迟甚至丢事件。
pub fn start(tx: Sender<Event>) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", PORT))
        .map_err(|e| format!("端口 {PORT} 无法监听: {e}"))?;
    let inflight = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
                drop(stream); // 过载:直接断开,不排队
                continue;
            }
            inflight.fetch_add(1, Ordering::Relaxed);
            let tx = tx.clone();
            let inflight = inflight.clone();
            // 单个连接慢/半开都只影响它自己
            let _ = std::thread::Builder::new()
                .stack_size(64 * 1024)
                .spawn(move || {
                    handle(stream, &tx);
                    inflight.fetch_sub(1, Ordering::Relaxed);
                });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(name: &str, cwd: &str) -> Value {
        json!({ "hook_event_name": name, "cwd": cwd })
    }

    #[test]
    fn maps_all_seven_hooks() {
        assert!(matches!(
            map_hook(&ev("SessionStart", "/a/deskcat")),
            Some(Event::SessionStart { .. })
        ));
        assert!(matches!(
            map_hook(&ev("UserPromptSubmit", "/a/deskcat")),
            Some(Event::Busy { .. })
        ));
        assert!(matches!(map_hook(&ev("PreToolUse", "/a/x")), Some(Event::Busy { .. })));
        assert!(matches!(map_hook(&ev("PostToolUse", "/a/x")), Some(Event::Busy { .. })));
        assert!(matches!(map_hook(&ev("Notification", "/a/x")), Some(Event::Waiting { .. })));
        assert!(matches!(map_hook(&ev("Stop", "/a/x")), Some(Event::Done { .. })));
        assert!(matches!(map_hook(&ev("SessionEnd", "/a/x")), Some(Event::SessionEnd { .. })));
    }

    #[test]
    fn session_id_is_project_dir_name() {
        let e = map_hook(&ev("Notification", "/Users/grayson/Desktop/DeskCat")).unwrap();
        match e {
            Event::Waiting { session, .. } => assert_eq!(session, "DeskCat"),
            _ => panic!("应为 Waiting"),
        }
    }

    #[test]
    fn missing_cwd_falls_back() {
        let e = map_hook(&json!({ "hook_event_name": "Stop" })).unwrap();
        match e {
            Event::Done { session, .. } => assert_eq!(session, "Claude Code"),
            _ => panic!(),
        }
    }

    #[test]
    fn tool_name_becomes_detail() {
        let e = map_hook(&json!({
            "hook_event_name": "PreToolUse", "cwd": "/a/x", "tool_name": "Bash"
        }))
        .unwrap();
        match e {
            Event::Busy { action, .. } => assert_eq!(action.as_deref(), Some("Bash")),
            _ => panic!(),
        }
    }

    #[test]
    fn permission_request_shows_tool_name() {
        // 等批权限时,"哪个工具要跑"比一句泛泛的提示更有用
        let e = map_hook(&json!({
            "hook_event_name": "Notification", "cwd": "/a/x",
            "tool_name": "Bash", "message": "Claude needs your permission to use Bash"
        }))
        .unwrap();
        match e {
            Event::Waiting { detail, permission, .. } => {
                assert!(permission, "应识别为等批权限");
                assert_eq!(detail.as_deref(), Some("Bash"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn plain_input_notification_is_not_permission() {
        // Claude Code 空闲提醒:只是等你回话,不是卡在权限上
        let e = map_hook(&json!({
            "hook_event_name": "Notification", "cwd": "/a/x",
            "message": "Claude is waiting for your input"
        }))
        .unwrap();
        match e {
            Event::Waiting { detail, permission, .. } => {
                assert!(!permission, "等你回话不该按'等批权限'报");
                assert_eq!(detail.as_deref(), Some("Claude is waiting for your input"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn permission_keywords_are_recognised() {
        for m in ["needs your permission", "Approve this action?", "允许执行吗", "请批准"] {
            assert!(is_permission_request(m), "应识别: {m}");
        }
        for m in ["Claude is waiting for your input", "task finished"] {
            assert!(!is_permission_request(m), "不应误判: {m}");
        }
    }

    #[test]
    fn session_id_is_the_key_not_the_directory() {
        // 同一个目录、两个不同 session_id → 必须是两个不同的 id
        let a = map_hook(&json!({
            "hook_event_name": "PreToolUse", "cwd": "/p/DeskCat", "session_id": "s-1"
        }))
        .unwrap();
        let b = map_hook(&json!({
            "hook_event_name": "PreToolUse", "cwd": "/p/DeskCat", "session_id": "s-2"
        }))
        .unwrap();
        let idof = |e: &Event| match e {
            Event::Busy { id, .. } => id.clone(),
            _ => panic!(),
        };
        assert_ne!(idof(&a), idof(&b), "同目录不同会话必须区分开");
        // 展示标签仍是目录名
        match &a {
            Event::Busy { session, .. } => assert_eq!(session, "DeskCat"),
            _ => panic!(),
        }
    }

    #[test]
    fn falls_back_to_cwd_when_no_session_id() {
        let e = map_hook(&json!({ "hook_event_name": "Stop", "cwd": "/p/x" })).unwrap();
        match e {
            Event::Done { id, .. } => assert_eq!(id, "/p/x"),
            _ => panic!(),
        }
    }

    #[test]
    fn unknown_and_malformed_are_dropped() {
        assert!(map_hook(&ev("SomethingElse", "/a/x")).is_none());
        assert!(map_hook(&json!({ "cwd": "/a/x" })).is_none(), "缺 hook_event_name 应丢弃");
        assert!(map_hook(&json!({ "hook_event_name": 42 })).is_none(), "类型不符应丢弃");
        assert!(map_hook(&json!([])).is_none(), "非对象应丢弃");
    }
}

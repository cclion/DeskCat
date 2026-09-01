//! 前后端胶水:形象包枚举、窗口显隐、设置窗口、开机自启、连接状态、命令集合。

use crate::config::Store;
use crate::hooks_install;
use crate::packs::{self, Pack};
use crate::pet_window;
use crate::sources::claude_hook;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// 连接状态(设置窗口与菜单栏共用)
#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub installed: bool,
    pub listening: bool,
    pub sessions: usize,
    pub settings_path: String,
}

pub fn claude_settings_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".claude/settings.json")
}

/// 从嵌入的前端资源里读一个文件。
/// 形象包与前端一起打包,**不能**依赖运行时扫描目录——那样换台机器就找不到。
fn read_asset<R: Runtime>(app: &AppHandle<R>, path: &str) -> Option<String> {
    let a = app.asset_resolver().get(path.to_string())?;
    String::from_utf8(a.bytes).ok()
}

/// 枚举可用形象包(清单缺失/包损坏都跳过,不崩溃)
pub fn list_packs<R: Runtime>(app: &AppHandle<R>) -> Vec<Pack> {
    let Some(index) = read_asset(app, "/packs/index.json") else {
        return Vec::new();
    };
    let ids: Vec<String> = serde_json::from_str(&index).unwrap_or_default();
    let mut out: Vec<Pack> = ids
        .iter()
        .filter_map(|id| {
            let text = read_asset(app, &format!("/packs/{id}/pack.json"))?;
            let p: Pack = serde_json::from_str(&text).ok()?;
            packs::is_valid(&p).then_some(p)
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn connection_label<R: Runtime>(app: &AppHandle<R>) -> String {
    let c = connection_state(app);
    if c.installed && c.listening {
        match c.sessions {
            0 => "Claude Code 已连接".into(),
            n => format!("Claude Code 已连接 · {n} 个会话"),
        }
    } else {
        "Claude Code 未连接 · 点击连接".into()
    }
}

pub fn connection_state<R: Runtime>(app: &AppHandle<R>) -> Connection {
    let path = claude_settings_path();
    let installed = std::fs::read_to_string(&path)
        .map(|t| t.contains(claude_hook::PATH) && t.contains(&claude_hook::PORT.to_string()))
        .unwrap_or(false);
    let sessions = app
        .try_state::<crate::bus::Bus>()
        .map(|b| b.machine.lock().unwrap().active_sessions())
        .unwrap_or(0);
    Connection {
        installed,
        listening: app.try_state::<Listening>().map(|l| l.0).unwrap_or(false),
        sessions,
        settings_path: path.display().to_string(),
    }
}

/// HTTP 监听是否成功(端口被占用时为 false,降级为未连接)
pub struct Listening(pub bool);

pub fn apply_visibility<R: Runtime>(app: &AppHandle<R>) {
    let store = app.state::<Store>();
    let hidden = store.get().hidden || app.try_state::<Fullscreen>().map(|f| f.hidden()).unwrap_or(false);
    if let Some(win) = app.get_webview_window("pet") {
        if hidden {
            let _ = win.hide();
        } else {
            let _ = win.show();
        }
        let _ = win.emit("visibility", !hidden);
    }
}

/// 全屏隐藏状态(P4)
#[derive(Default)]
pub struct Fullscreen(pub std::sync::atomic::AtomicBool);
impl Fullscreen {
    pub fn hidden(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set(&self, v: bool) {
        self.0.store(v, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn open_settings<R: Runtime>(app: &AppHandle<R>, page: Option<&str>) {
    let Some(win) = app.get_webview_window("settings") else {
        return;
    };
    let first_open = !win.is_visible().unwrap_or(false);
    // 先 show 再定位:窗口隐藏时定位不生效
    let _ = win.show();
    let _ = win.unminimize();
    // 首次打开时落在"猫所在的那块屏"——用户的注意力在那儿;
    // 之后保留用户自己挪过的位置,除非它已经不在任何显示器上了
    if first_open || !settings_position_visible(&win) {
        center_near_pet(app, &win);
    }
    let _ = win.set_focus();
    if let Some(p) = page {
        let _ = win.emit("goto-page", p);
    }
}

/// 在形象窗口所在的显示器上居中
fn center_near_pet<R: Runtime>(app: &AppHandle<R>, win: &tauri::WebviewWindow<R>) {
    let Ok(size) = win.outer_size() else { return };
    let wsf = win.scale_factor().unwrap_or(1.0);
    let (w, h) = (size.width as f64 / wsf, size.height as f64 / wsf);

    // 优先用形象窗口当前所在的屏;取不到就退回主屏
    let target = app
        .get_webview_window("pet")
        .and_then(|p| p.current_monitor().ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(mon) = target else { return };

    let sf = mon.scale_factor();
    let ms = mon.size().to_logical::<f64>(sf);
    let mp = mon.position().to_logical::<f64>(sf);
    let _ = win.set_position(tauri::LogicalPosition::new(
        mp.x + (ms.width - w) / 2.0,
        mp.y + (ms.height - h) / 2.0,
    ));
}

/// 设置窗口是否还落在某块显示器里(要求标题栏区域可见,否则用户拖不动它)
fn settings_position_visible<R: Runtime>(win: &tauri::WebviewWindow<R>) -> bool {
    let (Ok(pos), Ok(size), Ok(monitors)) =
        (win.outer_position(), win.outer_size(), win.available_monitors())
    else {
        return false;
    };
    let sf = win.scale_factor().unwrap_or(1.0);
    let (x, y) = (pos.x as f64 / sf, pos.y as f64 / sf);
    let (w, h) = (size.width as f64 / sf, size.height as f64 / sf);
    monitors.iter().any(|m| {
        let ms = m.size().to_logical::<f64>(m.scale_factor());
        let mp = m.position().to_logical::<f64>(m.scale_factor());
        // 顶部拖拽区必须在屏内,且横向至少露出一半
        y >= mp.y
            && y + 52.0 <= mp.y + ms.height
            && x + w / 2.0 >= mp.x
            && x + w / 2.0 <= mp.x + ms.width
            && h > 0.0
    })
}

/// 配置变化 → 通知所有窗口重新拉配置
pub fn notify_config_changed<R: Runtime>(app: &AppHandle<R>) {
    let store = app.state::<Store>();
    let _ = app.emit("config-changed", store.get());
}

// ---------- 开机自启(LaunchAgent) ----------

fn launch_agent_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Library/LaunchAgents/app.deskcat.desktop.plist")
}

pub fn apply_autostart(on: bool) -> Result<(), String> {
    let path = launch_agent_path();
    if !on {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("移除自启失败: {e}"))?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("定位程序失败: {e}"))?;
    // 打包后 exe 在 DeskCat.app/Contents/MacOS/,自启应拉起 .app 而非裸二进制
    let target = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
        .unwrap_or(exe);
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>app.deskcat.desktop</string>
  <key>ProgramArguments</key>
  <array><string>/usr/bin/open</string><string>-a</string><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
        target.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 LaunchAgents 失败: {e}"))?;
    }
    std::fs::write(&path, plist).map_err(|e| format!("写入自启失败: {e}"))?;
    Ok(())
}

// ---------- Tauri 命令 ----------

#[tauri::command]
pub fn get_packs(app: AppHandle) -> Vec<Pack> {
    list_packs(&app)
}

#[tauri::command]
pub fn get_connection(app: AppHandle) -> Connection {
    connection_state(&app)
}

#[tauri::command]
pub fn install_hooks(app: AppHandle) -> Result<Connection, String> {
    hooks_install::install(&claude_settings_path())?;
    crate::tray::refresh(&app);
    // 连上了给个反应(需求 3.4 AC3)
    if let Some(bus) = app.try_state::<crate::bus::Bus>() {
        bus.post(crate::state_machine::Event::SessionStart {
            id: "__connected__".into(),
            session: "已连接".into(),
        });
        bus.post(crate::state_machine::Event::SessionEnd {
            id: "__connected__".into(),
        });
    }
    Ok(connection_state(&app))
}

#[tauri::command]
pub fn uninstall_hooks(app: AppHandle) -> Result<Connection, String> {
    hooks_install::uninstall(&claude_settings_path())?;
    crate::tray::refresh(&app);
    Ok(connection_state(&app))
}

#[tauri::command]
pub fn open_settings_window(app: AppHandle, page: Option<String>) {
    open_settings(&app, page.as_deref());
}

/// 前端改配置的统一入口:落盘 + 生效 + 同步菜单栏
#[tauri::command]
pub fn update_config(
    app: AppHandle,
    key: String,
    value: serde_json::Value,
) -> Result<crate::config::Config, String> {
    let store = app.state::<Store>();
    let cfg = store.set(&key, value)?;

    match key.as_str() {
        "size" => {
            if let Some(win) = app.get_webview_window("pet") {
                pet_window::apply_geometry(&win, &store);
            }
        }
        "autostart" => apply_autostart(cfg.autostart)?,
        "hidden" => apply_visibility(&app),
        "idle_minutes" => {
            if let Some(bus) = app.try_state::<crate::bus::Bus>() {
                bus.machine
                    .lock()
                    .unwrap()
                    .set_idle_after(std::time::Duration::from_secs(cfg.idle_minutes as u64 * 60));
            }
        }
        "input_sensing" | "away_minutes" => {
            if let Some(h) = app.try_state::<crate::sources::input_activity::Handle>() {
                h.set_enabled(cfg.input_sensing);
                h.set_away_secs(cfg.away_minutes as u64 * 60);
            }
        }
        _ => {}
    }
    notify_config_changed(&app);
    crate::tray::refresh(&app);
    Ok(cfg)
}

/// 前端调试日志(仅 DESKCAT_DEBUG=1 时输出)
#[tauri::command]
pub fn debug_log(msg: String) {
    if std::env::var("DESKCAT_DEBUG").is_ok() {
        println!("[web] {msg}");
    }
}

/// 开始拖动形象窗口:挂起命中判定,返回窗口当前逻辑坐标供前端算位移。
///
/// 不用 Tauri 的 `startDragging()`——它在 macOS 上依赖 `NSApp.currentEvent`
/// 仍是那次 mousedown,而 IPC 往返之后这个前提已经不成立,表现为"拖不动"。
#[tauri::command]
pub fn start_pet_drag(app: AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("pet").ok_or("缺少形象窗口")?;
    let sf = win.scale_factor().unwrap_or(1.0);
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let cur = crate::hit_through::global_cursor().ok_or("取不到光标位置")?;
    app.state::<crate::drag::DragState>()
        .begin((pos.x as f64 / sf, pos.y as f64 / sf), cur);
    app.state::<crate::hit_through::HitMask>()
        .dragging
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// 兜底收尾:正常情况下由鼠标松开事件收尾,这里只是防止事件被漏掉时卡在拖拽态
#[tauri::command]
pub fn end_pet_drag(app: AppHandle) {
    if app.state::<crate::drag::DragState>().end() {
        app.state::<crate::hit_through::HitMask>()
            .dragging
            .store(false, std::sync::atomic::Ordering::Relaxed);
        settle_pet_position(&app);
    }
}

/// 定气泡摆哪边,并在方位变化时补偿窗口位置,让形象**绝对位置不变**。
///
/// 前端传入当前的翻转状态,返回新的状态。窗口位置的调整在这里一次做完,
/// 避免"先翻转再挪窗口"被用户看成两步跳动。
#[tauri::command]
pub fn resolve_pet_layout(
    app: AppHandle,
    flip_x: bool,
    flip_y: bool,
) -> Option<pet_window::BubbleSide> {
    let win = app.get_webview_window("pet")?;
    let sprite = app.state::<Store>().get().size as f64;
    let sf = win.scale_factor().unwrap_or(1.0);
    let pos = win.outer_position().ok()?;
    let (wx, wy) = (pos.x as f64 / sf, pos.y as f64 / sf);
    let (w, h) = pet_window::window_size(sprite);

    // 形象当前的绝对位置(锚点由当前翻转状态决定)
    let sx = wx + if flip_x { 0.0 } else { w - sprite };
    let sy = wy + if flip_y { 0.0 } else { h - sprite };

    // 找形象中心所在的那块屏
    let (cx, cy) = (sx + sprite / 2.0, sy + sprite / 2.0);
    let monitors = win.available_monitors().ok()?;
    let mon = monitors
        .iter()
        .find(|m| {
            let ms = m.size().to_logical::<f64>(m.scale_factor());
            let mp = m.position().to_logical::<f64>(m.scale_factor());
            cx >= mp.x && cx < mp.x + ms.width && cy >= mp.y && cy < mp.y + ms.height
        })
        .or_else(|| monitors.first())?;
    let ms = mon.size().to_logical::<f64>(mon.scale_factor());
    let mp = mon.position().to_logical::<f64>(mon.scale_factor());

    let side = pet_window::bubble_side_for(sx, sy, sprite, (mp.x, mp.y, ms.width, ms.height));
    if side.flip_x != flip_x || side.flip_y != flip_y {
        // 锚点变了 → 反向挪窗口,抵消形象在窗口内的位移
        let nx = sx - if side.flip_x { 0.0 } else { w - sprite };
        let ny = sy - if side.flip_y { 0.0 } else { h - sprite };
        let _ = win.set_position(tauri::LogicalPosition::new(nx, ny));
        let store = app.state::<Store>();
        if let Ok(p) = win.outer_position() {
            pet_window::remember_position(&win, &store, p);
        }
    }
    Some(side)
}

/// 拖拽收尾:必要时拉回可见区,并记住位置
pub fn settle_pet_position<R: Runtime>(app: &AppHandle<R>) {
    let store = app.state::<Store>();
    if let Some(win) = app.get_webview_window("pet") {
        // 只有当形象几乎看不见时才纠正 —— 跨屏摆放是合法的,乱纠正就是用户眼里的"闪一下"
        pet_window::clamp_into_view(&win, store.get().size as f64);
        if let Ok(pos) = win.outer_position() {
            pet_window::remember_position(&win, &store, pos);
        }
    }
}

/// 记住当前位置
#[tauri::command]
pub fn remember_pet_position(app: AppHandle) {
    let store = app.state::<Store>();
    if let Some(win) = app.get_webview_window("pet") {
        if let Ok(pos) = win.outer_position() {
            pet_window::remember_position(&win, &store, pos);
        }
    }
}

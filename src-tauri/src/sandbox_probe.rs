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

//! 沙盒能力自检:上架 App Store 必须开沙盒,先确认哪些系统能力还能用。
//! 仅在 DESKCAT_SANDBOX_PROBE=1 时运行,不影响正常功能。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub static MOUSE_EVENTS: AtomicU32 = AtomicU32::new(0);

pub fn run(app: &tauri::AppHandle) {
    use tauri::Manager;

    println!("========== 沙盒能力自检 ==========");

    // 是否真的在沙盒里
    let sandboxed = std::env::var("APP_SANDBOX_CONTAINER_ID").is_ok()
        || std::env::var("HOME")
            .map(|h| h.contains("/Library/Containers/"))
            .unwrap_or(false);
    println!("[1] 是否运行在沙盒中: {}", if sandboxed { "是" } else { "否(未签 entitlements 或未生效)" });
    println!("    HOME = {}", std::env::var("HOME").unwrap_or_default());

    // 配置目录:沙盒会重定向到容器内
    let cfg = crate::config::config_path();
    println!("[2] 配置文件路径: {}", cfg.display());
    match std::fs::write(cfg.with_extension("probe"), b"x") {
        Ok(_) => {
            println!("    写入配置目录: ✅ 可以");
            let _ = std::fs::remove_file(cfg.with_extension("probe"));
        }
        Err(e) => println!("    写入配置目录: ❌ {e}"),
    }

    // 容器外写入(一键连接依赖):沙盒下应当失败,需要走用户授权
    let claude = crate::ui::claude_settings_path();
    println!("[3] 直接写 {}: ", claude.display());
    match std::fs::OpenOptions::new().append(true).open(&claude) {
        Ok(_) => println!("    ✅ 可以(未受限)"),
        Err(e) => println!("    ❌ 被拒 → 必须走「用户选中 + 安全书签」({})", e.kind()),
    }

    // 本地 HTTP 监听
    match std::net::TcpListener::bind(("127.0.0.1", 43918)) {
        Ok(l) => { drop(l); println!("[4] 本地 HTTP 监听: ✅ 可以"); }
        Err(e) => println!("[4] 本地 HTTP 监听: ❌ {e}"),
    }

    // 读全窗口列表(全屏检测依赖)
    let (mine, others) = crate::fullscreen::window_count_probe();
    println!("[5] 读窗口列表: 自己 {mine} 个 / 其他应用 {others} 个");
    println!("    → {}", if others > 0 { "✅ 能看到别的应用,全屏检测可用" } else { "❌ 只看得到自己,全屏检测失效" });

    // 全局鼠标监听(点击穿透依赖):等 20 秒数事件
    println!("[6] 全局鼠标监听: 计数 20 秒中…(请随意移动鼠标)");
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let n = MOUSE_EVENTS.load(Ordering::Relaxed);
        println!("[6] 全局鼠标监听: 20 秒收到 {n} 次事件 → {}",
                 if n > 0 { "✅ 可用,点击穿透没问题" } else { "❌ 收不到,点击穿透会失效" });
        println!("========== 自检结束 ==========");
        let _ = app;
    });
    let _: Arc<()> = Arc::new(());
}

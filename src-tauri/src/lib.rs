// 架构分层见 docs/03-架构设计.md 的三条硬规则:
// 事件源只往总线投递语义事件,渲染层只订阅状态快照,两边都不碰状态机内部。
mod bus;
mod config;
mod drag;
mod fullscreen;
mod hit_through;
pub mod hooks_install;
mod packs;
mod sandbox_probe;
mod pet_window;
mod sources;
mod state_machine;
mod tray;
mod ui;

use std::time::Duration;
use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .manage(hit_through::HitMask::default())
        .manage(config::Store::load())
        .manage(ui::Fullscreen::default())
        .manage(drag::DragState::default())
        .invoke_handler(tauri::generate_handler![
            hit_through::set_hit_mask,
            config::get_config,
            ui::update_config,
            ui::get_packs,
            ui::get_connection,
            ui::install_hooks,
            ui::uninstall_hooks,
            ui::open_settings_window,
            ui::resolve_pet_layout,
            ui::remember_pet_position,
            ui::start_pet_drag,
            ui::end_pet_drag,
            ui::debug_log,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = app.state::<config::Store>().get();

            // 1. 总线 + 状态机
            let bus = bus::start(
                handle.clone(),
                Duration::from_secs(cfg.idle_minutes as u64 * 60),
            );

            // 2. 事件源:Claude hooks(端口占用则降级为未连接,应用其余功能照常)
            let listening = sources::claude_hook::start(bus.sender()).is_ok();
            if !listening {
                eprintln!("[deskcat] 端口 {} 被占用,Claude Code 感知不可用", sources::claude_hook::PORT);
            }
            app.manage(ui::Listening(listening));

            // 3. 事件源:键鼠感知
            let input = sources::input_activity::start(
                bus.sender(),
                cfg.input_sensing,
                cfg.away_minutes as u64 * 60,
            );
            app.manage(input);
            app.manage(bus);

            // 4. 形象窗口:几何 + 点击穿透 + 拖动记忆
            let pet = app.get_webview_window("pet").expect("缺少 pet 窗口");
            pet_window::apply_geometry(&pet, &app.state::<config::Store>());
            pet_window::raise_to_status_level(&pet);
            // 明确起始状态,不依赖窗口默认值
            let _ = pet.set_ignore_cursor_events(false);
            hit_through::start(pet.clone());
            {
                let h = handle.clone();
                pet.on_window_event(move |e| {
                    if let tauri::WindowEvent::Moved(_) = e {
                        // 拖动结束由前端 invoke remember_pet_position 落盘;
                        // 这里只保证窗口移动不影响其他状态
                        let _ = h.emit("pet-moved", ());
                    }
                });
            }

            // 5. 设置窗口:关闭时只隐藏,不退出应用
            if let Some(settings) = app.get_webview_window("settings") {
                let s = settings.clone();
                settings.on_window_event(move |e| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                        api.prevent_close();
                        let _ = s.hide();
                    }
                });
            }

            // 6. 菜单栏
            tray::build(&handle)?;
            if std::env::var("DESKCAT_DEBUG").is_ok() {
                let packs = ui::list_packs(&handle);
                println!(
                    "[boot] 形象包 {} 个: {}",
                    packs.len(),
                    packs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            ui::apply_visibility(&handle);

            // 7. 全屏隐藏:事件驱动(前台应用切换 / Space 切换时才检查),不轮询
            watch_fullscreen(handle.clone());

            if std::env::var("DESKCAT_SANDBOX_PROBE").is_ok() {
                sandbox_probe::run(&handle);
            }

            // 8. 首启打招呼(仅一次)
            if cfg.first_run {
                let h = handle.clone();
                let bus = app.state::<bus::Bus>();
                bus.post(state_machine::Event::SessionStart {
                    id: "__first_run__".into(),
                    session: "first-run".into(),
                });
                bus.post(state_machine::Event::SessionEnd {
                    id: "__first_run__".into(),
                });
                let store = app.state::<config::Store>();
                let _ = store.set("first_run", serde_json::json!(false));
                let _ = h.emit("first-run-hint", ());
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("DeskCat 启动失败");
}

/// 订阅前台应用/Space 变化来检查全屏,而不是常驻轮询(性能红线)
fn watch_fullscreen(app: tauri::AppHandle) {
    let check = move || {
        let store = app.state::<config::Store>();
        if !store.get().hide_on_fullscreen {
            app.state::<ui::Fullscreen>().set(false);
            ui::apply_visibility(&app);
            return;
        }
        let fs = fullscreen::any_fullscreen();
        let state = app.state::<ui::Fullscreen>();
        if state.hidden() != fs {
            state.set(fs);
            ui::apply_visibility(&app);
        }
    };
    platform_watch(check);
}

#[cfg(target_os = "macos")]
fn platform_watch<F: Fn() + Send + 'static>(check: F) {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{ns_string, MainThreadMarker, NSNotificationCenter, NSOperationQueue};

    let Some(_mtm) = MainThreadMarker::new() else {
        return;
    };
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let queue = NSOperationQueue::mainQueue();
    let handler = block2::RcBlock::new(move |_n: core::ptr::NonNull<objc2_foundation::NSNotification>| {
        check();
    });
    for name in [
        ns_string!("NSWorkspaceActiveSpaceDidChangeNotification"),
        ns_string!("NSWorkspaceDidActivateApplicationNotification"),
    ] {
        unsafe {
            let _ = NSNotificationCenter::addObserverForName_object_queue_usingBlock(
                &center,
                Some(name),
                None,
                Some(&queue),
                &handler,
            );
        }
    }
    std::mem::forget(handler);
}

#[cfg(not(target_os = "macos"))]
fn platform_watch<F: Fn() + Send + 'static>(_check: F) {}

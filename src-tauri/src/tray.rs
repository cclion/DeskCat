//! 菜单栏(Tray):项与设计板 S4 对齐;与设置窗口共享同一份配置,双向同步。

use crate::config::Store;
use crate::{pet_window, ui};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

pub const ID_TOGGLE: &str = "toggle_pet";
pub const ID_RESET_POS: &str = "reset_pos";
pub const ID_CHIME: &str = "chime";
pub const ID_AUTOSTART: &str = "autostart";
pub const ID_CONNECTION: &str = "connection";
pub const ID_SETTINGS: &str = "settings";
pub const ID_QUIT: &str = "quit";
pub const PACK_PREFIX: &str = "pack:";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    // 菜单栏用单色模板图(macOS 规范:系统按明暗主题自动反色);
    // 直接拿应用图标当模板会变成一坨实心圆角方块。
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray@2x.png"))
        .expect("托盘图标解码失败");
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let store = app.state::<Store>();
    let cfg = store.get();

    let toggle_label = if cfg.hidden { "显示小猫" } else { "隐藏小猫" };
    let toggle = MenuItem::with_id(app, ID_TOGGLE, toggle_label, true, None::<&str>)?;
    let reset = MenuItem::with_id(app, ID_RESET_POS, "回到默认位置", true, None::<&str>)?;

    // 切换形象子菜单
    let packs = ui::list_packs(app);
    let mut items: Vec<CheckMenuItem<R>> = Vec::new();
    for p in &packs {
        items.push(CheckMenuItem::with_id(
            app,
            format!("{PACK_PREFIX}{}", p.id),
            &p.name,
            true,
            p.id == cfg.pack_id,
            None::<&str>,
        )?);
    }
    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> =
        items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<R>).collect();
    let packs_menu = Submenu::with_items(app, "切换形象", true, &refs)?;

    let chime = CheckMenuItem::with_id(app, ID_CHIME, "提示音", true, cfg.chime, None::<&str>)?;
    let autostart =
        CheckMenuItem::with_id(app, ID_AUTOSTART, "开机自启", true, cfg.autostart, None::<&str>)?;

    let conn_label = ui::connection_label(app);
    let connection = MenuItem::with_id(app, ID_CONNECTION, conn_label, true, None::<&str>)?;

    let settings = MenuItem::with_id(app, ID_SETTINGS, "设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "退出 DeskCat", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);

    Menu::with_items(
        app,
        &[
            &toggle,
            &reset,
            &sep()?,
            &packs_menu,
            &chime,
            &autostart,
            &sep()?,
            &connection,
            &sep()?,
            &settings,
            &quit,
        ],
    )
}

/// 配置变化后重建菜单,保证勾选态与设置窗口一致
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = build_menu(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn on_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let store = app.state::<Store>();
    match id {
        ID_TOGGLE => {
            let hidden = !store.get().hidden;
            let _ = store.set("hidden", serde_json::json!(hidden));
            ui::apply_visibility(app);
            refresh(app);
        }
        ID_RESET_POS => {
            let _ = store.set("pos_x", serde_json::Value::Null);
            let _ = store.set("pos_y", serde_json::Value::Null);
            if let Some(win) = app.get_webview_window("pet") {
                pet_window::apply_geometry(&win, &store);
            }
        }
        ID_CHIME => {
            let v = !store.get().chime;
            let _ = store.set("chime", serde_json::json!(v));
            ui::notify_config_changed(app);
            refresh(app);
        }
        ID_AUTOSTART => {
            let v = !store.get().autostart;
            let _ = store.set("autostart", serde_json::json!(v));
            let _ = ui::apply_autostart(v);
            ui::notify_config_changed(app);
            refresh(app);
        }
        ID_CONNECTION => ui::open_settings(app, Some("claude")),
        ID_SETTINGS => ui::open_settings(app, None),
        ID_QUIT => app.exit(0),
        other if other.starts_with(PACK_PREFIX) => {
            let pack = other.trim_start_matches(PACK_PREFIX).to_string();
            let _ = store.set("pack_id", serde_json::json!(pack));
            ui::notify_config_changed(app);
            refresh(app);
        }
        _ => {}
    }
}

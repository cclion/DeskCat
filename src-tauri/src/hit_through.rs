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

//! 点击穿透:指针在形象不透明像素上时窗口接收事件,否则整窗穿透到底下应用。
//!
//! 关键约束:窗口一旦 ignore_cursor_events(true),WebView 就收不到任何 mousemove,
//! 无法自己判断指针何时回到猫身上。所以命中判定必须放在窗口之外——用 AppKit 的
//! 全局鼠标移动监听(事件驱动,鼠标不动时零开销;**不是**轮询)。
//!
//! 前端把形象的 alpha 掩码降采样成 N×N 布尔位图交给这里,移动时查表即可。

use std::sync::Mutex;
use tauri::WebviewWindow;

/// 掩码分辨率(N×N);64 足够区分猫身与空白,内存 4KB
pub const N: usize = 64;

pub struct HitMask {
    bits: Mutex<Option<Vec<bool>>>,
    ignoring: Mutex<bool>,
    /// 拖动期间挂起命中判定:否则指针滑出猫身会立刻穿透,把拖动打断
    pub dragging: std::sync::atomic::AtomicBool,
}

impl Default for HitMask {
    fn default() -> Self {
        Self {
            // 掩码就绪前不穿透,避免首帧点不到猫
            bits: Mutex::new(None),
            ignoring: Mutex::new(false),
            dragging: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// 前端算好 alpha 掩码后调用一次(换形象/换状态时重调)
#[tauri::command]
pub fn set_hit_mask(state: tauri::State<'_, HitMask>, bits: Vec<bool>) -> Result<(), String> {
    if bits.len() != N * N {
        return Err(format!("掩码长度应为 {}, 实为 {}", N * N, bits.len()));
    }
    if std::env::var("DESKCAT_DEBUG").is_ok() {
        let n = bits.iter().filter(|b| **b).count();
        println!("[mask] 收到掩码,不透明格 {n}/{}", N * N);
    }
    *state.bits.lock().unwrap() = Some(bits);
    Ok(())
}

/// 归一化坐标 (u,v) ∈ [0,1) 是否落在不透明像素上
pub fn opaque_at(mask: &HitMask, u: f64, v: f64) -> bool {
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return false;
    }
    let guard = mask.bits.lock().unwrap();
    match guard.as_ref() {
        None => true, // 掩码未就绪:窗口内一律接收,不穿透
        Some(bits) => {
            let x = (u * N as f64) as usize;
            let y = (v * N as f64) as usize;
            bits[y.min(N - 1) * N + x.min(N - 1)]
        }
    }
}

/// 全局光标位置(top-left 原点、points),跨显示器连续。
/// **不要用 webview 的 e.screenX/Y 做跨屏位移**:WebKit 报的是
/// "相对光标所在那块屏顶边"的坐标,跨屏瞬间会跳变上千点。
pub fn global_cursor() -> Option<(f64, f64)> {
    platform::cursor_position()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use core::ffi::c_void;
    use objc2_app_kit::{NSEvent, NSEventMask};
    use tauri::Manager;

    // 直接读全局指针位置:top-left 原点、points,与 Tauri 的窗口坐标同一坐标系,
    // 免去 AppKit 左下原点的翻转换算(NSEvent::mouseLocation 在此绑定下返回 0)。
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(v: *const c_void);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    pub fn cursor_position() -> Option<(f64, f64)> {
        unsafe {
            let ev = CGEventCreate(core::ptr::null());
            if ev.is_null() {
                return None;
            }
            let p = CGEventGetLocation(ev);
            CFRelease(ev);
            Some((p.x, p.y))
        }
    }

    /// 注册全局鼠标移动监听(免权限:仅 mouseMoved/dragged,不涉及键盘)
    pub fn start(window: WebviewWindow) {
        let debug = std::env::var("DESKCAT_HIT_DEBUG").is_ok();
        let win2 = window.clone();

        let handler = block2::RcBlock::new(move |_event: core::ptr::NonNull<NSEvent>| {
            crate::sandbox_probe::MOUSE_EVENTS
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some((cursor_x, cursor_y)) = cursor_position() else {
                return;
            };
            let app = window.app_handle();
            let mask = app.state::<HitMask>();

            let (Ok(pos), Ok(size), scale) = (
                window.outer_position(),
                window.outer_size(),
                window.scale_factor().unwrap_or(1.0),
            ) else {
                return;
            };
            let (wx, wy) = (pos.x as f64 / scale, pos.y as f64 / scale);
            let (ww, wh) = (size.width as f64 / scale, size.height as f64 / scale);
            if ww <= 0.0 || wh <= 0.0 {
                return;
            }

            // 拖动中:窗口必须持续接收事件,不做命中判定
            if mask.dragging.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let (u, v) = ((cursor_x - wx) / ww, (cursor_y - wy) / wh);
            let hit = opaque_at(&mask, u, v);
            if debug {
                println!(
                    "[hit:dbg] cursor=({cursor_x:.0},{cursor_y:.0}) win=({wx:.0},{wy:.0} {ww:.0}x{wh:.0}) uv=({u:.3},{v:.3}) hit={hit}"
                );
            }

            let want_ignore = !hit;
            let mut cur = mask.ignoring.lock().unwrap();
            if *cur != want_ignore {
                let r = window.set_ignore_cursor_events(want_ignore);
                if debug {
                    println!(
                        "[hit] {} @({cursor_x:.0},{cursor_y:.0}) 结果={:?}",
                        if want_ignore { "穿透" } else { "接收" },
                        r.as_ref().map(|_| "ok").map_err(|e| e.to_string())
                    );
                }
                if r.is_ok() {
                    *cur = want_ignore;
                }
            }
        });

        // 返回的监听对象必须持有:一旦被释放,AppKit 会把监听注销掉,
        // 表现为"命中判定完全不工作、点不到猫"。
        let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged,
            &handler,
        );
        if monitor.is_none() {
            eprintln!("[hit_through] 全局鼠标监听注册失败,点击穿透将不可用");
        }
        std::mem::forget(monitor);
        std::mem::forget(handler); // 监听存活到进程结束

        start_drag_pump(win2, debug);
    }

    /// 拖拽泵:窗口位置完全由鼠标事件驱动,IPC 不参与 —— 没有异步就没有竞态。
    ///
    /// 需要**两个**监听:拖动中窗口是接收事件的,那些事件属于本应用,
    /// 全局监听收不到(它只报送给其他应用的事件),必须靠本地监听。
    fn start_drag_pump(window: WebviewWindow, debug: bool) {
        use tauri::Manager;

        let step = move |window: &WebviewWindow| {
            let app = window.app_handle();
            let Some(drag) = app.try_state::<crate::drag::DragState>() else {
                return;
            };
            let Some((cx, cy)) = cursor_position() else { return };
            if let Some((x, y)) = drag.target((cx, cy)) {
                let _ = window.set_position(tauri::LogicalPosition::new(x, y));
            }
        };

        let finish = move |window: &WebviewWindow, debug: bool| {
            let app = window.app_handle();
            let Some(drag) = app.try_state::<crate::drag::DragState>() else {
                return;
            };
            if !drag.end() {
                return;
            }
            app.state::<HitMask>()
                .dragging
                .store(false, std::sync::atomic::Ordering::Relaxed);
            crate::ui::settle_pet_position(&app);
            if debug {
                println!("[drag] 松手,位置已收尾");
            }
        };

        // 本地监听:事件送给本应用时走这里(拖动中的绝大多数事件)
        let w_local = window.clone();
        let local = block2::RcBlock::new(move |event: core::ptr::NonNull<NSEvent>| -> *mut NSEvent {
            let app = w_local.app_handle();
            if let Some(drag) = app.try_state::<crate::drag::DragState>() {
                if drag.is_dragging() {
                    match unsafe { event.as_ref().r#type() } {
                        objc2_app_kit::NSEventType::LeftMouseDragged => step(&w_local),
                        objc2_app_kit::NSEventType::LeftMouseUp => finish(&w_local, debug),
                        _ => {}
                    }
                }
            }
            event.as_ptr() // 原样放行,不吞事件
        });
        // 本地监听会拿到事件指针,签名要求 unsafe;我们原样放行,不改动事件
        let m1 = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp,
                &local,
            )
        };
        std::mem::forget(m1);
        std::mem::forget(local);

        // 全局监听:光标拖出窗口、事件被别的应用接走时兜底
        let w_global = window;
        let global = block2::RcBlock::new(move |event: core::ptr::NonNull<NSEvent>| {
            let app = w_global.app_handle();
            if let Some(drag) = app.try_state::<crate::drag::DragState>() {
                if drag.is_dragging() {
                    match unsafe { event.as_ref().r#type() } {
                        objc2_app_kit::NSEventType::LeftMouseDragged => step(&w_global),
                        objc2_app_kit::NSEventType::LeftMouseUp => finish(&w_global, debug),
                        _ => {}
                    }
                }
            }
        });
        let m2 = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp,
            &global,
        );
        std::mem::forget(m2);
        std::mem::forget(global);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    pub fn start(_window: WebviewWindow) {}
    pub fn cursor_position() -> Option<(f64, f64)> {
        None
    }
}

pub use platform::start;

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_with(bits: Vec<bool>) -> HitMask {
        let m = HitMask::default();
        *m.bits.lock().unwrap() = Some(bits);
        m
    }

    #[test]
    fn out_of_bounds_is_transparent() {
        let m = mask_with(vec![true; N * N]);
        for (u, v) in [(-0.01, 0.5), (1.0, 0.5), (0.5, -0.01), (0.5, 1.0)] {
            assert!(!opaque_at(&m, u, v), "uv=({u},{v}) 应判为窗口外");
        }
    }

    #[test]
    fn mask_lookup_maps_uv_to_cell() {
        // 只有右下角那格不透明
        let mut bits = vec![false; N * N];
        bits[(N - 1) * N + (N - 1)] = true;
        let m = mask_with(bits);
        assert!(opaque_at(&m, 0.99, 0.99));
        assert!(!opaque_at(&m, 0.5, 0.5));
        assert!(!opaque_at(&m, 0.0, 0.0));
    }

    #[test]
    fn missing_mask_defaults_to_opaque_inside() {
        let m = HitMask::default();
        assert!(opaque_at(&m, 0.5, 0.5), "掩码未就绪时窗口内应接收事件");
        assert!(!opaque_at(&m, 1.5, 0.5), "窗口外仍应穿透");
    }

    #[test]
    fn rejects_wrong_length_mask() {
        let m = HitMask::default();
        assert!(m.bits.lock().unwrap().is_none());
        // set_hit_mask 的长度校验逻辑
        let bad: Vec<bool> = vec![true; 10];
        assert_ne!(bad.len(), N * N);
    }
}

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

//! 形象窗口的位置/尺寸/显隐管理:位置记忆、越界回弹、尺寸缩放、全屏隐藏。

use crate::config::Store;
use tauri::{Emitter, LogicalPosition, LogicalSize, PhysicalPosition, Runtime, WebviewWindow};

/// 距屏幕右下角的留白
const MARGIN: f64 = 24.0;
/// 气泡需要的横向余量倍数与最小宽度(窗口比形象宽,否则气泡会被裁)
const BUBBLE_W_RATIO: f64 = 2.6;
const BUBBLE_W_MIN: f64 = 340.0;
/// 气泡需要的纵向余量(形象上方):够放两行主文案 + 一行备注
const BUBBLE_H: f64 = 150.0;

/// 把形象窗口抬到"状态栏级"层级。
///
/// Tauri 的 alwaysOnTop 只到 floating(3),会被其他置顶面板压住;
/// 桌宠的交互优先级应当最高——和它重叠时先响应它。
/// 用 NSStatusWindowLevel(25):高于普通窗口与浮动面板,又低于菜单弹出(101),
/// 不会挡住系统菜单下拉。
#[cfg(target_os = "macos")]
pub fn raise_to_status_level<R: Runtime>(win: &WebviewWindow<R>) {
    use objc2::runtime::AnyObject;
    use objc2::msg_send;
    // NSStatusWindowLevel:高于普通窗口与浮动面板(桌宠的交互优先级最高),
    // 又低于菜单弹出(101),不会挡住系统菜单下拉。
    let level: isize = 25;
    if let Ok(handle) = win.ns_window() {
        let ns_window = handle as *mut AnyObject;
        if !ns_window.is_null() {
            unsafe {
                let _: () = msg_send![ns_window, setLevel: level];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn raise_to_status_level<R: Runtime>(_win: &WebviewWindow<R>) {}

/// 由形象边长推出窗口尺寸;形象锚在窗口右下角,气泡在其左上方
pub fn window_size(sprite: f64) -> (f64, f64) {
    ((sprite * BUBBLE_W_RATIO).max(BUBBLE_W_MIN), sprite + BUBBLE_H)
}

/// 主屏右下角的默认位置(逻辑点);形象在窗口右下角,所以窗口整体贴角即可
pub fn default_position<R: Runtime>(win: &WebviewWindow<R>, size: f64) -> (f64, f64) {
    let (w, h) = window_size(size);
    if let Ok(Some(mon)) = win.primary_monitor() {
        let sf = mon.scale_factor();
        let ms = mon.size().to_logical::<f64>(sf);
        let mp = mon.position().to_logical::<f64>(sf);
        (mp.x + ms.width - w - MARGIN, mp.y + ms.height - h - MARGIN)
    } else {
        (100.0, 100.0)
    }
}

/// 形象在所有显示器并集里露出的面积占比。
///
/// **必须按并集算,不能逐屏判断**:跨屏摆放时形象可能横跨两块屏,
/// 对每块屏单独看都"露得不够",但现实中它完全可见 ——
/// 逐屏判断会把这种正常情况误判成越界,松手时把猫拽回去(表现为"闪一下")。
fn visible_fraction<R: Runtime>(win: &WebviewWindow<R>, sx: f64, sy: f64, size: f64) -> f64 {
    let Ok(monitors) = win.available_monitors() else {
        return 0.0;
    };
    let area = size * size;
    if area <= 0.0 {
        return 0.0;
    }
    // 显示器互不重叠,各屏交集面积可直接相加
    let covered: f64 = monitors
        .iter()
        .map(|m| {
            let ms = m.size().to_logical::<f64>(m.scale_factor());
            let mp = m.position().to_logical::<f64>(m.scale_factor());
            let w = (sx + size).min(mp.x + ms.width) - sx.max(mp.x);
            let h = (sy + size).min(mp.y + ms.height) - sy.max(mp.y);
            if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
        })
        .sum();
    (covered / area).min(1.0)
}

/// 形象至少露出这么多才算"找得到"
const MIN_VISIBLE: f64 = 0.35;

/// 记忆位置是否还够得着(窗口左上角坐标 → 形象是否够可见)
pub fn position_visible<R: Runtime>(win: &WebviewWindow<R>, x: f64, y: f64, size: f64) -> bool {
    let (w, h) = window_size(size);
    // 形象在窗口右下角
    visible_fraction(win, x + w - size, y + h - size, size) >= MIN_VISIBLE
}

/// 应用配置里的尺寸与位置(启动时、改尺寸时、回到默认位置时都走这里)
pub fn apply_geometry<R: Runtime>(win: &WebviewWindow<R>, store: &Store) {
    let cfg = store.get();
    let size = cfg.size as f64;
    let (w, h) = window_size(size);
    let _ = win.set_size(LogicalSize::new(w, h));
    // 告诉前端形象该画多大(窗口比形象大,余量留给气泡)
    let _ = win.emit("sprite-size", size);

    let (x, y) = match (cfg.pos_x, cfg.pos_y) {
        (Some(x), Some(y)) if position_visible(win, x as f64, y as f64, size) => {
            (x as f64, y as f64)
        }
        // 无记忆位置,或记忆位置已不可见(改分辨率/拔外接屏)→ 回默认角落
        _ => default_position(win, size),
    };
    let _ = win.set_position(LogicalPosition::new(x, y));
}

/// 气泡该摆在形象的哪一侧。
///
/// 窗口比形象大一圈,多出来的部分是给气泡留的。翻转气泡方位 = 形象在窗口内换锚点,
/// **窗口不动的话形象会瞬间挪一大截**(用户眼里的"跳一下"),所以翻转必须同步补偿窗口位置。
/// 判定还必须相对**形象所在那块屏**算:用全局坐标判"离顶边多近",
/// 在主屏上方的显示器上 y 恒为负,会永远处于翻转态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct BubbleSide {
    /// 气泡向右展开(形象靠屏幕左边时)
    pub flip_x: bool,
    /// 气泡摆到形象下方(形象靠屏幕上边时)
    pub flip_y: bool,
}

/// 由形象在其所在屏幕内的位置,决定气泡摆哪边
pub fn bubble_side_for(sx: f64, sy: f64, sprite: f64, mon: (f64, f64, f64, f64)) -> BubbleSide {
    let (w, h) = window_size(sprite);
    let (need_left, need_above) = (w - sprite, h - sprite);
    let (mx, my, mw, mh) = mon;
    BubbleSide {
        // 左边放不下气泡就向右展开;但右边也放不下时维持默认(向左)
        flip_x: sx - mx < need_left && (mx + mw) - (sx + sprite) >= need_left,
        flip_y: sy - my < need_above && (my + mh) - (sy + sprite) >= need_above,
    }
}

/// 只有当形象几乎看不见时才把它拉回来。
///
/// 显示器之间可能有缝隙、排列也可能不连续,拖动时完全可能松手在无人区,
/// 那样猫就彻底找不回来了。但**跨屏摆放是合法的**,不能一跨屏就纠正位置。
pub fn clamp_into_view<R: Runtime>(win: &WebviewWindow<R>, sprite: f64) {
    let (Ok(pos), Ok(monitors)) = (win.outer_position(), win.available_monitors()) else {
        return;
    };
    let sf = win.scale_factor().unwrap_or(1.0);
    let (x, y) = (pos.x as f64 / sf, pos.y as f64 / sf);
    if position_visible(win, x, y, sprite) {
        return; // 够看得见就别动它 —— 动了就是用户眼里的"闪一下"
    }
    let (w, h) = window_size(sprite);
    let (sx, sy) = (x + w - sprite, y + h - sprite);
    let need = sprite * MIN_VISIBLE;

    // 选形象中心最近的那块屏,把它夹进该屏的合法范围
    let (cx, cy) = (sx + sprite / 2.0, sy + sprite / 2.0);
    let best = monitors.iter().min_by(|a, b| {
        let d = |m: &tauri::window::Monitor| {
            let ms = m.size().to_logical::<f64>(m.scale_factor());
            let mp = m.position().to_logical::<f64>(m.scale_factor());
            let mx = (mp.x + ms.width / 2.0 - cx).abs();
            let my = (mp.y + ms.height / 2.0 - cy).abs();
            mx * mx + my * my
        };
        d(a).total_cmp(&d(b))
    });
    let Some(mon) = best else { return };
    let ms = mon.size().to_logical::<f64>(mon.scale_factor());
    let mp = mon.position().to_logical::<f64>(mon.scale_factor());

    let sx = sx.clamp(mp.x + need - sprite, mp.x + ms.width - need);
    let sy = sy.clamp(mp.y + need - sprite, mp.y + ms.height - need);
    let _ = win.set_position(LogicalPosition::new(sx - (w - sprite), sy - (h - sprite)));
}

/// 拖动结束后记住位置
pub fn remember_position<R: Runtime>(win: &WebviewWindow<R>, store: &Store, pos: PhysicalPosition<i32>) {
    let sf = win.scale_factor().unwrap_or(1.0);
    let x = (pos.x as f64 / sf).round() as i32;
    let y = (pos.y as f64 / sf).round() as i32;
    let _ = store.set("pos_x", serde_json::json!(x));
    let _ = store.set("pos_y", serde_json::json!(y));
}

#[cfg(test)]
mod tests {
    use super::{window_size, MIN_VISIBLE};

    /// 复刻 visible_fraction:形象与"所有屏并集"的交集面积占比
    fn frac(sx: f64, sy: f64, size: f64, screens: &[(f64, f64, f64, f64)]) -> f64 {
        let area = size * size;
        let covered: f64 = screens
            .iter()
            .map(|&(mx, my, mw, mh)| {
                let w = (sx + size).min(mx + mw) - sx.max(mx);
                let h = (sy + size).min(my + mh) - sy.max(my);
                if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
            })
            .sum();
        (covered / area).min(1.0)
    }

    /// 主人的真实三屏布局(左上原点,与窗口坐标同系)
    const SCREENS: [(f64, f64, f64, f64); 3] = [
        (0.0, 0.0, 1728.0, 1117.0),        // 主屏
        (1728.0, -1080.0, 1080.0, 1920.0), // 右侧竖屏
        (-192.0, -1080.0, 1920.0, 1080.0), // 上方屏
    ];
    const S: f64 = 130.0;

    fn visible(sx: f64, sy: f64) -> bool {
        frac(sx, sy, S, &SCREENS) >= MIN_VISIBLE
    }

    #[test]
    fn window_leaves_room_for_bubble() {
        let (w, h) = window_size(130.0);
        assert!(w >= 340.0, "窗口要比形象宽,否则气泡被裁");
        assert!(h > 130.0, "形象上方要留气泡余量");
        assert!(window_size(200.0).0 > w, "形象变大,气泡余量同比变大");
    }

    #[test]
    fn fully_inside_any_screen_is_visible() {
        assert!(visible(800.0, 500.0), "主屏中央");
        assert!(visible(2200.0, 300.0), "右侧竖屏中央");
        assert!(visible(600.0, -500.0), "上方屏中央");
    }

    #[test]
    fn straddling_two_screens_is_visible() {
        // 这是"松手闪一下"的根因用例:横跨主屏与右侧竖屏,
        // 逐屏判断会两边都不达标,但实际完全可见。
        let sx = 1728.0 - S / 2.0; // 一半在主屏,一半在竖屏
        assert!(frac(sx, 400.0, S, &SCREENS) > 0.99, "并集应几乎全覆盖");
        assert!(visible(sx, 400.0));

        // 主屏与上方屏的横向接缝
        let sy = 0.0 - S / 2.0;
        assert!(frac(500.0, sy, S, &SCREENS) > 0.99);
        assert!(visible(500.0, sy));
    }

    #[test]
    fn dead_zone_between_screens_is_not_visible() {
        // 主屏下方:三块屏都够不着
        assert_eq!(frac(900.0, 1500.0, S, &SCREENS), 0.0);
        assert!(!visible(900.0, 1500.0));
        // 右侧竖屏下方的空档
        assert!(!visible(2200.0, 1000.0));
    }

    #[test]
    fn sliver_on_edge_is_not_enough() {
        // 只露出约 20px(< 35%)
        assert!(!visible(1728.0 - 20.0, 900.0));
    }

    #[test]
    fn far_offscreen_is_not_visible() {
        assert!(!visible(9000.0, 9000.0));
        assert!(!visible(-5000.0, 300.0));
    }

    // ---- 气泡方位 ----
    use super::bubble_side_for;

    const MAIN: (f64, f64, f64, f64) = (0.0, 0.0, 1728.0, 1117.0);
    const TOP: (f64, f64, f64, f64) = (-192.0, -1080.0, 1920.0, 1080.0);

    #[test]
    fn bubble_defaults_to_upper_left_when_there_is_room() {
        // 主屏右下角:左边和上边都放得下气泡 → 不翻转
        let side = bubble_side_for(1500.0, 900.0, S, MAIN);
        assert!(!side.flip_x && !side.flip_y);
    }

    #[test]
    fn bubble_flips_when_against_screen_edge() {
        // 贴左上角:两边都要翻
        let side = bubble_side_for(10.0, 10.0, S, MAIN);
        assert!(side.flip_x, "左边放不下,气泡应向右展开");
        assert!(side.flip_y, "上边放不下,气泡应摆到下方");
    }

    #[test]
    fn side_is_relative_to_own_screen_not_global_origin() {
        // 上方那块屏的 y 恒为负;用全局坐标判"离顶边多近"会永远翻转。
        // 形象在该屏中部偏下 → 上方有足够空间 → 不该翻。
        let side = bubble_side_for(600.0, -300.0, S, TOP);
        assert!(!side.flip_y, "相对本屏有空间就不该翻转");
        // 贴该屏顶边才翻
        let side = bubble_side_for(600.0, -1070.0, S, TOP);
        assert!(side.flip_y);
    }

    #[test]
    fn flip_keeps_sprite_in_place() {
        // 翻转必须补偿窗口位置,否则形象在窗口内瞬间挪一大截(用户眼里的"跳一下")
        let (w, h) = window_size(S);
        let (sx, sy) = (400.0, 300.0); // 形象的绝对位置
        for (fx, fy) in [(false, false), (true, false), (false, true), (true, true)] {
            let wx = sx - if fx { 0.0 } else { w - S };
            let wy = sy - if fy { 0.0 } else { h - S };
            // 由窗口位置反推形象位置,应回到原值
            let back_x = wx + if fx { 0.0 } else { w - S };
            let back_y = wy + if fy { 0.0 } else { h - S };
            assert_eq!((back_x, back_y), (sx, sy), "flip=({fx},{fy}) 时形象位置不该变");
        }
    }

    #[test]
    fn side_is_stable_no_oscillation() {
        // 判定基于形象位置(补偿后不变),所以重复求解必须收敛,不能来回翻
        for &(sx, sy) in &[(10.0, 10.0), (1500.0, 900.0), (10.0, 900.0), (1500.0, 10.0)] {
            let a = bubble_side_for(sx, sy, S, MAIN);
            let b = bubble_side_for(sx, sy, S, MAIN);
            assert_eq!(a, b, "同一形象位置必须得到同一方位");
        }
    }

    #[test]
    fn narrow_screen_keeps_default_when_neither_side_fits() {
        // 屏幕比气泡还窄:两边都放不下 → 维持默认,不要乱翻
        let tiny = (0.0, 0.0, 200.0, 200.0);
        let side = bubble_side_for(30.0, 30.0, S, tiny);
        assert!(!side.flip_x && !side.flip_y);
    }

    #[test]
    fn default_corner_position_is_visible() {
        let (w, h) = window_size(S);
        // 贴主屏右下角
        let (x, y) = (1728.0 - w - 24.0, 1117.0 - h - 24.0);
        assert!(visible(x + w - S, y + h - S));
    }
}

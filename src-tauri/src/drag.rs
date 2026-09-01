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

//! 形象窗口拖拽:整个循环跑在原生事件监听里,IPC 不参与。
//!
//! 为什么不用别的做法(踩过的坑,别再绕回去):
//! - `Tauri startDragging()`:macOS 上依赖 `NSApp.currentEvent` 仍是那次 mousedown,
//!   IPC 往返之后前提已不成立 → 完全拖不动。
//! - "前端每帧 invoke 一次,Rust 读当前光标":松手时若还有 step 在 IPC 路上,
//!   它会在松手之后才被处理,读到的是已经移开的光标 → 窗口"闪一下"跳过去追。
//!   跨屏时光标移动快,跳得更远,表现为高概率但不必现。
//! - 用 `e.screenX/Y` 自己算位移:WebKit 的 screenY 相对"光标所在那块屏顶边",
//!   跨屏瞬间跳变上千点。
//!
//! 现在的做法:pointerdown 只负责"记录起点",之后窗口位置完全由
//! 本地/全局鼠标事件监听驱动,松手也由 mouseUp 事件收尾——没有异步,没有竞态。

use std::sync::Mutex;

/// 拖拽锚点:窗口起始位置 + 光标起始位置,同一套全局坐标系(points, top-left)
#[derive(Default)]
pub struct DragState {
    anchor: Mutex<Option<Anchor>>,
}

#[derive(Clone, Copy)]
struct Anchor {
    win_x: f64,
    win_y: f64,
    cur_x: f64,
    cur_y: f64,
}

impl DragState {
    pub fn is_dragging(&self) -> bool {
        self.anchor.lock().unwrap().is_some()
    }

    pub fn begin(&self, win: (f64, f64), cur: (f64, f64)) {
        *self.anchor.lock().unwrap() = Some(Anchor {
            win_x: win.0,
            win_y: win.1,
            cur_x: cur.0,
            cur_y: cur.1,
        });
    }

    pub fn end(&self) -> bool {
        self.anchor.lock().unwrap().take().is_some()
    }

    /// 由当前光标位置推出窗口应处的位置
    pub fn target(&self, cur: (f64, f64)) -> Option<(f64, f64)> {
        let a = (*self.anchor.lock().unwrap())?;
        Some((a.win_x + (cur.0 - a.cur_x), a.win_y + (cur.1 - a.cur_y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_follows_cursor_delta() {
        let d = DragState::default();
        d.begin((100.0, 200.0), (500.0, 600.0));
        assert_eq!(d.target((520.0, 630.0)), Some((120.0, 230.0)));
        // 跨屏到负坐标也只是普通减法(全局坐标系连续)
        assert_eq!(d.target((300.0, -400.0)), Some((-100.0, -800.0)));
    }

    #[test]
    fn no_target_before_begin_or_after_end() {
        let d = DragState::default();
        assert!(d.target((10.0, 10.0)).is_none());
        d.begin((0.0, 0.0), (0.0, 0.0));
        assert!(d.target((10.0, 10.0)).is_some());
        assert!(d.end());
        assert!(d.target((10.0, 10.0)).is_none(), "松手后任何迟到的移动都不该再动窗口");
    }

    #[test]
    fn end_is_idempotent() {
        let d = DragState::default();
        d.begin((0.0, 0.0), (0.0, 0.0));
        assert!(d.end());
        assert!(!d.end(), "重复收尾不应误判为一次新拖拽");
    }

    #[test]
    fn is_dragging_reflects_state() {
        let d = DragState::default();
        assert!(!d.is_dragging());
        d.begin((0.0, 0.0), (0.0, 0.0));
        assert!(d.is_dragging());
        d.end();
        assert!(!d.is_dragging());
    }
}

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

//! InputActivitySource:键鼠活跃/空闲感知。
//!
//! 隐私铁律:只读系统的"距上次输入过了几秒"这一个标量,
//! **物理上拿不到任何按键内容**;免辅助功能/输入监控权限(P0 spike 2 已验证)。

use crate::state_machine::Event;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// 采样间隔:5 秒查一次系统计数器(不是文件轮询,成本≈0)
pub const SAMPLE_SECS: u64 = 5;

#[cfg(target_os = "macos")]
mod sys {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state_id: u32, event_type: u32) -> f64;
    }
    const HID_SYSTEM_STATE: u32 = 1;
    const ANY_INPUT: u32 = u32::MAX;

    pub fn idle_secs() -> f64 {
        unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT) }
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    pub fn idle_secs() -> f64 {
        0.0
    }
}

/// 运行期可调的开关与阈值(设置窗口改动即时生效)
#[derive(Clone)]
pub struct Handle {
    enabled: Arc<AtomicBool>,
    away_secs: Arc<AtomicU64>,
}

impl Handle {
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
    pub fn set_away_secs(&self, secs: u64) {
        self.away_secs.store(secs.max(1), Ordering::Relaxed);
    }
}

/// 由空闲秒数与阈值决定该投递什么事件;纯函数,便于单测。
pub fn decide(idle: f64, away_secs: u64, was_away: bool) -> Option<(Event, bool)> {
    let away_now = idle >= away_secs as f64;
    if away_now == was_away {
        return None; // 状态没变,不投递(避免刷屏)
    }
    Some(if away_now {
        (Event::InputIdle, true)
    } else {
        (Event::InputActive, false)
    })
}

pub fn start(tx: Sender<Event>, enabled: bool, away_secs: u64) -> Handle {
    let handle = Handle {
        enabled: Arc::new(AtomicBool::new(enabled)),
        away_secs: Arc::new(AtomicU64::new(away_secs.max(1))),
    };
    let h = handle.clone();
    std::thread::spawn(move || {
        let mut was_away = false;
        loop {
            std::thread::sleep(Duration::from_secs(SAMPLE_SECS));
            if !h.enabled.load(Ordering::Relaxed) {
                was_away = false;
                continue;
            }
            let away_secs = h.away_secs.load(Ordering::Relaxed);
            if let Some((ev, now_away)) = decide(sys::idle_secs(), away_secs, was_away) {
                was_away = now_away;
                if tx.send(ev).is_err() {
                    break; // 总线关闭,退出线程
                }
            }
        }
    });
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosses_into_away() {
        let (ev, away) = decide(200.0, 180, false).unwrap();
        assert_eq!(ev, Event::InputIdle);
        assert!(away);
    }

    #[test]
    fn comes_back_from_away() {
        let (ev, away) = decide(0.5, 180, true).unwrap();
        assert_eq!(ev, Event::InputActive);
        assert!(!away);
    }

    #[test]
    fn no_event_when_unchanged() {
        assert!(decide(5.0, 180, false).is_none(), "一直活跃不该反复投递");
        assert!(decide(500.0, 180, true).is_none(), "一直离开不该反复投递");
    }

    #[test]
    fn threshold_is_inclusive() {
        assert!(decide(180.0, 180, false).is_some(), "刚好到阈值应判为离开");
    }

    #[test]
    fn only_scalar_is_read() {
        // 回归护栏:实现部分不得出现任何按键内容相关的 API
        // (只扫 #[cfg(test)] 之前的实现代码,否则本列表会自我命中)
        let full = include_str!("input_activity.rs");
        let impl_src = full.split("#[cfg(test)]").next().unwrap();
        for banned in ["EventTapCreate", "keyCode", "MaskKeyDown", "EventKeyboard"] {
            assert!(!impl_src.contains(banned), "隐私红线:实现中不得引入 {banned}");
        }
        // 正面确认:只用了"距上次输入几秒"这一个标量 API
        assert!(impl_src.contains("CGEventSourceSecondsSinceLastEventType"));
    }
}

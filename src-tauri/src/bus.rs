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

//! 统一事件总线:事件源 → 总线 → 状态机 → 渲染层。
//!
//! 架构硬规则:事件源只往 tx 投递语义事件,不知道状态机的存在;
//! 渲染层只订阅 `state-changed`,不知道事件源的存在。

use crate::state_machine::{Event, Machine, Snapshot};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// 兜底 tick 间隔:一次性动画到期与 busy 回落都靠它;
/// 全局只有这一个定时器(与键鼠采样一起构成仅有的两个周期任务)
const TICK_SECS: u64 = 1;

pub struct Bus {
    tx: Sender<Event>,
    pub machine: Arc<Mutex<Machine>>,
}

impl Bus {
    pub fn sender(&self) -> Sender<Event> {
        self.tx.clone()
    }

    pub fn post(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }
}

/// 启动总线:一个线程消费事件 + 一个线程按秒投 Tick
pub fn start(app: AppHandle, idle_after: Duration) -> Bus {
    let (tx, rx): (Sender<Event>, Receiver<Event>) = channel();
    let machine = Arc::new(Mutex::new(Machine::new(idle_after)));
    let debug = std::env::var("DESKCAT_DEBUG").is_ok();

    {
        let machine = machine.clone();
        std::thread::spawn(move || {
            for ev in rx {
                let snap: Option<Snapshot> =
                    machine.lock().unwrap().apply(ev, Instant::now());
                if let Some(snap) = snap {
                    if debug {
                        println!(
                            "[state] {:?}/{:?} session={:?} detail={:?} sticky={} also={}",
                            snap.state, snap.substate, snap.session, snap.detail,
                            snap.sticky, snap.also_pending
                        );
                    }
                    let _ = app.emit("state-changed", &snap);
                }
            }
        });
    }

    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(TICK_SECS));
            if tx.send(Event::Tick).is_err() {
                break;
            }
        });
    }

    Bus { tx, machine }
}

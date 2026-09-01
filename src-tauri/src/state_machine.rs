//! 状态机:六个语义态 + 子状态 + 多来源仲裁 + 兜底回落。
//!
//! 设计要点(对齐 docs/03-架构设计.md 三条硬规则):
//! - 事件源只投递语义事件,不直接操作状态;状态机不认识 "Claude" 以外的任何实现细节
//! - 纯函数式:`apply(event, now) -> 状态快照`,不依赖窗口、不做 IO,因此可完整单元测试
//! - 仲裁优先级(07 §状态仲裁):waiting/alert > celebrating > busy(Claude) > busy(陪伴) > greeting > idle

use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 语义状态(渲染层只认这六个 + 子表现)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Busy,
    Waiting,
    Celebrating,
    Alert,
    Greeting,
}

/// 子表现:同一语义态下的不同演法
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Substate {
    None,
    /// idle 的打盹
    Sleep,
    /// greeting 的睡醒伸懒腰
    Wake,
    /// busy 来自 Claude 在干活
    Claude,
    /// busy 来自键鼠陪伴
    Company,
}

/// 进入状态机的语义事件(与事件源解耦:事件源负责翻译,状态机只认这些)
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Claude 会话开始。`id` = Claude Code 的 session_id(唯一),
    /// `session` = 展示用标签(项目目录名)——同一个目录可能同时开着多个会话,
    /// 所以**绝不能拿目录名当 key**,否则会话数会被合并成 1。
    SessionStart { id: String, session: String },
    /// Claude 在干活(带动作描述,用于气泡)
    Busy { id: String, session: String, action: Option<String> },
    /// Claude 等待用户决策——最高优先级。
    /// `permission=true` 表示等你批准某个操作(不点就一直卡着),false 表示只是等你回话。
    Waiting { id: String, session: String, detail: Option<String>, permission: bool },
    /// Claude 出错
    Alert { id: String, session: String, detail: Option<String> },
    /// Claude 一轮任务完成
    Done { id: String, session: String },
    /// Claude 会话结束
    SessionEnd { id: String },
    /// 键鼠活跃
    InputActive,
    /// 键鼠空闲超阈值
    InputIdle,
    /// 定时器:检查一次性动画到期与 busy 兜底回落
    Tick,
}

/// 推给渲染层的快照
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub state: State,
    pub substate: Substate,
    /// 事件来源会话(气泡备注用:项目目录名)
    pub session: Option<String>,
    /// 事件细节(工具名/错误摘要),气泡备注行用
    pub detail: Option<String>,
    /// 是否常驻气泡(waiting / alert)
    pub sticky: bool,
    /// waiting 与 alert 并存时置位,气泡提示"还有另一件事"
    pub also_pending: bool,
    /// waiting 时:是否在等你批准某个操作(而不是只等你回话)
    pub permission: bool,
}

/// 每个 Claude 会话的独立状态
#[derive(Debug, Clone)]
struct Session {
    state: State,
    /// 展示用标签(项目目录名)
    label: String,
    /// waiting 是否为"等你批准操作"(区别于"等你回话")
    permission: bool,
    detail: Option<String>,
    /// 最后一次收到该会话事件的时间(用于 busy 兜底回落)
    last_seen: Instant,
    /// 进入 waiting / alert 的时刻(用于"显示最新那个")
    pending_since: Option<Instant>,
}

pub struct Machine {
    sessions: HashMap<String, Session>,
    /// 键鼠陪伴态
    company_active: bool,
    sleeping: bool,
    /// 一次性动画(greeting / celebrating)的到期时间
    oneshot: Option<(State, Substate, Option<String>, Instant)>,
    /// busy 兜底回落阈值
    idle_after: Duration,
    last_snapshot: Option<Snapshot>,
}

pub const CELEBRATING_MS: u64 = 5_000;
pub const GREETING_MS: u64 = 3_000;
/// 会话彻底无动静多久后视为已消失(终端被直接关掉时收不到 SessionEnd)
pub const STALE_AFTER: Duration = Duration::from_secs(2 * 3600);
/// waiting/alert 的兜底回落倍数(相对 idle 阈值)。
/// 常驻气泡本意是"不解除不消失",但会话可能已经死了、或用户已在别处处理完
/// 却没有后续 hook —— 没有兜底就会永远举着手不放。
const STICKY_FALLBACK_MULT: u32 = 3;

impl Machine {
    pub fn new(idle_after: Duration) -> Self {
        Self {
            sessions: HashMap::new(),
            company_active: false,
            sleeping: false,
            oneshot: None,
            idle_after,
            last_snapshot: None,
        }
    }

    pub fn set_idle_after(&mut self, d: Duration) {
        self.idle_after = d;
    }

    /// 投递事件,返回变化后的快照(无变化返回 None,避免无谓的渲染推送)
    pub fn apply(&mut self, ev: Event, now: Instant) -> Option<Snapshot> {
        self.ingest(ev, now);
        self.expire(now);
        let snap = self.arbitrate();
        if self.last_snapshot.as_ref() == Some(&snap) {
            return None;
        }
        self.last_snapshot = Some(snap.clone());
        Some(snap)
    }

    fn touch(&mut self, id: &str, label: &str, state: State, detail: Option<String>, now: Instant) {
        let pending_since = matches!(state, State::Waiting | State::Alert).then_some(now);
        self.sessions.insert(
            id.to_string(),
            Session {
                state,
                label: label.to_string(),
                permission: false,
                detail,
                last_seen: now,
                pending_since,
            },
        );
    }

    fn ingest(&mut self, ev: Event, now: Instant) {
        match ev {
            Event::SessionStart { id, session } => {
                // 打招呼一下再进入干活
                self.oneshot = Some((
                    State::Greeting,
                    Substate::None,
                    Some(session.clone()),
                    now + Duration::from_millis(GREETING_MS),
                ));
                self.touch(&id, &session, State::Busy, None, now);
            }
            Event::Busy { id, session, action } => {
                self.touch(&id, &session, State::Busy, action, now)
            }
            Event::Waiting { id, session, detail, permission } => {
                self.touch(&id, &session, State::Waiting, detail, now);
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.permission = permission;
                }
            }
            Event::Alert { id, session, detail } => {
                self.touch(&id, &session, State::Alert, detail, now)
            }
            Event::Done { id, session } => {
                self.oneshot = Some((
                    State::Celebrating,
                    Substate::None,
                    Some(session.clone()),
                    now + Duration::from_millis(CELEBRATING_MS),
                ));
                self.touch(&id, &session, State::Idle, None, now);
            }
            Event::SessionEnd { id } => {
                self.sessions.remove(&id);
            }
            Event::InputActive => {
                self.company_active = true;
                if self.sleeping {
                    // 睡醒:伸懒腰一下再回 idle
                    self.sleeping = false;
                    self.oneshot = Some((
                        State::Greeting,
                        Substate::Wake,
                        None,
                        now + Duration::from_millis(GREETING_MS),
                    ));
                }
            }
            Event::InputIdle => {
                self.company_active = false;
                self.sleeping = true;
            }
            Event::Tick => {}
        }
    }

    /// 清理到期的一次性动画与超时的 busy 会话
    fn expire(&mut self, now: Instant) {
        if let Some((_, _, _, until)) = &self.oneshot {
            if now >= *until {
                self.oneshot = None;
            }
        }
        let idle_after = self.idle_after;
        let sticky_after = idle_after * STICKY_FALLBACK_MULT;
        for s in self.sessions.values_mut() {
            let quiet = now.duration_since(s.last_seen);
            let expired = match s.state {
                State::Busy => quiet >= idle_after,
                // waiting/alert 挂得久得多,但不是永远:见 STICKY_FALLBACK_MULT
                State::Waiting | State::Alert => quiet >= sticky_after,
                _ => false,
            };
            if expired {
                s.state = State::Idle;
                s.detail = None;
                s.permission = false;
            }
        }
        // idle 的会话**仍然算活跃会话**(只是这一轮跑完了),不能删——
        // 否则设置窗口里的"N 个会话活跃"会一直显示 0/1。
        // 只回收长时间彻底没动静的(终端被直接关掉,收不到 SessionEnd)。
        self.sessions
            .retain(|_, s| now.duration_since(s.last_seen) < STALE_AFTER);
    }

    /// 按优先级选出当前该演什么
    fn arbitrate(&self) -> Snapshot {
        let pick = |st: State| -> Option<(&String, &Session)> {
            self.sessions
                .iter()
                .filter(|(_, s)| s.state == st)
                // 同级多会话:取最新发生的那个
                .max_by_key(|(_, s)| s.pending_since.unwrap_or(s.last_seen))
        };

        let waiting = pick(State::Waiting);
        let alert = pick(State::Alert);

        // waiting / alert 同时存在:显示最新那个,并置位"还有另一件事"
        if waiting.is_some() || alert.is_some() {
            let both = waiting.is_some() && alert.is_some();
            let newest = match (waiting, alert) {
                (Some(w), Some(a)) => {
                    let wt = w.1.pending_since.unwrap_or(w.1.last_seen);
                    let at = a.1.pending_since.unwrap_or(a.1.last_seen);
                    if wt >= at { w } else { a }
                }
                (Some(w), None) => w,
                (None, Some(a)) => a,
                (None, None) => unreachable!(),
            };
            return Snapshot {
                state: newest.1.state,
                substate: Substate::None,
                session: Some(newest.1.label.clone()),
                detail: newest.1.detail.clone(),
                sticky: true,
                also_pending: both,
                permission: newest.1.permission,
            };
        }

        // 一次性动画(celebrating / greeting)压过 busy
        if let Some((st, sub, session, _)) = &self.oneshot {
            return Snapshot {
                state: *st,
                substate: *sub,
                session: session.clone(),
                detail: None,
                sticky: false,
                also_pending: false,
                permission: false,
            };
        }

        // Claude 在干活,压过键鼠陪伴
        if let Some((_, s)) = pick(State::Busy) {
            return Snapshot {
                state: State::Busy,
                substate: Substate::Claude,
                session: Some(s.label.clone()),
                detail: s.detail.clone(),
                sticky: false,
                also_pending: false,
                permission: false,
            };
        }

        if self.company_active {
            return Snapshot {
                state: State::Busy,
                substate: Substate::Company,
                session: None,
                detail: None,
                sticky: false,
                also_pending: false,
                permission: false,
            };
        }

        Snapshot {
            state: State::Idle,
            substate: if self.sleeping { Substate::Sleep } else { Substate::None },
            session: None,
            detail: None,
            sticky: false,
            also_pending: false,
            permission: false,
        }
    }

    /// 当前活跃会话数(设置窗口显示用)
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Machine {
        Machine::new(Duration::from_secs(600))
    }
    fn s(name: &str) -> String {
        name.to_string()
    }

    #[test]
    fn basic_flow_session_start_to_end() {
        let mut mc = m();
        let t0 = Instant::now();
        // 会话开始 → greeting
        let snap = mc.apply(Event::SessionStart { id: s("a"), session: s("a") }, t0).unwrap();
        assert_eq!(snap.state, State::Greeting);
        // greeting 到期 → busy
        let t1 = t0 + Duration::from_millis(GREETING_MS + 1);
        let snap = mc.apply(Event::Tick, t1).unwrap();
        assert_eq!((snap.state, snap.substate), (State::Busy, Substate::Claude));
        // 等待批准 → waiting(常驻)
        let snap = mc
            .apply(Event::Waiting { id: s("a"), session: s("a"), detail: Some("Bash".into()), permission: true }, t1)
            .unwrap();
        assert_eq!(snap.state, State::Waiting);
        assert!(snap.sticky);
        assert_eq!(snap.session.as_deref(), Some("a"));
        assert_eq!(snap.detail.as_deref(), Some("Bash"));
        // 完成 → celebrating
        let snap = mc.apply(Event::Done { id: s("a"), session: s("a") }, t1).unwrap();
        assert_eq!(snap.state, State::Celebrating);
        // celebrating 到期 → idle
        let t2 = t1 + Duration::from_millis(CELEBRATING_MS + 1);
        let snap = mc.apply(Event::Tick, t2).unwrap();
        assert_eq!(snap.state, State::Idle);
    }

    #[test]
    fn waiting_beats_busy_across_sessions() {
        let mut mc = m();
        let t = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t);
        let snap = mc.apply(Event::Waiting { id: s("b"), session: s("b"), detail: None, permission: true }, t).unwrap();
        assert_eq!(snap.state, State::Waiting);
        assert_eq!(snap.session.as_deref(), Some("b"));
    }

    #[test]
    fn claude_busy_beats_company() {
        let mut mc = m();
        let t = Instant::now();
        mc.apply(Event::InputActive, t);
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t);
        let snap = mc.last_snapshot.clone().unwrap();
        assert_eq!((snap.state, snap.substate), (State::Busy, Substate::Claude));
        // 再来一次键鼠活跃也不改变
        assert!(mc.apply(Event::InputActive, t).is_none(), "不应产生状态变化");
    }

    #[test]
    fn waiting_and_alert_shows_newest_with_flag() {
        let mut mc = m();
        let t0 = Instant::now();
        mc.apply(Event::Alert { id: s("a"), session: s("a"), detail: None }, t0);
        let t1 = t0 + Duration::from_secs(1);
        let snap = mc.apply(Event::Waiting { id: s("b"), session: s("b"), detail: None, permission: true }, t1).unwrap();
        assert_eq!(snap.state, State::Waiting, "显示最新发生的");
        assert!(snap.also_pending, "应置位'还有另一件事'");

        // 反序:alert 更晚
        let mut mc = m();
        mc.apply(Event::Waiting { id: s("b"), session: s("b"), detail: None, permission: true }, t0);
        let snap = mc.apply(Event::Alert { id: s("a"), session: s("a"), detail: None }, t1).unwrap();
        assert_eq!(snap.state, State::Alert);
        assert!(snap.also_pending);
    }

    #[test]
    fn busy_falls_back_after_idle_timeout() {
        let mut mc = Machine::new(Duration::from_secs(60));
        let t0 = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t0);
        let t1 = t0 + Duration::from_secs(61);
        let snap = mc.apply(Event::Tick, t1).unwrap();
        assert_eq!(snap.state, State::Idle, "超过阈值无新事件应回落 idle");
        assert_eq!(mc.active_sessions(), 1, "回落只是这轮跑完了,会话仍然活着");
    }

    #[test]
    fn same_directory_multiple_sessions_counted_separately() {
        // 同一个项目目录开两个会话:必须算两个,不能被目录名合并成一个
        let mut mc = m();
        let t = Instant::now();
        mc.apply(Event::Busy { id: s("sess-1"), session: s("DeskCat"), action: None }, t);
        mc.apply(Event::Busy { id: s("sess-2"), session: s("DeskCat"), action: None }, t);
        assert_eq!(mc.active_sessions(), 2, "同目录的两个会话不能合并");
    }

    #[test]
    fn idle_sessions_still_count_until_stale() {
        let mut mc = Machine::new(Duration::from_secs(60));
        let t0 = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("proj"), action: None }, t0);
        // 跑完一轮 → idle,但会话还在
        mc.apply(Event::Done { id: s("a"), session: s("proj") }, t0);
        assert_eq!(mc.active_sessions(), 1);
        // 长时间彻底没动静(终端被直接关掉)→ 回收
        mc.apply(Event::Tick, t0 + STALE_AFTER + Duration::from_secs(1));
        assert_eq!(mc.active_sessions(), 0, "超过 STALE 才回收");
    }

    #[test]
    fn snapshot_shows_label_not_session_id() {
        let mut mc = m();
        let t = Instant::now();
        let snap = mc
            .apply(Event::Waiting { id: s("abc-123-uuid"), session: s("DeskCat"), detail: None, permission: true }, t)
            .unwrap();
        assert_eq!(snap.session.as_deref(), Some("DeskCat"), "气泡显示项目名,不是 uuid");
    }

    #[test]
    fn waiting_holds_far_longer_than_busy() {
        let mut mc = Machine::new(Duration::from_secs(60));
        let t0 = Instant::now();
        mc.apply(Event::Waiting { id: s("a"), session: s("a"), detail: None, permission: true }, t0);
        // busy 早就该回落的时刻,waiting 仍然挂着
        mc.apply(Event::Tick, t0 + Duration::from_secs(61));
        assert_eq!(mc.last_snapshot.clone().unwrap().state, State::Waiting, "等你决策不该几分钟就消失");
        mc.apply(Event::Tick, t0 + Duration::from_secs(170));
        assert_eq!(mc.last_snapshot.clone().unwrap().state, State::Waiting);
    }

    #[test]
    fn waiting_eventually_falls_back() {
        // 会话已死/用户已在别处处理完,没有后续 hook —— 不能永远举着手
        let mut mc = Machine::new(Duration::from_secs(60));
        let t0 = Instant::now();
        mc.apply(Event::Waiting { id: s("a"), session: s("a"), detail: None, permission: true }, t0);
        let snap = mc.apply(Event::Tick, t0 + Duration::from_secs(60 * 3 + 1)).unwrap();
        assert_eq!(snap.state, State::Idle, "超过兜底时长应回落");
    }

    #[test]
    fn permission_flag_reaches_snapshot() {
        let mut mc = m();
        let t = Instant::now();
        let a = mc
            .apply(Event::Waiting { id: s("x"), session: s("p"), detail: None, permission: true }, t)
            .unwrap();
        assert!(a.permission, "等批权限要能和'等你回话'区分开");
        let b = mc
            .apply(Event::Waiting { id: s("y"), session: s("q"), detail: None, permission: false },
                   t + Duration::from_secs(1))
            .unwrap();
        assert!(!b.permission);
    }

    #[test]
    fn multi_session_independent_timers() {
        let mut mc = Machine::new(Duration::from_secs(60));
        let t0 = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t0);
        let t1 = t0 + Duration::from_secs(50);
        mc.apply(Event::Busy { id: s("b"), session: s("b"), action: None }, t1);
        let t2 = t0 + Duration::from_secs(61); // a 到期,b 未到期
        mc.apply(Event::Tick, t2);
        let snap = mc.last_snapshot.clone().unwrap();
        assert_eq!(snap.state, State::Busy);
        assert_eq!(snap.session.as_deref(), Some("b"), "b 不该被 a 的超时连累");
    }

    #[test]
    fn sleep_and_wake_cycle() {
        let mut mc = m();
        let t0 = Instant::now();
        let snap = mc.apply(Event::InputIdle, t0).unwrap();
        assert_eq!((snap.state, snap.substate), (State::Idle, Substate::Sleep));
        let snap = mc.apply(Event::InputActive, t0).unwrap();
        assert_eq!((snap.state, snap.substate), (State::Greeting, Substate::Wake), "睡醒应伸懒腰");
        let t1 = t0 + Duration::from_millis(GREETING_MS + 1);
        let snap = mc.apply(Event::Tick, t1).unwrap();
        assert_eq!((snap.state, snap.substate), (State::Busy, Substate::Company));
    }

    #[test]
    fn out_of_order_and_duplicate_events_are_safe() {
        let mut mc = m();
        let t = Instant::now();
        // 无对应开始的完成
        mc.apply(Event::Done { id: s("ghost"), session: s("ghost") }, t);
        // 重复三次
        mc.apply(Event::Done { id: s("ghost"), session: s("ghost") }, t);
        mc.apply(Event::Done { id: s("ghost"), session: s("ghost") }, t);
        assert_eq!(mc.last_snapshot.clone().unwrap().state, State::Celebrating);
        // 结束一个不存在的会话
        mc.apply(Event::SessionEnd { id: s("nobody") }, t);
        let t2 = t + Duration::from_millis(CELEBRATING_MS + 1);
        assert_eq!(mc.apply(Event::Tick, t2).unwrap().state, State::Idle);
    }

    #[test]
    fn no_snapshot_when_nothing_changes() {
        let mut mc = m();
        let t = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t);
        assert!(mc.apply(Event::Tick, t).is_none(), "无变化不应重复推送");
    }

    #[test]
    fn session_end_clears_state() {
        let mut mc = m();
        let t = Instant::now();
        mc.apply(Event::Busy { id: s("a"), session: s("a"), action: None }, t);
        assert_eq!(mc.active_sessions(), 1);
        let snap = mc.apply(Event::SessionEnd { id: s("a") }, t).unwrap();
        assert_eq!(snap.state, State::Idle);
        assert_eq!(mc.active_sessions(), 0);
    }

    #[test]
    fn detail_carries_to_snapshot_for_bubble_remark() {
        let mut mc = m();
        let t = Instant::now();
        let snap = mc
            .apply(Event::Waiting { id: s("sess-1"), session: s("deskcat"), detail: Some("Bash · rm -rf ./dist".into()), permission: true }, t)
            .unwrap();
        // 气泡要能点名"谁在等你"
        assert_eq!(snap.session.as_deref(), Some("deskcat"));
        assert_eq!(snap.detail.as_deref(), Some("Bash · rm -rf ./dist"));
        assert!(snap.sticky);
    }
}

//! Spike 2:CGEventSourceSecondsSinceLastEventType 免权限空闲查询验证。
//! 用法: cargo run --example spike_idle [采样次数,默认 8,每 5s 一次]
//! 验证点:读数随空闲线性递增、有输入立即归零、全程无 TCC 授权弹窗。

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state_id: u32, event_type: u32) -> f64;
}

const HID_SYSTEM_STATE: u32 = 1; // kCGEventSourceStateHIDSystemState
const ANY_INPUT: u32 = u32::MAX; // kCGAnyInputEventType

fn idle_secs() -> f64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT) }
}

fn main() {
    let n: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    for i in 0..n {
        println!("t={:>3}s idle={:.2}s", i * 5, idle_secs());
        if i + 1 < n {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }
}

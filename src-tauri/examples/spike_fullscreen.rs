//! Spike 3:全屏检测验证——前台应用是否有覆盖整个屏幕的 layer-0 窗口。
//! 免权限依据:CGWindowList 的 bounds/layer/owner 不需要屏幕录制权限(仅窗口标题需要)。
//! 用法: cargo run --example spike_fullscreen [秒数,默认 60],每 1s 打一行,状态变化时标记 <<<
use core::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, idx: isize) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(alloc: *const c_void, s: *const u8, encoding: u32) -> *const c_void;
    fn CFNumberGetValue(num: *const c_void, ty: isize, out: *mut c_void) -> bool;
    fn CFRelease(v: *const c_void);
}

const ON_SCREEN_ONLY: u32 = 1 << 0;
const EXCLUDE_DESKTOP: u32 = 1 << 4;
const UTF8: u32 = 0x0800_0100;
const CF_NUMBER_F64: isize = 13; // kCFNumberFloat64Type
const CF_NUMBER_I64: isize = 4; // kCFNumberSInt64Type

fn cfstr(s: &str) -> *const c_void {
    let c = format!("{s}\0");
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

fn num_i64(dict: *const c_void, key: *const c_void) -> Option<i64> {
    unsafe {
        let v = CFDictionaryGetValue(dict, key);
        if v.is_null() {
            return None;
        }
        let mut out: i64 = 0;
        CFNumberGetValue(v, CF_NUMBER_I64, &mut out as *mut i64 as *mut c_void).then_some(out)
    }
}

fn bounds_wh(dict: *const c_void, key_bounds: *const c_void, kw: *const c_void, kh: *const c_void) -> Option<(f64, f64)> {
    unsafe {
        let b = CFDictionaryGetValue(dict, key_bounds);
        if b.is_null() {
            return None;
        }
        let (mut w, mut h): (f64, f64) = (0.0, 0.0);
        let okw = {
            let v = CFDictionaryGetValue(b, kw);
            !v.is_null() && CFNumberGetValue(v, CF_NUMBER_F64, &mut w as *mut f64 as *mut c_void)
        };
        let okh = {
            let v = CFDictionaryGetValue(b, kh);
            !v.is_null() && CFNumberGetValue(v, CF_NUMBER_F64, &mut h as *mut f64 as *mut c_void)
        };
        (okw && okh).then_some((w, h))
    }
}

fn any_fullscreen_window() -> bool {
    unsafe {
        let (sw, sh) = {
            let d = CGMainDisplayID();
            (CGDisplayPixelsWide(d) as f64, CGDisplayPixelsHigh(d) as f64)
        };
        let key_layer = cfstr("kCGWindowLayer");
        let key_bounds = cfstr("kCGWindowBounds");
        let kw = cfstr("Width");
        let kh = cfstr("Height");
        let arr = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
        let mut hit = false;
        if !arr.is_null() {
            for i in 0..CFArrayGetCount(arr) {
                let w = CFArrayGetValueAtIndex(arr, i);
                // 只看普通应用层(layer 0);菜单栏/Dock/悬浮层排除
                if num_i64(w, key_layer) != Some(0) {
                    continue;
                }
                if let Some((ww, wh)) = bounds_wh(w, key_bounds, kw, kh) {
                    // 覆盖整个主屏 = 全屏(含浏览器视频全屏与全屏 Space)
                    if ww >= sw && wh >= sh {
                        hit = true;
                        break;
                    }
                }
            }
            CFRelease(arr);
        }
        for k in [key_layer, key_bounds, kw, kh] {
            CFRelease(k);
        }
        hit
    }
}

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let mut prev = None;
    for t in 0..secs {
        let fs = any_fullscreen_window();
        if prev != Some(fs) {
            println!("t={t:>3}s fullscreen={fs} <<< 状态变化");
        } else if t % 10 == 0 {
            println!("t={t:>3}s fullscreen={fs}");
        }
        prev = Some(fs);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

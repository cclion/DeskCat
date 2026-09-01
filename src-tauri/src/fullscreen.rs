//! 全屏检测(P4):前台是否存在覆盖整个主屏的 layer-0 窗口。
//!
//! P0 spike 3 已验证判定式与免权限性(只读 bounds/layer,不读标题、不截图)。
//! 生产实现用 NSWorkspace 通知驱动,不做常驻轮询。

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
const CF_NUMBER_F64: isize = 13;
const CF_NUMBER_I64: isize = 4;

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

fn bounds_wh(
    dict: *const c_void,
    key_bounds: *const c_void,
    kw: *const c_void,
    kh: *const c_void,
) -> Option<(f64, f64)> {
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

/// 判定式(纯逻辑部分抽出来便于单测):窗口是否覆盖整个主屏
pub fn covers_screen(win_w: f64, win_h: f64, screen_w: f64, screen_h: f64) -> bool {
    win_w >= screen_w && win_h >= screen_h
}

/// 沙盒自检用:能看到多少个自己的窗口、多少个别的应用的窗口
pub fn window_count_probe() -> (usize, usize) {
    unsafe {
        let key_owner = cfstr("kCGWindowOwnerPID");
        let arr = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
        let me = std::process::id() as i64;
        let (mut mine, mut others) = (0usize, 0usize);
        if !arr.is_null() {
            for i in 0..CFArrayGetCount(arr) {
                let w = CFArrayGetValueAtIndex(arr, i);
                match num_i64(w, key_owner) {
                    Some(pid) if pid == me => mine += 1,
                    Some(_) => others += 1,
                    None => {}
                }
            }
            CFRelease(arr);
        }
        CFRelease(key_owner);
        (mine, others)
    }
}

pub fn any_fullscreen() -> bool {
    unsafe {
        let d = CGMainDisplayID();
        let (sw, sh) = (CGDisplayPixelsWide(d) as f64, CGDisplayPixelsHigh(d) as f64);
        let key_layer = cfstr("kCGWindowLayer");
        let key_bounds = cfstr("kCGWindowBounds");
        let kw = cfstr("Width");
        let kh = cfstr("Height");
        let arr = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0);
        let mut hit = false;
        if !arr.is_null() {
            for i in 0..CFArrayGetCount(arr) {
                let w = CFArrayGetValueAtIndex(arr, i);
                // 只看普通应用层;菜单栏/Dock/我们自己的置顶窗口都不在 layer 0
                if num_i64(w, key_layer) != Some(0) {
                    continue;
                }
                if let Some((ww, wh)) = bounds_wh(w, key_bounds, kw, kh) {
                    if covers_screen(ww, wh, sw, sh) {
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

#[cfg(test)]
mod tests {
    use super::covers_screen;

    #[test]
    fn exact_screen_size_counts() {
        assert!(covers_screen(1728.0, 1117.0, 1728.0, 1117.0));
    }

    #[test]
    fn larger_than_screen_counts() {
        assert!(covers_screen(1920.0, 1200.0, 1728.0, 1117.0));
    }

    #[test]
    fn maximized_window_below_menubar_does_not_count() {
        // 最大化窗口不覆盖菜单栏,高度小于屏幕高
        assert!(!covers_screen(1728.0, 1092.0, 1728.0, 1117.0));
    }

    #[test]
    fn small_window_does_not_count() {
        assert!(!covers_screen(320.0, 320.0, 1728.0, 1117.0));
    }
}

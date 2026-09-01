//! 用真实 settings.json 的副本验证 hooks 安装/卸载的合并安全性。
//! 用法: cargo run --example hooks_check <settings.json 副本路径>
use deskcat_lib::hooks_install as hooks;
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("需要一个 settings.json 路径"));
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let before_json: serde_json::Value =
        serde_json::from_str(&before).expect("输入必须是合法 JSON");

    let changed = hooks::install(&path).expect("安装失败");
    println!("安装: changed={changed}");
    let after_install: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    // 幂等
    let again = hooks::install(&path).expect("重复安装失败");
    println!("重复安装: changed={again}(应为 false)");

    hooks::uninstall(&path).expect("卸载失败");
    let after_uninstall: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    println!("卸载后与安装前语义等价: {}", after_uninstall == before_json);

    // 用户原有的非 DeskCat 内容是否保住
    let mut preserved = true;
    if let (Some(a), Some(b)) = (before_json.as_object(), after_install.as_object()) {
        for (k, v) in a {
            if k == "hooks" {
                continue;
            }
            if b.get(k) != Some(v) {
                preserved = false;
                println!("  ✗ 字段被改动: {k}");
            }
        }
    }
    println!("安装后用户其他顶层字段完好: {preserved}");

    let user_hook_count = |v: &serde_json::Value| -> usize {
        v.get("hooks")
            .and_then(|h| h.as_object())
            .map(|o| {
                o.values()
                    .filter_map(|a| a.as_array())
                    .flatten()
                    .filter(|e| !serde_json::to_string(e).unwrap_or_default().contains("deskcat"))
                    .count()
            })
            .unwrap_or(0)
    };
    println!(
        "用户原有 hook 条目数 安装前={} 安装后={}(应相等)",
        user_hook_count(&before_json),
        user_hook_count(&after_install)
    );
}

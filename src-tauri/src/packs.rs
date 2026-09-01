//! 形象包发现与读取:扫描前端资源根下的 packs/,读 pack.json。
//! 内置形象包随前端一起打包,不走 asset 协议。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateAsset {
    pub file: String,
    #[serde(default)]
    pub loop_: Option<u32>,
    #[serde(rename = "loop", default)]
    pub loop_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub states: BTreeMap<String, StateAsset>,
    #[serde(default)]
    pub extras: BTreeMap<String, StateAsset>,
}

/// 校验一个包是否可用:六态齐全且文件字段非空
pub fn is_valid(p: &Pack) -> bool {
    const REQUIRED: [&str; 6] = ["idle", "busy", "waiting", "celebrating", "alert", "greeting"];
    !p.id.trim().is_empty()
        && REQUIRED
            .iter()
            .all(|k| p.states.get(*k).is_some_and(|a| !a.file.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Pack {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn parses_real_calico_pack() {
        let text = include_str!("../../src/packs/calico/pack.json");
        let p = parse(text);
        assert_eq!(p.id, "calico");
        assert_eq!(p.name, "三花猫");
        assert!(is_valid(&p), "内置 calico 包应通过校验");
        for k in ["sleep", "pet", "wake"] {
            assert!(p.extras.contains_key(k), "缺少子表现 {k}");
        }
        // 一次性动画的 loop 标记
        assert_eq!(p.states["celebrating"].loop_count, Some(1));
        assert_eq!(p.states["greeting"].loop_count, Some(1));
        assert_eq!(p.states["idle"].loop_count, Some(0));
    }

    #[test]
    fn rejects_incomplete_pack() {
        let p = parse(r#"{"id":"x","name":"X","states":{"idle":{"file":"i.webp","loop":0}}}"#);
        assert!(!is_valid(&p), "六态不全应判为无效");
    }

    #[test]
    fn rejects_empty_file_field() {
        let p = parse(
            r#"{"id":"x","name":"X","states":{
            "idle":{"file":"","loop":0},"busy":{"file":"b.webp","loop":0},
            "waiting":{"file":"w.webp","loop":0},"celebrating":{"file":"c.webp","loop":1},
            "alert":{"file":"a.webp","loop":0},"greeting":{"file":"g.webp","loop":1}}}"#,
        );
        assert!(!is_valid(&p));
    }
}

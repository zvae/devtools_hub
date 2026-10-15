//! 字体：与 macOS 系统界面一致，西文用 SF Pro，中文用苹方，等宽用 SF Mono。
//!
//! egui 自带的字体不含中文；如果主字体和后备字体的上下沿度量差别很大（例如冬青黑体、
//! Apple Symbols），混排时基线会错位。SF Pro 自带 ⌘ ⇧ ⌥ ⌃ ⌫ 等按键符号，
//! 苹方与 SF 的度量相近，组合起来对齐效果最好。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{FontData, FontDefinitions, FontFamily};

/// 标题等需要加粗的文字使用的字体族。
pub const SEMIBOLD: &str = "semibold";

/// 苹方作为 SF 的后备字体时需要下移的距离（以字号为单位）。
///
/// egui 把后备字体的字形按“两种字体行高差的一半”居中，而不是对齐基线：
/// SF 上沿 0.967em、行高 1.178em，苹方上沿 1.06em、行高 1.40em，
/// 算下来苹方的基线比 SF 高 0.967 - (1.06 - (1.40 - 1.178) / 2) ≈ 0.018em。
const PINGFANG_BASELINE_SHIFT: f32 = 0.018;
/// Menlo（提供 ↩ ⇥）作为后备字体时的基线偏差，同理计算约 0.032em。
const MENLO_BASELINE_SHIFT: f32 = 0.032;

const SF_PRO: &str = "/System/Library/Fonts/SFNS.ttf";
const SF_MONO: &str = "/System/Library/Fonts/SFNSMono.ttf";
/// Menlo 提供 SF 中缺少的 ↩ ⇥ 等符号。
const MENLO: &str = "/System/Library/Fonts/Menlo.ttc";
/// 苹方不在 /System/Library/Fonts 时（较新的系统通过资源包下载）的后备中文字体。
const CJK_FALLBACKS: [&str; 3] = [
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
];

pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut proportional = Vec::new();
    let mut monospace = Vec::new();
    let mut semibold = Vec::new();
    let add = |fonts: &mut FontDefinitions, name: &str, data: FontData| {
        fonts.font_data.insert(name.to_string(), Arc::new(data));
        name.to_string()
    };

    // 可变字体默认是 Display 光学尺寸（opsz 28），界面字号用 Text 尺寸更舒展。
    let sf_loaded = Path::new(SF_PRO).exists();
    // 只有主字体是 SF 时才需要校正后备字体的基线。
    let shift = |value: f32| if sf_loaded { value } else { 0.0 };
    if let Ok(bytes) = std::fs::read(SF_PRO) {
        let regular = FontData::from_owned(bytes.clone())
            .tweak(variation(&[(b"wght", 400.0), (b"opsz", 17.0)]));
        let bold =
            FontData::from_owned(bytes).tweak(variation(&[(b"wght", 600.0), (b"opsz", 17.0)]));
        proportional.push(add(&mut fonts, "sf-pro", regular));
        semibold.push(add(&mut fonts, "sf-pro-semibold", bold));
    }
    if let Ok(bytes) = std::fs::read(SF_MONO) {
        // SF Mono 的默认实例是 Light，改为 Regular。
        let regular = FontData::from_owned(bytes).tweak(variation(&[(b"wght", 400.0)]));
        monospace.push(add(&mut fonts, "sf-mono", regular));
    }

    let cjk = find_pingfang().and_then(|path| {
        let bytes = std::fs::read(&path).ok()?;
        let regular = ttc_face_index(&bytes, "PingFang SC Regular")?;
        let semibold = ttc_face_index(&bytes, "PingFang SC Semibold").unwrap_or(regular);
        Some((bytes, regular, semibold))
    });
    match cjk {
        Some((bytes, regular, bold)) => {
            let tweak = FontTweak {
                y_offset_factor: shift(PINGFANG_BASELINE_SHIFT),
                ..Default::default()
            };
            let mut regular_data = FontData::from_owned(bytes.clone()).tweak(tweak.clone());
            regular_data.index = regular;
            let mut bold_data = FontData::from_owned(bytes).tweak(tweak);
            bold_data.index = bold;
            let name = add(&mut fonts, "pingfang", regular_data);
            proportional.push(name.clone());
            monospace.push(name);
            semibold.push(add(&mut fonts, "pingfang-semibold", bold_data));
        }
        None => {
            if let Some(bytes) = CJK_FALLBACKS
                .iter()
                .find_map(|path| std::fs::read(path).ok())
            {
                let name = add(&mut fonts, "cjk", FontData::from_owned(bytes));
                proportional.push(name.clone());
                monospace.push(name.clone());
                semibold.push(name);
            } else {
                tracing::warn!("没有找到中文字体，中文可能无法显示");
            }
        }
    }

    if let Ok(bytes) = std::fs::read(MENLO) {
        let tweak = FontTweak {
            y_offset_factor: shift(MENLO_BASELINE_SHIFT),
            ..Default::default()
        };
        let name = add(
            &mut fonts,
            "menlo",
            FontData::from_owned(bytes).tweak(tweak),
        );
        proportional.push(name.clone());
        monospace.push(name.clone());
        semibold.push(name);
    }

    // 最后接上 egui 自带的字体（含 emoji），作为兜底。
    let defaults = FontDefinitions::default();
    for (family, list) in [
        (FontFamily::Proportional, &mut proportional),
        (FontFamily::Monospace, &mut monospace),
    ] {
        list.extend(defaults.families.get(&family).cloned().unwrap_or_default());
    }
    semibold.extend(proportional.iter().cloned());
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    fonts
}

fn variation(coords: &[(&[u8; 4], f32)]) -> FontTweak {
    FontTweak {
        coords: VariationCoords::new(coords.iter().map(|(tag, value)| (*tag, *value))),
        ..Default::default()
    }
}

/// 苹方在新系统上以资源包形式存放在 `/System/Library/AssetsV2/…/AssetData/PingFang.ttc`，
/// 目录名带哈希，需要搜索。
fn find_pingfang() -> Option<PathBuf> {
    let legacy = Path::new("/System/Library/Fonts/PingFang.ttc");
    if legacy.exists() {
        return Some(legacy.to_path_buf());
    }
    let assets = std::fs::read_dir("/System/Library/AssetsV2").ok()?;
    for bundle in assets.flatten() {
        if !bundle
            .file_name()
            .to_string_lossy()
            .starts_with("com_apple_MobileAsset_Font")
        {
            continue;
        }
        for asset in std::fs::read_dir(bundle.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = asset.path().join("AssetData/PingFang.ttc");
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

/// 在 TTC 字体集合中按全名（name ID 4）查找字面的索引。
///
/// 同一个字面通常有多条全名记录（英文、简体中文“苹方-简 常规体”等），任意一条匹配即可。
pub fn ttc_face_index(data: &[u8], full_name: &str) -> Option<u32> {
    if data.get(..4)? != b"ttcf" {
        return None;
    }
    let count = read_u32(data, 8)?;
    (0..count.min(256)).find(|&index| {
        read_u32(data, 12 + index as usize * 4)
            .map(|offset| face_full_names(data, offset as usize))
            .is_some_and(|names| names.iter().any(|name| name == full_name))
    })
}

fn face_full_names(data: &[u8], face_offset: usize) -> Vec<String> {
    let Some(table_count) = read_u16(data, face_offset + 4) else {
        return Vec::new();
    };
    let name_offset = (0..table_count as usize).find_map(|i| {
        let record = face_offset + 12 + i * 16;
        (data.get(record..record + 4)? == b"name").then(|| read_u32(data, record + 8))?
    });
    let Some(name_offset) = name_offset.map(|offset| offset as usize) else {
        return Vec::new();
    };
    let (Some(record_count), Some(strings)) = (
        read_u16(data, name_offset + 2),
        read_u16(data, name_offset + 4),
    ) else {
        return Vec::new();
    };
    let strings = name_offset + strings as usize;
    (0..record_count as usize)
        .filter_map(|i| {
            let record = name_offset + 6 + i * 12;
            let platform = read_u16(data, record)?;
            if read_u16(data, record + 6)? != 4 {
                return None;
            }
            let length = read_u16(data, record + 8)? as usize;
            let start = strings + read_u16(data, record + 10)? as usize;
            let bytes = data.get(start..start + length)?;
            match platform {
                // Unicode / Windows 平台：UTF-16BE
                0 | 3 => {
                    let units: Vec<u16> = bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_be_bytes(*pair))
                        .collect();
                    String::from_utf16(&units).ok()
                }
                // Mac 平台：英文名是 ASCII
                1 => bytes
                    .is_ascii()
                    .then(|| String::from_utf8_lossy(bytes).into_owned()),
                _ => None,
            }
        })
        .collect()
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pingfang_sc_face() {
        let Some(path) = find_pingfang() else {
            eprintln!("本机没有苹方，跳过");
            return;
        };
        let data = std::fs::read(path).unwrap();
        let regular = ttc_face_index(&data, "PingFang SC Regular").expect("应包含简体常规体");
        let semibold = ttc_face_index(&data, "PingFang SC Semibold").expect("应包含简体中粗体");
        assert_ne!(regular, semibold);
        assert_eq!(ttc_face_index(&data, "No Such Font"), None);
    }

    #[test]
    fn rejects_non_collections() {
        assert_eq!(ttc_face_index(b"\x00\x01\x00\x00rest", "x"), None);
        assert_eq!(ttc_face_index(b"ttcf", "x"), None);
    }

    /// 守护上面两个基线常量：egui 或系统字体的度量变化后，这个测试会失败提醒重新计算。
    #[test]
    fn fallback_baselines_match_sf() {
        if !Path::new(SF_PRO).exists() || find_pingfang().is_none() || !Path::new(MENLO).exists() {
            eprintln!("缺少系统字体，跳过");
            return;
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions());
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        let size = 100.0;
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "H中↩".into(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
        });
        let glyphs = &galley.rows[0].row.glyphs;
        // 字形绘制时还会加上 FontTweak 的 y 偏移。
        let baseline = |index: usize, shift: f32| glyphs[index].pos.y + shift * size;
        let latin = baseline(0, 0.0);
        assert!(
            (baseline(1, PINGFANG_BASELINE_SHIFT) - latin).abs() < 1.0,
            "苹方基线偏差"
        );
        assert!(
            (baseline(2, MENLO_BASELINE_SHIFT) - latin).abs() < 1.0,
            "Menlo 基线偏差"
        );
    }

    #[test]
    fn families_put_system_fonts_first() {
        let fonts = definitions();
        let proportional = &fonts.families[&FontFamily::Proportional];
        if Path::new(SF_PRO).exists() {
            assert_eq!(proportional[0], "sf-pro");
        }
        assert!(fonts
            .families
            .contains_key(&FontFamily::Name(SEMIBOLD.into())));
        // egui 自带字体放在最后兜底（emoji）。
        assert!(proportional.iter().any(|name| name.contains("Emoji")));
    }
}

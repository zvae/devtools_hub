//! 字体：与系统界面一致。
//!
//! - macOS：西文 SF Pro，中文苹方，等宽 SF Mono，Menlo 补充符号；
//! - Windows：西文 Segoe UI，中文微软雅黑 UI，等宽 Cascadia Mono / Consolas，Segoe UI Symbol 补充符号。
//!
//! egui 自带的字体不含中文。egui 把后备字体的字形按“与主字体行高差的一半”居中，
//! 而不是对齐基线，度量差别大的字体混排时中西文会上下错开。这里从字体文件读出
//! 上沿、下沿和行距，自动算出每个后备字体需要的偏移量，任何字体组合都能对齐基线。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{FontData, FontDefinitions, FontFamily};

/// 标题等需要加粗的文字使用的字体族。
pub const SEMIBOLD: &str = "semibold";

/// 可变字体的轴坐标，例如 `[(b"wght", 600.0)]`。
type Coords = &'static [(&'static [u8; 4], f32)];

/// 一个候选字面：文件路径、在字体集合（.ttc）中的全名、可变字体的轴坐标。
struct FaceSpec {
    path: PathBuf,
    full_name: Option<&'static str>,
    coords: Coords,
}

impl FaceSpec {
    /// 单个字体文件中的第一个字面。
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            full_name: None,
            coords: &[],
        }
    }

    /// 字体集合（.ttc）中指定全名的字面。
    fn named(mut self, full_name: &'static str) -> Self {
        self.full_name = Some(full_name);
        self
    }

    /// 可变字体的轴坐标只有 macOS 的候选列表用得到，Windows 上不需要。
    #[cfg(target_os = "macos")]
    fn coords(mut self, coords: Coords) -> Self {
        self.coords = coords;
        self
    }
}

/// 各用途按顺序尝试的候选字面，取第一个能加载的。
#[derive(Default)]
struct Candidates {
    ui: Vec<FaceSpec>,
    ui_semibold: Vec<FaceSpec>,
    cjk: Vec<FaceSpec>,
    cjk_semibold: Vec<FaceSpec>,
    mono: Vec<FaceSpec>,
    symbols: Vec<FaceSpec>,
}

#[cfg(target_os = "macos")]
fn candidates() -> Candidates {
    const SF_PRO: &str = "/System/Library/Fonts/SFNS.ttf";
    // 可变字体默认是 Display 光学尺寸（opsz 28），界面字号用 Text 尺寸更舒展。
    const REGULAR: Coords = &[(b"wght", 400.0), (b"opsz", 17.0)];
    const BOLD: Coords = &[(b"wght", 600.0), (b"opsz", 17.0)];
    let pingfang = find_pingfang();
    let cjk_fallbacks = [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ];
    let mut cjk: Vec<FaceSpec> = pingfang
        .iter()
        .map(|path| FaceSpec::new(path).named("PingFang SC Regular"))
        .collect();
    cjk.extend(cjk_fallbacks.map(FaceSpec::new));
    Candidates {
        ui: vec![FaceSpec::new(SF_PRO).coords(REGULAR)],
        ui_semibold: vec![FaceSpec::new(SF_PRO).coords(BOLD)],
        cjk,
        cjk_semibold: pingfang
            .iter()
            .map(|path| FaceSpec::new(path).named("PingFang SC Semibold"))
            .collect(),
        // SF Mono 的默认实例是 Light，改为 Regular。
        mono: vec![
            FaceSpec::new("/System/Library/Fonts/SFNSMono.ttf").coords(&[(b"wght", 400.0)]),
            FaceSpec::new("/System/Library/Fonts/Menlo.ttc"),
        ],
        symbols: vec![FaceSpec::new("/System/Library/Fonts/Menlo.ttc")],
    }
}

#[cfg(windows)]
fn candidates() -> Candidates {
    let windir =
        std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let system = windir.join("Fonts");
    // 单独为当前用户安装的字体（例如 Cascadia）在这里。
    let user = std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join(r"Microsoft\Windows\Fonts"));
    let font = |name: &str| FaceSpec::new(system.join(name));
    let mut mono = vec![font("CascadiaMono.ttf")];
    if let Some(user) = &user {
        mono.push(FaceSpec::new(user.join("CascadiaMono.ttf")));
    }
    mono.push(font("consola.ttf"));
    Candidates {
        ui: vec![font("segoeui.ttf")],
        ui_semibold: vec![font("seguisb.ttf"), font("segoeui.ttf")],
        // msyh.ttc 中第二个字面是 “Microsoft YaHei UI”，行距更紧凑，适合界面。
        cjk: vec![
            font("msyh.ttc").named("Microsoft YaHei UI"),
            font("msyh.ttc"),
            font("msyh.ttf"),
            font("simsun.ttc"),
        ],
        cjk_semibold: vec![
            font("msyhbd.ttc").named("Microsoft YaHei UI Bold"),
            font("msyhbd.ttc"),
        ],
        mono,
        symbols: vec![font("seguisym.ttf")],
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn candidates() -> Candidates {
    Candidates::default()
}

/// 行度量（以 em 为单位，下沿为负数）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct LineMetrics {
    ascent: f32,
    descent: f32,
    line_gap: f32,
}

impl LineMetrics {
    /// 行高：上沿到下沿再加行距。
    fn height(self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

/// 后备字体的字形需要下移多少（以字号为单位）才能与主字体基线对齐。
///
/// egui 定位字形时：主字体的基线在 `上沿`，后备字体的基线在
/// `后备上沿 + (主行高 - 后备行高) / 2`，两者之差就是需要补偿的偏移。
fn baseline_shift(primary: LineMetrics, fallback: LineMetrics) -> f32 {
    primary.ascent - fallback.ascent - 0.5 * (primary.height() - fallback.height())
}

/// 加载好的字面：常驻内存的文件数据、在集合中的序号、轴坐标与行度量。
#[derive(Clone, Copy)]
struct Face {
    bytes: &'static [u8],
    index: u32,
    coords: Coords,
    metrics: LineMetrics,
}

impl Face {
    /// 转成 egui 的字体数据；`baseline_shift` 见 [`baseline_shift`]。
    fn data(&self, baseline_shift: f32) -> FontData {
        let mut data = FontData::from_static(self.bytes).tweak(FontTweak {
            y_offset_factor: baseline_shift,
            coords: VariationCoords::new(self.coords.iter().map(|(tag, value)| (*tag, *value))),
            ..Default::default()
        });
        data.index = self.index;
        data
    }
}

/// 字体文件只读取一次并常驻内存（字体在程序整个生命周期内都要用），多个字体族共享同一份数据。
fn read_font_file(path: &Path) -> Option<&'static [u8]> {
    static FILES: Mutex<BTreeMap<PathBuf, &'static [u8]>> = Mutex::new(BTreeMap::new());
    let mut files = FILES.lock().unwrap();
    if let Some(bytes) = files.get(path) {
        return Some(bytes);
    }
    let bytes: &'static [u8] = Box::leak(std::fs::read(path).ok()?.into_boxed_slice());
    files.insert(path.to_path_buf(), bytes);
    Some(bytes)
}

/// 按顺序尝试候选字面，返回第一个能读到的。
fn load_first(specs: &[FaceSpec]) -> Option<Face> {
    specs.iter().find_map(|spec| {
        let bytes = read_font_file(&spec.path)?;
        let index = match spec.full_name {
            Some(name) => ttc_face_index(bytes, name)?,
            None => 0,
        };
        Some(Face {
            bytes,
            index,
            coords: spec.coords,
            metrics: line_metrics(bytes, index)?,
        })
    })
}

/// 按平台加载系统字体，组装 egui 的字体族：比例字体、等宽字体与 [`SEMIBOLD`]。
/// 后备字体按主字体的基线做偏移，混排时中西文对齐。
pub fn definitions() -> FontDefinitions {
    let candidates = candidates();
    let ui = load_first(&candidates.ui);
    let cjk = load_first(&candidates.cjk);
    let mono = load_first(&candidates.mono);
    let symbols = load_first(&candidates.symbols);
    let ui_semibold = load_first(&candidates.ui_semibold).or(ui);
    let cjk_semibold = load_first(&candidates.cjk_semibold).or(cjk);
    if cjk.is_none() {
        tracing::warn!("没有找到中文字体，中文可能无法显示");
    }

    let mut fonts = FontDefinitions::default();
    let defaults = FontDefinitions::default();
    let default_family =
        |family: &FontFamily| defaults.families.get(family).cloned().unwrap_or_default();
    // 每个字体族里，第一个字体是主字体，其余字体按主字体的度量计算基线偏移。
    // 同一个字面在不同字体族中的偏移可能不同，所以按“字面@字体族”分别注册。
    let mut family = |family: &str, chain: [(&str, Option<Face>); 3], tail: Vec<String>| {
        let faces: Vec<(&str, Face)> = chain
            .into_iter()
            .filter_map(|(role, face)| face.map(|face| (role, face)))
            .collect();
        let primary = faces.first().map(|(_, face)| face.metrics);
        let mut names = Vec::new();
        for (role, face) in faces {
            let shift = primary.map_or(0.0, |primary| baseline_shift(primary, face.metrics));
            let name = format!("{role}@{family}");
            fonts
                .font_data
                .insert(name.clone(), Arc::new(face.data(shift)));
            names.push(name);
        }
        // 最后接上 egui 自带的字体（含 emoji）兜底。
        names.extend(tail);
        names
    };

    let proportional = family(
        "proportional",
        [("ui", ui), ("cjk", cjk), ("symbols", symbols)],
        default_family(&FontFamily::Proportional),
    );
    let monospace = family(
        "monospace",
        [("mono", mono), ("cjk", cjk), ("symbols", symbols)],
        default_family(&FontFamily::Monospace),
    );
    let semibold = family(
        SEMIBOLD,
        [
            ("ui", ui_semibold),
            ("cjk", cjk_semibold),
            ("symbols", symbols),
        ],
        proportional.clone(),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    fonts
}

/// 苹方在新系统上以资源包形式存放在 `/System/Library/AssetsV2/…/AssetData/PingFang.ttc`，
/// 目录名带哈希，需要搜索。
#[cfg(target_os = "macos")]
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

/// 字体集合（.ttc）中第 `index` 个字面的起始位置；单个字体文件为 0。
fn face_offset(data: &[u8], index: u32) -> Option<usize> {
    if data.starts_with(b"ttcf") {
        read_u32(data, 12 + index as usize * 4).map(|offset| offset as usize)
    } else {
        (index == 0).then_some(0)
    }
}

/// 字面中某张表的起始位置。
fn table(data: &[u8], face: usize, tag: &[u8; 4]) -> Option<usize> {
    let count = read_u16(data, face + 4)? as usize;
    (0..count)
        .find_map(|i| {
            let record = face + 12 + i * 16;
            (data.get(record..record + 4)? == tag).then(|| read_u32(data, record + 8))?
        })
        .map(|offset| offset as usize)
}

/// 与 egui 使用的 skrifa（以及 FreeType）相同的规则选取行度量：
/// OS/2 设置了 USE_TYPO_METRICS 时用 typo 度量；否则用 hhea；
/// hhea 上下沿都为 0 时再退回 typo，typo 也为 0 时用 win 度量。
fn line_metrics(data: &[u8], index: u32) -> Option<LineMetrics> {
    let face = face_offset(data, index)?;
    let units_per_em = f32::from(read_u16(data, table(data, face, b"head")? + 18)?);
    if units_per_em <= 0.0 {
        return None;
    }
    let hhea = table(data, face, b"hhea")?;
    let os2 = table(data, face, b"OS/2");
    let signed = |at: usize| read_u16(data, at).map(|value| f32::from(value as i16));
    let typo = os2.and_then(|os2| Some((signed(os2 + 68)?, signed(os2 + 70)?, signed(os2 + 72)?)));
    let use_typo = os2
        .and_then(|os2| read_u16(data, os2 + 62))
        .is_some_and(|selection| selection & (1 << 7) != 0);
    let hhea_metrics = (signed(hhea + 4)?, signed(hhea + 6)?, signed(hhea + 8)?);
    let (ascent, descent, line_gap) = match typo {
        Some(typo) if use_typo => typo,
        _ if hhea_metrics.0 != 0.0 || hhea_metrics.1 != 0.0 => hhea_metrics,
        Some(typo) if typo.0 != 0.0 || typo.1 != 0.0 => typo,
        _ => match os2 {
            Some(os2) => (
                f32::from(read_u16(data, os2 + 74)?),
                -f32::from(read_u16(data, os2 + 76)?),
                hhea_metrics.2,
            ),
            None => hhea_metrics,
        },
    };
    Some(LineMetrics {
        ascent: ascent / units_per_em,
        descent: descent / units_per_em,
        line_gap: line_gap / units_per_em,
    })
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

/// 字面 `name` 表中的全名（name id 4），各平台、各语言的写法都列出来。
fn face_full_names(data: &[u8], face: usize) -> Vec<String> {
    let Some(name_table) = table(data, face, b"name") else {
        return Vec::new();
    };
    let (Some(record_count), Some(strings)) = (
        read_u16(data, name_table + 2),
        read_u16(data, name_table + 4),
    ) else {
        return Vec::new();
    };
    let strings = name_table + strings as usize;
    (0..record_count as usize)
        .filter_map(|i| {
            let record = name_table + 6 + i * 12;
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

/// 按大端读取（字体文件里的整数都是大端）。
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
    fn rejects_non_collections() {
        assert_eq!(ttc_face_index(b"\x00\x01\x00\x00rest", "x"), None);
        assert_eq!(ttc_face_index(b"ttcf", "x"), None);
        assert_eq!(line_metrics(b"garbage", 0), None);
    }

    #[test]
    fn baseline_shift_of_primary_is_zero() {
        let sf = LineMetrics {
            ascent: 0.967,
            descent: -0.211,
            line_gap: 0.0,
        };
        let pingfang = LineMetrics {
            ascent: 1.06,
            descent: -0.34,
            line_gap: 0.0,
        };
        assert_eq!(baseline_shift(sf, sf), 0.0);
        // 与此前实测的苹方偏差（约 0.018em）一致。
        assert!((baseline_shift(sf, pingfang) - 0.018).abs() < 0.001);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_system_font_metrics() {
        let Some(path) = find_pingfang() else {
            eprintln!("本机没有苹方，跳过");
            return;
        };
        let data = std::fs::read(path).unwrap();
        let regular = ttc_face_index(&data, "PingFang SC Regular").expect("应包含简体常规体");
        let semibold = ttc_face_index(&data, "PingFang SC Semibold").expect("应包含简体中粗体");
        assert_ne!(regular, semibold);
        assert_eq!(ttc_face_index(&data, "No Such Font"), None);
        let metrics = line_metrics(&data, regular).unwrap();
        assert!((metrics.ascent - 1.06).abs() < 0.001 && (metrics.descent + 0.34).abs() < 0.001);
        let sf =
            line_metrics(&std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap(), 0).unwrap();
        assert!((sf.ascent - 0.967).abs() < 0.001 && (sf.height() - 1.178).abs() < 0.001);
    }

    /// 用 egui 实际排版验证：每个后备字体加上计算出的偏移后，与主字体的基线对齐。
    #[test]
    fn fallback_baselines_match_primary() {
        let definitions = definitions();
        let shift = |name: &str| {
            definitions
                .font_data
                .get(name)
                .map(|data| data.tweak.y_offset_factor)
        };
        let (Some(cjk_shift), Some(_)) = (shift("cjk@proportional"), shift("ui@proportional"))
        else {
            eprintln!("缺少系统字体，跳过");
            return;
        };
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions.clone());
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        let size = 100.0;
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "H中".into(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
        });
        let glyphs = &galley.rows[0].row.glyphs;
        // 绘制时字形还会加上 FontTweak 的 y 偏移。
        let latin = glyphs[0].pos.y;
        let cjk = glyphs[1].pos.y + cjk_shift * size;
        assert!((cjk - latin).abs() < 1.0, "中文基线偏差 {}", cjk - latin);
    }

    #[test]
    fn families_put_system_fonts_first() {
        let fonts = definitions();
        let proportional = &fonts.families[&FontFamily::Proportional];
        if fonts.font_data.contains_key("ui@proportional") {
            assert_eq!(proportional[0], "ui@proportional");
        }
        assert!(fonts
            .families
            .contains_key(&FontFamily::Name(SEMIBOLD.into())));
        // egui 自带字体放在最后兜底（emoji）。
        assert!(proportional.iter().any(|name| name.contains("Emoji")));
    }
}

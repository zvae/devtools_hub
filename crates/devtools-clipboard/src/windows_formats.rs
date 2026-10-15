//! Windows 剪贴板格式与平台无关表示（UTI）之间的转换。
//!
//! 存储层和内容分析都以 UTI 为键（`public.utf8-plain-text`、`public.png`……），
//! 这里把 Windows 的剪贴板格式翻译成同样的表示，写回时再翻译回去。
//! 纯 Rust 实现、不调用 Win32 API，因此在任何平台上都能测试。

use std::collections::HashSet;
use std::io::Cursor;

use crate::model::{Representation, Snapshot};
use crate::uti;

pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_TIFF: u32 = 6;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_HDROP: u32 = 15;
pub const CF_LOCALE: u32 = 16;
pub const CF_DIBV5: u32 = 17;
pub const CF_OWNERDISPLAY: u32 = 0x80;
pub const CF_DSPBITMAP: u32 = 0x82;
pub const CF_DSPMETAFILEPICT: u32 = 0x83;
pub const CF_DSPENHMETAFILE: u32 = 0x8E;

pub const HTML_FORMAT: &str = "HTML Format";
pub const RTF_FORMAT: &str = "Rich Text Format";
pub const PNG_FORMAT: &str = "PNG";
pub const URL_FORMAT: &str = "UniformResourceLocatorW";

/// 密码管理器等用来声明“不要记录”的格式（KeePass、1Password、Bitwarden 都会设置）。
pub const EXCLUDE_FROM_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const VIEWER_IGNORE: &str = "Clipboard Viewer Ignore";
/// 值为 0 时表示不要进入剪贴板历史（Windows 10 起的约定）。
pub const CAN_INCLUDE_IN_HISTORY: &str = "CanIncludeInClipboardHistory";
const CAN_UPLOAD_TO_CLOUD: &str = "CanUploadToCloudClipboard";

/// 不保存的格式：OLE 内部数据、与 CF_HDROP 重复的文件描述，
/// 以及 “Preferred DropEffect”（它表示剪切还是复制，从历史粘贴文件时不应变成移动）。
const SKIPPED_FORMATS: [&str; 10] = [
    "Ole Private Data",
    "Object Descriptor",
    "Link Source Descriptor",
    "DataObject",
    "Preferred DropEffect",
    "Shell Object Offsets",
    "Shell IDList Array",
    "FileName",
    "FileNameW",
    "UniformResourceLocator",
];

/// 没有对应 UTI 的格式原样保存，类型名为此前缀加格式名（标准格式用 `#编号`）。
pub const RAW_PREFIX: &str = "com.microsoft.windows.clipboard.";

/// 从剪贴板读到的一种格式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFormat {
    pub id: u32,
    /// 注册格式的名称；标准格式（CF_*）为 `None`。
    pub name: Option<String>,
    pub data: Vec<u8>,
}

/// 写回剪贴板时的目标格式。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FormatTarget {
    Standard(u32),
    /// 需要先用 `RegisterClipboardFormatW` 取得编号。
    Registered(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatData {
    pub target: FormatTarget,
    pub data: Vec<u8>,
}

/// 是否需要读取该格式的数据：GDI 句柄型格式无法按内存块读取，
/// 由系统自动合成的格式（CF_TEXT、CF_DIBV5 等）与已保存的格式重复。
pub fn should_read(id: u32) -> bool {
    !matches!(
        id,
        CF_TEXT
            | CF_OEMTEXT
            | CF_LOCALE
            | CF_DIBV5
            | CF_BITMAP
            | CF_METAFILEPICT
            | CF_PALETTE
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
    )
}

/// 隐私标记格式对应的 UTI（统一映射为 Concealed，由过滤规则决定是否跳过）。
pub fn marker_uti(name: &str, data: &[u8]) -> Option<&'static str> {
    match name {
        EXCLUDE_FROM_MONITOR | VIEWER_IGNORE => Some(uti::CONCEALED),
        CAN_INCLUDE_IN_HISTORY if read_u32(data, 0) == Some(0) => Some(uti::CONCEALED),
        _ => None,
    }
}

/// 只看格式列表（用于读取数据之前的隐私判断）。
pub fn type_names(formats: &[ClipboardFormat]) -> Vec<String> {
    formats
        .iter()
        .map(|format| match format.name.as_deref() {
            Some(name) => marker_uti(name, &format.data)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{RAW_PREFIX}{name}")),
            None => format!("{RAW_PREFIX}#{}", format.id),
        })
        .collect()
}

/// Windows 剪贴板内容 → 快照。
///
/// Windows 剪贴板只有一个“项目”，多个文件都放在同一个 CF_HDROP 里；
/// 为了和 macOS 的结构一致（每个文件一个 item），第一个文件放在主 item，其余各占一个 item。
pub fn snapshot_from_windows(formats: &[ClipboardFormat]) -> Snapshot {
    let mut main = Vec::new();
    let mut extra_files = Vec::new();
    for format in formats {
        let data = &format.data;
        match (format.id, format.name.as_deref()) {
            (CF_UNICODETEXT, _) => {
                main.push(Representation::new(uti::PLAIN_TEXT, utf16_to_string(data)));
            }
            (CF_DIB, _) => {
                if let Some(bmp) = dib_to_bmp(data) {
                    main.push(Representation::new(uti::BMP, bmp));
                }
            }
            (CF_TIFF, _) => main.push(Representation::new(uti::TIFF, data.clone())),
            (CF_HDROP, _) => {
                for (index, path) in hdrop_decode(data).iter().enumerate() {
                    let rep = Representation::new(uti::FILE_URL, uti::path_to_file_url(path));
                    if index == 0 {
                        main.push(rep);
                    } else {
                        extra_files.push(vec![rep]);
                    }
                }
            }
            (_, Some(name)) => {
                if let Some(marker) = marker_uti(name, data) {
                    main.push(Representation::new(marker, Vec::new()));
                    continue;
                }
                match name {
                    HTML_FORMAT => {
                        if let Some(html) = cf_html_decode(data) {
                            main.push(Representation::new(uti::HTML, html));
                        }
                    }
                    RTF_FORMAT => main.push(Representation::new(uti::RTF, data.clone())),
                    PNG_FORMAT | "image/png" => {
                        main.push(Representation::new(uti::PNG, data.clone()))
                    }
                    URL_FORMAT => main.push(Representation::new(uti::URL, utf16_to_string(data))),
                    _ if SKIPPED_FORMATS.contains(&name)
                        || name == CAN_INCLUDE_IN_HISTORY
                        || name == CAN_UPLOAD_TO_CLOUD => {}
                    _ => main.push(Representation::new(
                        format!("{RAW_PREFIX}{name}"),
                        data.clone(),
                    )),
                }
            }
            (id, None) => main.push(Representation::new(
                format!("{RAW_PREFIX}#{id}"),
                data.clone(),
            )),
        }
    }
    let mut items = Vec::new();
    if !main.is_empty() {
        items.push(main);
    }
    items.extend(extra_files);
    Snapshot { items }
}

/// 快照 → 需要写入 Windows 剪贴板的格式。
///
/// macOS 专有的类型会被忽略；只有 PNG/JPEG/TIFF 等图片而没有 DIB 时，
/// 额外生成一份 CF_DIB，让不认识 PNG 的程序也能粘贴图片。
pub fn windows_formats_from_snapshot(snapshot: &Snapshot) -> Vec<FormatData> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |out: &mut Vec<FormatData>, target: FormatTarget, data: Vec<u8>| {
        if seen.insert(target.clone()) {
            out.push(FormatData { target, data });
        }
    };
    let mut files = Vec::new();
    let mut image_source: Option<&[u8]> = None;
    let mut has_dib = false;

    for rep in snapshot.representations() {
        let text = || String::from_utf8_lossy(&rep.data).into_owned();
        match rep.uti.as_str() {
            uti::PLAIN_TEXT => push(
                &mut out,
                FormatTarget::Standard(CF_UNICODETEXT),
                string_to_utf16(&text()),
            ),
            uti::HTML => push(
                &mut out,
                FormatTarget::Registered(HTML_FORMAT.into()),
                cf_html_encode(&text()),
            ),
            uti::RTF => push(
                &mut out,
                FormatTarget::Registered(RTF_FORMAT.into()),
                rep.data.clone(),
            ),
            uti::URL => push(
                &mut out,
                FormatTarget::Registered(URL_FORMAT.into()),
                string_to_utf16(&text()),
            ),
            uti::PNG => {
                image_source.get_or_insert(&rep.data);
                push(
                    &mut out,
                    FormatTarget::Registered(PNG_FORMAT.into()),
                    rep.data.clone(),
                );
            }
            uti::BMP => {
                if let Some(dib) = bmp_to_dib(&rep.data) {
                    has_dib = true;
                    push(&mut out, FormatTarget::Standard(CF_DIB), dib.to_vec());
                }
            }
            uti::TIFF | uti::JPEG | uti::GIF | uti::WEBP => {
                image_source.get_or_insert(&rep.data);
            }
            uti::FILE_URL => {
                if let Some(path) = uti::file_url_to_path(&rep.data) {
                    files.push(path);
                }
            }
            other => {
                if let Some(name) = other.strip_prefix(RAW_PREFIX) {
                    let target = match name.strip_prefix('#').and_then(|id| id.parse().ok()) {
                        Some(id) => FormatTarget::Standard(id),
                        None => FormatTarget::Registered(name.to_string()),
                    };
                    push(&mut out, target, rep.data.clone());
                }
                // 其余（macOS 专有类型、隐私标记）不写回。
            }
        }
    }
    if !files.is_empty() {
        push(
            &mut out,
            FormatTarget::Standard(CF_HDROP),
            hdrop_encode(&files),
        );
    }
    if !has_dib {
        if let Some(dib) = image_source.and_then(image_to_dib) {
            push(&mut out, FormatTarget::Standard(CF_DIB), dib);
        }
    }
    out
}

/// UTF-16LE（以 NUL 结尾）→ UTF-8。
pub fn utf16_to_string(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// UTF-8 → UTF-16LE，带 NUL 结尾（CF_UNICODETEXT 的格式）。
pub fn string_to_utf16(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// 解析 CF_HTML：头部记录了 HTML 在数据中的字节偏移。
pub fn cf_html_decode(data: &[u8]) -> Option<String> {
    let header_end = data.iter().position(|&b| b == b'<').unwrap_or(data.len());
    let header = String::from_utf8_lossy(&data[..header_end]);
    let offset = |key: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|value| value.trim().parse::<i64>().ok())
            .and_then(|value| usize::try_from(value).ok())
    };
    let range = match (offset("StartHTML"), offset("EndHTML")) {
        (Some(start), Some(end)) if start < end && end <= data.len() => start..end,
        _ => {
            let (start, end) = (offset("StartFragment")?, offset("EndFragment")?);
            (start < end && end <= data.len()).then_some(start..end)?
        }
    };
    Some(String::from_utf8_lossy(&data[range]).into_owned())
}

const START_FRAGMENT: &str = "<!--StartFragment-->";
const END_FRAGMENT: &str = "<!--EndFragment-->";

/// 生成 CF_HTML：固定宽度的偏移量头部 + HTML（必要时补上片段标记）。
pub fn cf_html_encode(html: &str) -> Vec<u8> {
    let body = if html.contains(START_FRAGMENT) && html.contains(END_FRAGMENT) {
        html.to_string()
    } else {
        format!("<html><body>{START_FRAGMENT}{html}{END_FRAGMENT}</body></html>")
    };
    let header =
        |start_html: usize, end_html: usize, start_fragment: usize, end_fragment: usize| {
            format!(
                "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
             StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
            )
        };
    let header_len = header(0, 0, 0, 0).len();
    let start_fragment = header_len + body.find(START_FRAGMENT).unwrap_or(0) + START_FRAGMENT.len();
    let end_fragment = header_len + body.rfind(END_FRAGMENT).unwrap_or(body.len());
    let mut out = header(
        header_len,
        header_len + body.len(),
        start_fragment,
        end_fragment,
    )
    .into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// CF_DIB（不带文件头的位图）→ BMP 文件，便于生成缩略图和预览。
pub fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    let header_size = read_u32(dib, 0)? as usize;
    if header_size < 12 || dib.len() < header_size {
        return None;
    }
    let (bit_count, compression, colors_used) = if header_size == 12 {
        (read_u16(dib, 10)? as u32, 0, 0)
    } else {
        (
            read_u16(dib, 14)? as u32,
            read_u32(dib, 16)?,
            read_u32(dib, 32).unwrap_or(0),
        )
    };
    let palette_entries = if colors_used > 0 {
        colors_used as usize
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    let entry_size = if header_size == 12 { 3 } else { 4 };
    // BI_BITFIELDS / BI_ALPHABITFIELDS 在 40 字节头之后紧跟颜色掩码。
    let masks = match (compression, header_size) {
        (3, 40) => 12,
        (6, 40) => 16,
        _ => 0,
    };
    let pixel_offset = 14 + header_size + masks + palette_entries * entry_size;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Some(bmp)
}

/// BMP 文件 → CF_DIB（去掉 14 字节文件头）。
pub fn bmp_to_dib(bmp: &[u8]) -> Option<&[u8]> {
    (bmp.len() > 14 && bmp.starts_with(b"BM")).then(|| &bmp[14..])
}

/// 任意图片（PNG/JPEG/TIFF…）→ 32 位 CF_DIB。
pub fn image_to_dib(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut dib = Vec::with_capacity(40 + (width * height * 4) as usize);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(width as i32).to_le_bytes());
    // 正的高度表示自下而上存储，兼容性最好。
    dib.extend_from_slice(&(height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib.extend_from_slice(&(width * height * 4).to_le_bytes());
    dib.extend_from_slice(&[0; 16]); // 分辨率与调色板字段
    for row in image.rows().rev() {
        for pixel in row {
            let [r, g, b, a] = pixel.0;
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(dib)
}

/// 解析 CF_HDROP（DROPFILES 结构 + 以双 NUL 结尾的路径列表）。
pub fn hdrop_decode(data: &[u8]) -> Vec<String> {
    let (Some(offset), Some(wide)) = (read_u32(data, 0), read_u32(data, 16)) else {
        return Vec::new();
    };
    let Some(list) = data.get(offset as usize..) else {
        return Vec::new();
    };
    let paths: Vec<String> = if wide != 0 {
        let units: Vec<u16> = list
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        units
            .split(|&unit| unit == 0)
            .take_while(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    } else {
        list.split(|&byte| byte == 0)
            .take_while(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect()
    };
    paths
}

/// 生成 CF_HDROP（宽字符路径）。
pub fn hdrop_encode(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles：路径列表紧跟在 20 字节的结构之后
    out.extend_from_slice(&[0; 8]); // pt
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.encode_utf16().chain(std::iter::once(0)) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uti::analyze;
    use crate::ContentKind;

    fn registered(name: &str, data: Vec<u8>) -> ClipboardFormat {
        ClipboardFormat {
            id: 0xC000,
            name: Some(name.into()),
            data,
        }
    }

    fn standard(id: u32, data: Vec<u8>) -> ClipboardFormat {
        ClipboardFormat {
            id,
            name: None,
            data,
        }
    }

    #[test]
    fn utf16_round_trip() {
        let text = "你好，Windows 剪贴板 😀";
        assert_eq!(utf16_to_string(&string_to_utf16(text)), text);
    }

    #[test]
    fn cf_html_round_trip_keeps_fragment() {
        let encoded = cf_html_encode("<b>粗体</b>");
        let text = String::from_utf8(encoded.clone()).unwrap();
        assert!(text.starts_with("Version:0.9\r\nStartHTML:"));
        let html = cf_html_decode(&encoded).unwrap();
        assert!(html.contains("<!--StartFragment--><b>粗体</b><!--EndFragment-->"));
        // 已带片段标记的 HTML 再次编码时保持不变。
        assert_eq!(cf_html_decode(&cf_html_encode(&html)).unwrap(), html);
        // 片段偏移量指向正确的位置。
        let start: usize = text
            .lines()
            .find_map(|l| l.strip_prefix("StartFragment:"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(encoded[start..].starts_with("<b>粗体</b>".as_bytes()));
    }

    #[test]
    fn dib_bmp_and_image_conversions() {
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let dib = image_to_dib(&png).unwrap();
        let bmp = dib_to_bmp(&dib).unwrap();
        assert_eq!(bmp_to_dib(&bmp), Some(dib.as_slice()));
        let decoded = image::load_from_memory(&bmp).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(2, 1).0, [10, 20, 30, 255]);
        assert!(dib_to_bmp(b"tiny").is_none());
    }

    #[test]
    fn hdrop_round_trip() {
        let paths = vec![
            r"C:\Users\我\文档 1.txt".to_string(),
            r"\\server\share\a.zip".to_string(),
        ];
        assert_eq!(hdrop_decode(&hdrop_encode(&paths)), paths);
        assert!(hdrop_decode(b"short").is_empty());
    }

    #[test]
    fn reads_text_html_and_markers() {
        let snapshot = snapshot_from_windows(&[
            standard(CF_UNICODETEXT, string_to_utf16("hello")),
            registered(HTML_FORMAT, cf_html_encode("<i>hello</i>")),
            registered("Ole Private Data", vec![1, 2, 3]),
            registered("Csv", b"a,b".to_vec()),
        ]);
        let analysis = analyze(&snapshot);
        assert_eq!(analysis.kind, ContentKind::RichText);
        assert_eq!(analysis.text, "hello");
        assert!(snapshot.has_type(&format!("{RAW_PREFIX}Csv")));
        assert!(!snapshot.has_type(&format!("{RAW_PREFIX}Ole Private Data")));

        let secret = [
            standard(CF_UNICODETEXT, string_to_utf16("p@ss")),
            registered(CAN_INCLUDE_IN_HISTORY, 0u32.to_le_bytes().to_vec()),
        ];
        assert!(uti::is_concealed(&snapshot_from_windows(&secret)));
        assert!(type_names(&secret).contains(&uti::CONCEALED.to_string()));
        let allowed = [registered(
            CAN_INCLUDE_IN_HISTORY,
            1u32.to_le_bytes().to_vec(),
        )];
        assert!(!type_names(&allowed).contains(&uti::CONCEALED.to_string()));
        assert!(uti::is_concealed(&snapshot_from_windows(&[registered(
            EXCLUDE_FROM_MONITOR,
            vec![]
        )])));
    }

    #[test]
    fn files_become_one_item_each_and_round_trip() {
        let paths = vec![r"C:\a b\x.txt".to_string(), r"D:\y.png".to_string()];
        let snapshot = snapshot_from_windows(&[standard(CF_HDROP, hdrop_encode(&paths))]);
        assert_eq!(snapshot.items.len(), 2);
        let analysis = analyze(&snapshot);
        assert_eq!(analysis.kind, ContentKind::Files);
        assert_eq!(analysis.file_paths, paths);

        let formats = windows_formats_from_snapshot(&snapshot);
        let hdrop = formats
            .iter()
            .find(|f| f.target == FormatTarget::Standard(CF_HDROP))
            .unwrap();
        assert_eq!(hdrop_decode(&hdrop.data), paths);
    }

    #[test]
    fn writes_text_html_png_and_raw_formats() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let snapshot = Snapshot {
            items: vec![vec![
                Representation::new(uti::PLAIN_TEXT, "hi"),
                Representation::new(uti::HTML, "<b>hi</b>"),
                Representation::new(uti::PNG, png.clone()),
                Representation::new(format!("{RAW_PREFIX}Csv"), b"a,b".to_vec()),
                Representation::new(format!("{RAW_PREFIX}#11"), b"riff".to_vec()),
                Representation::new("com.apple.flat-rtfd", b"mac only".to_vec()),
                Representation::new(uti::CONCEALED, Vec::new()),
            ]],
        };
        let formats = windows_formats_from_snapshot(&snapshot);
        let find = |target: FormatTarget| {
            formats
                .iter()
                .find(|f| f.target == target)
                .map(|f| f.data.clone())
        };
        assert_eq!(
            utf16_to_string(&find(FormatTarget::Standard(CF_UNICODETEXT)).unwrap()),
            "hi"
        );
        assert!(
            cf_html_decode(&find(FormatTarget::Registered(HTML_FORMAT.into())).unwrap())
                .unwrap()
                .contains("<b>hi</b>")
        );
        assert_eq!(
            find(FormatTarget::Registered(PNG_FORMAT.into())).unwrap(),
            png
        );
        assert!(
            find(FormatTarget::Standard(CF_DIB)).is_some(),
            "只有 PNG 时补一份 DIB"
        );
        assert_eq!(
            find(FormatTarget::Registered("Csv".into())).unwrap(),
            b"a,b"
        );
        assert_eq!(find(FormatTarget::Standard(11)).unwrap(), b"riff");
        assert_eq!(formats.len(), 6, "macOS 专有类型与隐私标记不写回");
    }

    #[test]
    fn plain_text_snapshot_writes_only_unicode_text() {
        let formats = windows_formats_from_snapshot(&Snapshot::from_text("纯文本"));
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].target, FormatTarget::Standard(CF_UNICODETEXT));
    }

    #[test]
    fn skips_handle_and_synthesized_formats() {
        assert!(!should_read(CF_BITMAP));
        assert!(!should_read(CF_TEXT));
        assert!(should_read(CF_UNICODETEXT));
        assert!(should_read(0xC123));
    }
}

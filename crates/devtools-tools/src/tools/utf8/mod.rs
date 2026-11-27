//! UTF-8 编解码工具：界面复用 `support::codec`，算法与测试留在本目录的 model。

mod model;

use crate::support::codec::{self, Spec, VariantHelp};
use crate::support::style;
use devtools_module_api::{ModuleDescriptor, Selection};
use egui::{Align2, Color32, FontId, Rect, Ui};

static SPEC: Spec = Spec {
    title: "UTF-8 编解码",
    subtitle: "查看文本的 UTF-8 字节，或把字节序列还原为文本",
    modes: ["文本 → UTF-8 字节", "UTF-8 字节 → 文本"],
    variant_label: "字节格式",
    variants: &["\\x 转义", "十六进制", "十进制"],
    variant_help: &[
        VariantHelp {
            description:
                "每个字节写成 \\xHH，常见于 C、Python 与日志；解码时未转义的字符原样保留。",
            example: "中  →  \\xE4\\xB8\\xAD",
        },
        VariantHelp {
            description:
                "每个字节写成两位十六进制并用空格分隔；解码也接受 0x 前缀、逗号和连续写法。",
            example: "中  →  E4 B8 AD",
        },
        VariantHelp {
            description: "每个字节写成 0–255 的十进制数并用逗号分隔；解码也接受方括号数组。",
            example: "中  →  228, 184, 173",
        },
    ],
    encode_hint: "输入要查看字节的文本",
    decode_hint: "粘贴字节序列，例如 \\xE4\\xB8\\xAD 或 E4 B8 AD",
    encode: model::encode,
    decode: model::decode,
    detect: model::detect,
};

fn paint_icon(ui: &Ui, rect: Rect, _palette: &style::Palette) {
    ui.painter().rect_filled(
        rect,
        style::SMALL_RADIUS,
        Color32::from_rgb(0xc0, 0x3b, 0x7a),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "U8",
        FontId::monospace(rect.height() * 0.4),
        Color32::WHITE,
    );
}

pub(super) fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "devtools.utf8",
        name: "UTF-8 编解码",
        description: "文本与 UTF-8 字节序列互转",
        order: 60,
        size: [680.0, 560.0],
        toast_offset: -48.0,
        matches: |input| matches!(input, Selection::Text(text) if model::detect(text).is_some()),
        paint_icon,
        create: |_| codec::create(&SPEC),
        validate_config: |config| config.is_object(),
        default_hotkey: "",
    }
}

#[cfg(test)]
mod tests;

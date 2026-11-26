//! Base64 编解码工具：界面复用 `support::codec`，算法与测试留在本目录的 model。

mod model;

use crate::support::codec::{self, Spec, VariantHelp};
use crate::support::style;
use devtools_module_api::{ModuleDescriptor, Selection};
use egui::{Align2, Color32, FontId, Rect, Ui};

static SPEC: Spec = Spec {
    title: "Base64 编解码",
    subtitle: "文本按 UTF-8 编码为 Base64，或把 Base64 还原为文本",
    modes: ["文本 → Base64", "Base64 → 文本"],
    variant_label: "字母表",
    variants: &["标准", "URL 安全"],
    variant_help: &[
        VariantHelp {
            description: "RFC 4648 标准字母表，使用 + 和 /，末尾用 = 补齐。",
            example: ">>>???  →  Pj4+Pz8/",
        },
        VariantHelp {
            description: "URL 安全字母表，用 - 和 _ 代替 + 和 /，不带 = 填充，常见于 JWT 与链接参数。解码时两种字母表都接受。",
            example: ">>>???  →  Pj4-Pz8_",
        },
    ],
    encode_hint: "输入要编码的文本",
    decode_hint: "粘贴 Base64，自动忽略空格与换行",
    encode: model::encode,
    decode: model::decode,
    detect: model::detect,
};

fn paint_icon(ui: &Ui, rect: Rect, _palette: &style::Palette) {
    ui.painter().rect_filled(
        rect,
        style::SMALL_RADIUS,
        Color32::from_rgb(0xd9, 0x77, 0x06),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "64",
        FontId::monospace(rect.height() * 0.42),
        Color32::WHITE,
    );
}

pub(super) fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "devtools.base64",
        name: "Base64 编解码",
        description: "Base64 编码与解码，支持 URL 安全字母表",
        order: 40,
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

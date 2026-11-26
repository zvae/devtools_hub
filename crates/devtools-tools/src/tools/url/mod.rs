//! URL 编解码工具：界面复用 `support::codec`，算法与测试留在本目录的 model。

mod model;

use crate::support::codec::{self, Spec, VariantHelp};
use crate::support::style;
use devtools_module_api::{ModuleDescriptor, Selection};
use egui::{Align2, Color32, FontId, Rect, Ui};

static SPEC: Spec = Spec {
    title: "URL 编解码",
    subtitle: "百分号编码（%XX），规则与浏览器的 encodeURIComponent、encodeURI 和表单提交一致",
    modes: ["文本 → URL 编码", "URL 编码 → 文本"],
    variant_label: "编码规则",
    variants: &["组件", "完整 URL", "表单"],
    variant_help: &[
        VariantHelp {
            description: "同 encodeURIComponent：只保留字母、数字和 - _ . ! ~ * ' ( )，适合编码查询参数的值。",
            example: "a b&c=中  →  a%20b%26c%3D%E4%B8%AD",
        },
        VariantHelp {
            description: "同 encodeURI：额外保留 ; , / ? : @ & = + $ # 等 URL 结构字符，适合编码整条链接。",
            example: "/搜索?q=a b  →  /%E6%90%9C%E7%B4%A2?q=a%20b",
        },
        VariantHelp {
            description: "application/x-www-form-urlencoded：空格编码为 +，解码时 + 视为空格。",
            example: "a b&c  →  a+b%26c",
        },
    ],
    encode_hint: "输入要编码的文本或链接",
    decode_hint: "粘贴含 %XX 的文本，例如 %E4%B8%AD%E6%96%87",
    encode: model::encode,
    decode: model::decode,
    detect: model::detect,
};

fn paint_icon(ui: &Ui, rect: Rect, _palette: &style::Palette) {
    ui.painter().rect_filled(
        rect,
        style::SMALL_RADIUS,
        Color32::from_rgb(0x25, 0x63, 0xeb),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "%",
        FontId::monospace(rect.height() * 0.5),
        Color32::WHITE,
    );
}

pub(super) fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "devtools.url",
        name: "URL 编解码",
        description: "URL 百分号编码与解码",
        order: 50,
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

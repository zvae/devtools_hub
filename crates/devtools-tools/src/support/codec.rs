//! 编码类工具（Base64、URL、UTF-8）共用的界面与实例：编码／解码两个方向、可选变体、输入即转换。
//! 具体算法留在各工具目录的 model 中，这里只负责布局、结果缓存与宿主请求；
//! 新增同类工具只需提供一份 [`Spec`]，不必复制整套界面代码。
//!
//! 布局取舍：外层不用滚动区域，输入框与结果框平分窗口剩余高度。这样默认尺寸下内容正好铺满，
//! 用户拉高、拉低窗口（宿主会记住尺寸）时也不会出现整页滚动条，长文本只在各自框内滚动。

use devtools_module_api::{Selection, ToolEffect, ToolModule};
use egui::{
    Align, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense, Stroke,
    TextWrapMode, Ui, Vec2,
};

use crate::support::drag_window;
use crate::support::icons::Icon;
use crate::support::{style, widgets};

/// 转换函数：输入文本与当前变体下标，返回结果或给用户看的错误说明。
pub(crate) type Convert = fn(&str, usize) -> Result<String, String>;

/// 文本框内边距（上下合计）与描边，用于从剩余高度中扣除。
const BOX_CHROME: f32 = 10.0 * 2.0 + 2.0;
/// 文本框滚动区的最小高度；窗口过矮时宁可裁掉底部说明，也不让输入区缩没。
const MIN_BOX: f32 = 48.0;
/// 结果标题行（含按钮）的高度。
const OUTPUT_HEADER: f32 = 28.0;
/// 底部说明文字的行高。
const FOOTER: f32 = 16.0;

/// 变体的悬停说明：一句话描述加一个转换示例。
pub(crate) struct VariantHelp {
    pub description: &'static str,
    /// 转换示例，按原样以等宽字体显示；为空时不显示示例块。
    pub example: &'static str,
}

/// 一个编码工具的全部差异：文案、变体与算法入口。
pub(crate) struct Spec {
    pub title: &'static str,
    pub subtitle: &'static str,
    /// 两个方向的标签，依次为编码、解码。
    pub modes: [&'static str; 2],
    /// 变体分段控件左侧的说明；`variants` 为空时整行不显示。
    pub variant_label: &'static str,
    pub variants: &'static [&'static str],
    /// 与 `variants` 一一对应的悬停说明。
    pub variant_help: &'static [VariantHelp],
    pub encode_hint: &'static str,
    pub decode_hint: &'static str,
    pub encode: Convert,
    pub decode: Convert,
    /// 判断选中文本是否已是编码结果；是则返回解码时应使用的变体，否则按原文编码。
    pub detect: fn(&str) -> Option<usize>,
}

/// 上次转换的输入与结果；输入不变时不重复计算，大段文本也不会每帧重算。
struct Cached {
    input: String,
    decode: bool,
    variant: usize,
    result: Result<String, String>,
}

#[derive(Default)]
pub(crate) struct CodecView {
    pub pinned: bool,
    pub decode: bool,
    pub variant: usize,
    pub input: String,
    cache: Option<Cached>,
}

impl CodecView {
    /// 选中内容带入：已编码的文本切到解码方向，其余文本按当前变体编码。
    fn load(&mut self, spec: &Spec, text: &str) {
        match (spec.detect)(text) {
            Some(variant) => {
                self.decode = true;
                self.variant = variant.min(spec.variants.len().saturating_sub(1));
            }
            None => self.decode = false,
        }
        self.input = text.to_owned();
    }

    /// 当前输入的转换结果；方向、变体或输入任一变化时才重新计算。
    pub(crate) fn result(&mut self, spec: &Spec) -> Result<String, String> {
        let fresh = self.cache.as_ref().is_some_and(|c| {
            c.decode == self.decode && c.variant == self.variant && c.input == self.input
        });
        if !fresh {
            let convert = if self.decode { spec.decode } else { spec.encode };
            self.cache = Some(Cached {
                input: self.input.clone(),
                decode: self.decode,
                variant: self.variant,
                result: convert(&self.input, self.variant),
            });
        }
        match &self.cache {
            Some(cached) => cached.result.clone(),
            None => Ok(String::new()),
        }
    }
}

pub(crate) struct CodecTool {
    spec: &'static Spec,
    pub view: CodecView,
    palette: style::Palette,
    effects: Vec<ToolEffect>,
}
crate::support::tool_helpers!(CodecTool);

/// 供各工具描述符的 `create` 使用。
pub(crate) fn create(spec: &'static Spec) -> Box<dyn ToolModule> {
    Box::new(CodecTool::new(spec))
}

impl CodecTool {
    fn new(spec: &'static Spec) -> Self {
        Self {
            spec,
            view: CodecView::default(),
            palette: style::Palette::new(true),
            effects: Vec::new(),
        }
    }

    fn codec_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let spec = self.spec;
        let mut close = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        Frame::NONE
            .inner_margin(Margin::symmetric(style::PAD, 12))
            .show(ui, |ui| {
                drag_window(ui, ui.max_rect(), &mut self.effects);
                widgets::centered_row(ui, 28.0, |ui| {
                    #[cfg(target_os = "macos")]
                    {
                        close |= widgets::traffic_light_close(ui).clicked();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new(spec.title)
                            .size(16.0)
                            .strong()
                            .color(palette.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        #[cfg(not(target_os = "macos"))]
                        {
                            close |=
                                widgets::icon_button(ui, Icon::Close, false, &palette).clicked();
                        }
                        if widgets::icon_button(ui, Icon::Pin, self.view.pinned, &palette)
                            .on_hover_text("固定窗口，点击其他应用时保持显示")
                            .clicked()
                        {
                            self.view.pinned = !self.view.pinned;
                        }
                    });
                });
            });
        let view = &mut self.view;
        let mut copy: Option<String> = None;
        let mut swap = false;
        Frame::NONE
            .inner_margin(Margin::symmetric(24, 14))
            .show(ui, |ui| {
                let gap = 12.0;
                ui.spacing_mut().item_spacing.y = gap;
                ui.add(
                    egui::Label::new(RichText::new(spec.subtitle).size(12.0).color(palette.weak))
                        .wrap(),
                );
                // 方向切换放在最上方，与时间戳转换的双向标签保持一致。
                let (tabs, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
                let mut mode = usize::from(view.decode);
                if segmented(ui, tabs, &spec.modes, &[], &mut mode, 12.0, &palette) {
                    view.decode = mode == 1;
                }
                if !spec.variants.is_empty() {
                    widgets::centered_row(ui, 28.0, |ui| {
                        ui.label(
                            RichText::new(spec.variant_label)
                                .size(12.0)
                                .color(palette.secondary),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // 按最长的标签定宽，“URL 安全”“完整 URL”等不会被挤得换行。
                            let width = segment_width(ui, spec.variants, 11.5)
                                * spec.variants.len() as f32;
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
                            segmented(
                                ui,
                                rect,
                                spec.variants,
                                spec.variant_help,
                                &mut view.variant,
                                11.5,
                                &palette,
                            );
                        });
                    });
                }

                // 输入区：标题行带字符数与清空，编辑框在框内滚动。
                let mut clear = false;
                widgets::centered_row(ui, 20.0, |ui| {
                    ui.label(
                        RichText::new(if view.decode { "编码文本" } else { "原文" })
                            .size(11.5)
                            .color(palette.secondary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        clear = ui
                            .add_enabled(
                                !view.input.is_empty(),
                                egui::Button::new(
                                    RichText::new("清空").size(11.5).color(palette.accent),
                                )
                                .frame(false),
                            )
                            .clicked();
                        ui.label(
                            RichText::new(format!("{} 字符", view.input.chars().count()))
                                .size(11.0)
                                .color(palette.weak),
                        );
                    });
                });
                if clear {
                    view.input.clear();
                }
                // 剩余高度扣掉其下所有固定元素后，由输入框与结果框平分。
                let reserved = gap // 输入框之前的间距
                    + BOX_CHROME * 2.0
                    + gap
                    + OUTPUT_HEADER
                    + gap
                    + gap
                    + FOOTER;
                let box_height = ((ui.available_height() - reserved) / 2.0)
                    .floor()
                    .max(MIN_BOX);
                let id = ui.id().with("codec-input");
                let focus = ui.memory(|m| m.has_focus(id));
                let border = if focus { palette.accent } else { palette.border };
                let hint = if view.decode {
                    spec.decode_hint
                } else {
                    spec.encode_hint
                };
                text_box(ui, palette.field, border, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("codec-input-scroll")
                        .min_scrolled_height(box_height)
                        .max_height(box_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // 编辑框至少铺满整个框，点击空白处也能聚焦。
                            let rows = (box_height / 18.0).floor().max(1.0) as usize;
                            ui.add(
                                egui::TextEdit::multiline(&mut view.input)
                                    .id(id)
                                    .font(FontId::monospace(13.5))
                                    .frame(Frame::NONE)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(rows)
                                    .hint_text(hint),
                            );
                        });
                });

                // 结果区：输入后自动转换，出错时在原位置显示原因，不清空用户的输入。
                let result = view.result(spec);
                let has_output =
                    !view.input.is_empty() && result.as_ref().is_ok_and(|t| !t.is_empty());
                widgets::centered_row(ui, OUTPUT_HEADER, |ui| {
                    ui.label(
                        RichText::new(if view.decode { "解码结果" } else { "编码结果" })
                            .size(11.5)
                            .color(palette.secondary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                has_output,
                                egui::Button::new(
                                    RichText::new("复制").size(12.0).color(Color32::WHITE),
                                )
                                .fill(palette.accent)
                                .stroke(Stroke::NONE)
                                .corner_radius(7.0)
                                .min_size(Vec2::new(56.0, 28.0)),
                            )
                            .clicked()
                        {
                            copy = result.as_ref().ok().cloned();
                        }
                        if ui
                            .add_enabled(
                                has_output,
                                egui::Button::new(RichText::new("作为输入").size(12.0))
                                    .fill(palette.field)
                                    .stroke(Stroke::new(1.0, palette.border))
                                    .corner_radius(7.0)
                                    .min_size(Vec2::new(72.0, 28.0)),
                            )
                            .on_hover_ui(|ui| {
                                hint_card(
                                    ui,
                                    "作为输入",
                                    "把结果填回输入框并切换方向，便于反向校验",
                                    "",
                                    &palette,
                                )
                            })
                            .clicked()
                        {
                            swap = true;
                        }
                    });
                });
                let tint = if palette.dark {
                    Color32::from_rgb(32, 43, 61)
                } else {
                    Color32::from_rgb(240, 245, 255)
                };
                text_box(ui, tint, tint, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("codec-output-scroll")
                        .min_scrolled_height(box_height)
                        .max_height(box_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| match &result {
                            _ if view.input.is_empty() => {
                                ui.label(
                                    RichText::new("输入内容后自动转换")
                                        .size(12.0)
                                        .color(palette.weak),
                                );
                            }
                            Ok(text) => {
                                // 只读文本框：可以选中部分结果，但不会被误改。
                                let mut value = text.as_str();
                                ui.add(
                                    egui::TextEdit::multiline(&mut value)
                                        .font(FontId::monospace(13.5))
                                        .frame(Frame::NONE)
                                        .desired_width(f32::INFINITY)
                                        .text_color(palette.text),
                                );
                            }
                            Err(message) => {
                                ui.label(RichText::new(message).size(12.0).color(palette.danger));
                            }
                        });
                });
                ui.label(
                    RichText::new("本机转换，不保存内容；选中已编码的文本后按中键可直接解码")
                        .size(11.0)
                        .color(palette.weak),
                );
            });
        if let Some(text) = copy {
            self.effects.push(ToolEffect::Copy {
                text,
                sensitive: false,
            });
            self.show_toast("已复制");
        }
        if swap {
            if let Ok(text) = self.view.result(spec) {
                self.view.input = text;
                self.view.decode = !self.view.decode;
            }
        }
        if close {
            self.hide_panel(true);
        }
    }
}

/// 带底色与描边的文本容器，宽度撑满当前列。
fn text_box(ui: &mut Ui, fill: Color32, stroke: Color32, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(fill)
        .corner_radius(10.0)
        .stroke(Stroke::new(1.0, stroke))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// 单个分段的宽度：最长标签的单行宽度加左右留白，且不小于 64。
fn segment_width(ui: &Ui, labels: &[&str], size: f32) -> f32 {
    labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).to_owned(), FontId::proportional(size), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max)
        .max(36.0)
        + 28.0
}

/// 等宽分段控件；返回本帧是否切换了选项。`help` 可短于 `labels`，缺省项不显示悬停说明。
fn segmented(
    ui: &mut Ui,
    rect: Rect,
    labels: &[&str],
    help: &[VariantHelp],
    selected: &mut usize,
    size: f32,
    palette: &style::Palette,
) -> bool {
    ui.painter().rect_filled(rect, 7.0, palette.field);
    let width = rect.width() / labels.len().max(1) as f32;
    let mut changed = false;
    for (index, label) in labels.iter().enumerate() {
        let segment = Rect::from_min_size(
            rect.min + Vec2::new(index as f32 * width, 0.0),
            Vec2::new(width, rect.height()),
        )
        .shrink(3.0);
        let active = *selected == index;
        let button = egui::Button::new(RichText::new(*label).size(size).color(if active {
            palette.text
        } else {
            palette.weak
        }))
        // 标签永不折行；宽度已按最长标签计算，这里只是兜底。
        .wrap_mode(TextWrapMode::Extend)
        .fill(if active {
            palette.card
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(5.0)
        .stroke(Stroke::NONE);
        let mut response = ui.put(segment, button);
        if let Some(text) = help.get(index) {
            response = response
                .on_hover_ui(|ui| hint_card(ui, label, text.description, text.example, palette));
        }
        if response.clicked() && !active {
            *selected = index;
            changed = true;
        }
    }
    changed
}

/// 悬停说明卡片：标题、限宽自动换行的描述，以及可选的等宽示例块；
/// 样式与时间戳转换的「格式说明」一致，避免默认提示把长句排成一整行。
fn hint_card(ui: &mut Ui, title: &str, description: &str, example: &str, palette: &style::Palette) {
    Frame::NONE.inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_max_width(260.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new(title).size(12.0).strong().color(palette.text));
        ui.add(
            egui::Label::new(
                RichText::new(description)
                    .size(11.5)
                    .color(palette.secondary),
            )
            .wrap(),
        );
        if !example.is_empty() {
            Frame::new()
                .fill(palette.field)
                .corner_radius(5.0)
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(example)
                                .monospace()
                                .size(10.5)
                                .color(palette.text),
                        )
                        .wrap(),
                    );
                });
        }
    });
}

impl ToolModule for CodecTool {
    fn activate(&mut self, input: Option<Selection>) -> Vec<ToolEffect> {
        if let Some(Selection::Text(text)) = input {
            self.view.load(self.spec, &text);
        }
        std::mem::take(&mut self.effects)
    }
    fn render(&mut self, ui: &mut Ui, palette: &style::Palette) -> Vec<ToolEffect> {
        self.palette = *palette;
        self.codec_ui(ui);
        std::mem::take(&mut self.effects)
    }
    fn pinned(&self) -> bool {
        self.view.pinned
    }
}

/// 三个编码工具共用的交互与离屏渲染流程：带入原文 → 复制编码结果 → 变体悬停说明 →
/// 结果作为输入反向解码 → 非法输入报错 → 窄窗口与拉高窗口 → esc 关闭。截图输出到 `target/ui-snapshots/`。
#[cfg(test)]
pub(crate) fn snapshot_case(
    spec: &'static Spec,
    name: &str,
    text: &str,
    encoded: &str,
    invalid: &str,
) {
    use egui_kittest::kittest::Queryable;
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut tool = CodecTool::new(spec);
        tool.activate(Some(Selection::Text(text.into())));
        let mut harness = crate::test_support::harness(tool, theme, egui::vec2(680.0, 560.0));
        assert!(!harness.state().tool.view.decode);
        crate::test_support::save(&mut harness, &format!("{name}-{suffix}"));
        harness.get_by_label("复制").click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().copied.as_ref(),
            Some(&(encoded.to_owned(), false))
        );
        assert_eq!(harness.state().toast.as_deref(), Some("已复制"));
        if let Some(last) = spec.variants.last() {
            harness.get_by_label(last).hover();
            harness.run_steps(3);
            crate::test_support::save(&mut harness, &format!("{name}-hint-{suffix}"));
        }
        harness.get_by_label("作为输入").click();
        harness.run_steps(2);
        assert!(harness.state().tool.view.decode);
        assert_eq!(harness.state().tool.view.input, encoded);
        crate::test_support::save(&mut harness, &format!("{name}-decode-{suffix}"));
        harness.state_mut().tool.view.input = invalid.into();
        harness.run_steps(2);
        assert!(harness.state_mut().tool.view.result(spec).is_err());
        crate::test_support::save(&mut harness, &format!("{name}-error-{suffix}"));
        harness.set_size(egui::vec2(380.0, 640.0));
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("{name}-narrow-{suffix}"));
        harness.set_size(egui::vec2(680.0, 800.0));
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("{name}-tall-{suffix}"));
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().closed);
    }
}

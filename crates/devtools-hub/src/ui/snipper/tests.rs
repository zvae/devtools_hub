//! 截屏遮罩的交互测试：不创建真实窗口，直接在 egui_kittest 里绘制，用模拟的鼠标、键盘事件驱动。

use super::*;
use crate::snip::annotation::Size;
use crate::snip::{Display, ScreenWindow};
use egui_kittest::Harness;
use image::Rgba;

/// 截屏遮罩与它最后一次给出的结果。
struct Test {
    session: SnipSession,
    outcome: Option<Outcome>,
}

/// 400×300 的“屏幕”（缩放 1），(20, 20)–(80, 60) 有一个窗口，(30, 30) 是一个红色像素。
fn harness() -> Harness<'static, Test> {
    let mut image = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
    image.put_pixel(30, 30, Rgba([255, 0, 0, 255]));
    let display = Display {
        id: 0,
        frame: [0.0, 0.0, 400.0, 300.0],
        logical: Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)),
        scale: 1.0,
    };
    let windows = [ScreenWindow {
        frame: [20.0, 20.0, 60.0, 40.0],
        floating: false,
    }];
    let capture = Capture::new(display, image, &windows);
    Harness::builder()
        .with_size(vec2(400.0, 300.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, test: &mut Test| {
                let outcome = test.session.ui(ui);
                if !matches!(outcome, Outcome::Continue) {
                    test.outcome = Some(outcome);
                }
            },
            Test {
                session: SnipSession::new(capture, [300.0, 250.0]),
                outcome: None,
            },
        )
}

fn button(harness: &mut Harness<'_, Test>, button: PointerButton, pos: Pos2, pressed: bool) {
    harness.event(Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    });
    harness.step();
}

/// 移动到 `from` 按下，拖到 `to` 松开。
fn drag(harness: &mut Harness<'_, Test>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.step();
    button(harness, PointerButton::Primary, from, true);
    harness.hover_at(to);
    harness.step();
    button(harness, PointerButton::Primary, to, false);
}

fn click(harness: &mut Harness<'_, Test>, pos: Pos2) {
    harness.hover_at(pos);
    harness.step();
    button(harness, PointerButton::Primary, pos, true);
    button(harness, PointerButton::Primary, pos, false);
}

fn selection(harness: &Harness<'_, Test>) -> Option<Rect> {
    harness.state().session.selection
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

#[test]
fn click_selects_window_under_cursor_and_enter_copies_it() {
    let mut harness = harness();
    click(&mut harness, pos2(30.0, 30.0));
    assert_eq!(selection(&harness), Some(rect(20.0, 20.0, 80.0, 60.0)));
    harness.key_press(Key::Enter);
    harness.step();
    let Some(Outcome::Copy(image)) = harness.state_mut().outcome.take() else {
        panic!("应该复制选区");
    };
    assert_eq!(image.dimensions(), (60, 40));
    assert_eq!(image.get_pixel(10, 10).0, [255, 0, 0, 255]);
}

#[test]
fn click_on_desktop_selects_whole_screen() {
    let mut harness = harness();
    click(&mut harness, pos2(300.0, 200.0));
    assert_eq!(selection(&harness), Some(rect(0.0, 0.0, 400.0, 300.0)));
}

#[test]
fn drag_creates_resizes_and_moves_selection_then_pins() {
    let mut harness = harness();
    drag(&mut harness, pos2(100.0, 50.0), pos2(150.0, 90.0));
    assert_eq!(selection(&harness), Some(rect(100.0, 50.0, 150.0, 90.0)));
    // 拖右下角放大。
    drag(&mut harness, pos2(150.0, 90.0), pos2(170.0, 110.0));
    assert_eq!(selection(&harness), Some(rect(100.0, 50.0, 170.0, 110.0)));
    // 在选区内拖动：移动。
    drag(&mut harness, pos2(130.0, 80.0), pos2(120.0, 70.0));
    assert_eq!(selection(&harness), Some(rect(90.0, 40.0, 160.0, 100.0)));
    // 方向键移动 1 像素，⇧+方向键调整宽度。
    harness.key_press(Key::ArrowRight);
    harness.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
    harness.step();
    assert_eq!(selection(&harness), Some(rect(91.0, 40.0, 160.0, 100.0)));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::T);
    harness.step();
    let Some(Outcome::Pin { image, rect: at }) = harness.state_mut().outcome.take() else {
        panic!("应该贴图");
    };
    assert_eq!(image.dimensions(), (69, 60));
    assert_eq!(at, rect(91.0, 40.0, 160.0, 100.0));
}

#[test]
fn double_click_inside_selection_copies() {
    let mut harness = harness();
    drag(&mut harness, pos2(100.0, 50.0), pos2(150.0, 90.0));
    click(&mut harness, pos2(120.0, 70.0));
    click(&mut harness, pos2(120.0, 70.0));
    assert!(matches!(
        harness.state_mut().outcome.take(),
        Some(Outcome::Copy(image)) if image.dimensions() == (50, 40)
    ));
}

#[test]
fn right_click_resets_selection_then_cancels() {
    let mut harness = harness();
    drag(&mut harness, pos2(100.0, 50.0), pos2(150.0, 90.0));
    button(
        &mut harness,
        PointerButton::Secondary,
        pos2(120.0, 70.0),
        true,
    );
    button(
        &mut harness,
        PointerButton::Secondary,
        pos2(120.0, 70.0),
        false,
    );
    assert_eq!(selection(&harness), None);
    assert!(harness.state().outcome.is_none());
    button(
        &mut harness,
        PointerButton::Secondary,
        pos2(120.0, 70.0),
        true,
    );
    button(
        &mut harness,
        PointerButton::Secondary,
        pos2(120.0, 70.0),
        false,
    );
    assert!(matches!(harness.state().outcome, Some(Outcome::Cancel)));
}

#[test]
fn escape_cancels_and_c_copies_color() {
    let mut harness = harness();
    harness.hover_at(pos2(30.5, 30.5));
    harness.step();
    harness.key_press(Key::C);
    harness.step();
    assert!(matches!(
        harness.state_mut().outcome.take(),
        Some(Outcome::CopyColor(hex)) if hex == "#FF0000"
    ));
    harness.key_press(Key::Escape);
    harness.step();
    assert!(matches!(harness.state().outcome, Some(Outcome::Cancel)));
}

#[test]
fn c_copies_color_inside_selection() {
    let mut harness = harness();
    click(&mut harness, pos2(30.0, 30.0));
    assert_eq!(selection(&harness), Some(rect(20.0, 20.0, 80.0, 60.0)));
    harness.hover_at(pos2(30.5, 30.5));
    harness.step();
    assert!(harness
        .state()
        .session
        .shows_magnifier(Some(pos2(30.5, 30.5)), false));
    harness.key_press(Key::C);
    harness.step();
    assert!(matches!(
        harness.state_mut().outcome.take(),
        Some(Outcome::CopyColor(hex)) if hex == "#FF0000"
    ));
    // 选区外不显示放大镜，C 不复制颜色。
    assert!(!harness
        .state()
        .session
        .shows_magnifier(Some(pos2(200.0, 200.0)), false));
    harness.hover_at(pos2(200.0, 200.0));
    harness.step();
    harness.key_press(Key::C);
    harness.step();
    assert!(harness.state().outcome.is_none());
}

fn shift(harness: &mut Harness<'_, Test>, down: bool) {
    let modifiers = if down {
        Modifiers::SHIFT
    } else {
        Modifiers::NONE
    };
    harness.event(Event::ModifiersChanged(modifiers));
    harness.step();
}

#[test]
fn shift_toggles_rgb_and_hex() {
    let mut harness = harness();
    harness.hover_at(pos2(30.5, 30.5));
    harness.step();
    shift(&mut harness, true);
    shift(&mut harness, false);
    harness.key_press(Key::C);
    harness.step();
    assert!(matches!(
        harness.state_mut().outcome.take(),
        Some(Outcome::CopyColor(color)) if color == "rgb(255, 0, 0)"
    ));
    // ⇧ 和别的键一起按（⇧+方向键）不切换。
    shift(&mut harness, true);
    harness.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
    harness.step();
    shift(&mut harness, false);
    assert!(harness.state().session.rgb);
    shift(&mut harness, true);
    shift(&mut harness, false);
    assert!(!harness.state().session.rgb);
}

/// 选好 (100, 50)–(300, 200) 的区域。
fn select_area(harness: &mut Harness<'_, Test>) {
    drag(harness, pos2(100.0, 50.0), pos2(300.0, 200.0));
    assert_eq!(selection(harness), Some(rect(100.0, 50.0, 300.0, 200.0)));
}

fn marks(harness: &Harness<'_, Test>) -> Vec<Mark> {
    harness.state().session.marks.items().to_vec()
}

#[test]
fn toolbar_button_selects_tool() {
    let mut harness = harness();
    select_area(&mut harness);
    // 第一个按钮是矩形 / 椭圆。
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
    let bars = toolbar::layout(rect(100.0, 50.0, 300.0, 200.0), screen, None, Mode::Snip);
    let first = bars.main.left_top() + vec2(4.0 + 15.0, 4.0 + 15.0);
    click(&mut harness, first);
    assert_eq!(harness.state().session.tool, Some(Tool::Shape));
    // 点在工具栏上不会改变选区。
    assert_eq!(selection(&harness), Some(rect(100.0, 50.0, 300.0, 200.0)));
    // 再点一次取消选择。
    click(&mut harness, first);
    assert_eq!(harness.state().session.tool, None);
}

#[test]
fn pen_draws_inside_selection_with_undo_redo() {
    let mut harness = harness();
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Pen);
    drag(&mut harness, pos2(150.0, 100.0), pos2(200.0, 150.0));
    let marks_now = marks(&harness);
    let [Mark::Stroke { points, width, .. }] = marks_now.as_slice() else {
        panic!("应该有一笔画笔");
    };
    assert_eq!(points.first(), Some(&pos2(150.0, 100.0)));
    assert_eq!(points.last(), Some(&pos2(200.0, 150.0)));
    assert_eq!(*width, Size::Medium.points(Tool::Pen));
    // 画标注不会移动选区。
    assert_eq!(selection(&harness), Some(rect(100.0, 50.0, 300.0, 200.0)));

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.step();
    assert!(marks(&harness).is_empty());
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.step();
    assert_eq!(marks(&harness).len(), 1);
}

#[test]
fn shape_is_clamped_to_selection_and_eraser_removes_it() {
    let mut harness = harness();
    select_area(&mut harness);
    {
        let session = &mut harness.state_mut().session;
        session.tool = Some(Tool::Shape);
        session.style.ellipse = true;
        session.style.size = Size::Large;
    }
    drag(&mut harness, pos2(150.0, 100.0), pos2(390.0, 290.0));
    let marks_now = marks(&harness);
    let [Mark::Shape {
        rect: shape,
        ellipse,
        ..
    }] = marks_now.as_slice()
    else {
        panic!("应该有一个椭圆");
    };
    assert!(*ellipse);
    assert_eq!(*shape, rect(150.0, 100.0, 300.0, 200.0));

    harness.state_mut().session.tool = Some(Tool::Eraser);
    // 椭圆最左边的点 (150, 150)。
    click(&mut harness, pos2(151.0, 150.0));
    assert!(marks(&harness).is_empty());
}

#[test]
fn text_tool_types_and_commits_on_escape() {
    let mut harness = harness();
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Text);
    click(&mut harness, pos2(150.0, 100.0));
    assert!(harness.state().session.text.is_some());
    harness.step();
    harness.event(Event::Text("你好".into()));
    harness.step();
    // 输入框向右延伸：一行放得下的文字不会换行。
    harness.event(Event::Text(" hello world".into()));
    harness.step();
    harness.step();
    {
        let draft = harness.state().session.text.as_ref().unwrap();
        let rect = draft.rect.unwrap();
        let line = draft.size; // 缩放为 1，字号的像素就是点
        assert!(
            rect.height() < line * 1.8,
            "输入框高度 {} 说明换行了",
            rect.height()
        );
        assert!(
            rect.width() > line * 4.0,
            "输入框宽度 {} 没有随文字变宽",
            rect.width()
        );
    }
    // 删掉后面加的英文，后面的断言只看“你好”。
    for _ in 0.." hello world".len() {
        harness.key_press(Key::Backspace);
    }
    harness.step();
    // 输入文字时 esc 结束输入，而不是退出截屏。
    harness.key_press(Key::Escape);
    harness.step();
    assert!(harness.state().outcome.is_none());
    assert!(harness.state().session.text.is_none());
    let marks_now = marks(&harness);
    let [Mark::Text {
        pos, text, extent, ..
    }] = marks_now.as_slice()
    else {
        panic!("应该有一段文字");
    };
    assert_eq!(*pos, pos2(150.0, 100.0));
    assert_eq!(text, "你好");
    assert!(extent.x > 0.0 && extent.y > 0.0);
}

#[test]
fn long_text_wraps_at_selection_edge() {
    let mut harness = harness();
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Text);
    click(&mut harness, pos2(150.0, 100.0));
    harness.step();
    harness.event(Event::Text("hello world hello world hello world".into()));
    harness.step();
    harness.step();
    let line = {
        let draft = harness.state().session.text.as_ref().unwrap();
        let rect = draft.rect.unwrap();
        // 选区右边缘是 300：输入框不越过它（虚线框外扩 3），而是换行向下延伸。
        assert!(
            rect.right() <= 300.0 + 3.0 + 0.5,
            "输入框越过了选区：{rect:?}"
        );
        assert!(rect.height() > draft.size * 1.8, "长文字没有换行：{rect:?}");
        draft.size
    };
    harness.key_press(Key::Escape);
    harness.step();
    let marks_now = marks(&harness);
    let [Mark::Text { wrap, extent, .. }] = marks_now.as_slice() else {
        panic!("应该有一段文字");
    };
    // 完成后的文字按同样的宽度换行。
    assert!((*wrap - 148.0).abs() < 0.5, "换行宽度 {wrap}");
    assert!(extent.x <= *wrap + 0.5 && extent.y > line * 1.8);
}

/// 编辑 (100, 50)–(300, 200) 处的一张贴图，贴图原图是 400×300 的纯蓝色（比显示的大小精细一倍）。
fn edit_pin(harness: &mut Harness<'_, Test>) {
    let image = RgbaImage::from_pixel(400, 300, Rgba([0, 0, 255, 255]));
    harness
        .state_mut()
        .session
        .edit(Arc::new(image), rect(100.0, 50.0, 300.0, 200.0));
    harness.step();
}

#[test]
fn editing_pin_keeps_selection_fixed() {
    let mut harness = harness();
    edit_pin(&mut harness);
    let fixed = Some(rect(100.0, 50.0, 300.0, 200.0));
    assert_eq!(selection(&harness), fixed);
    // 拖控制点、在选区内拖动、在选区外拖动都不改变选区。
    drag(&mut harness, pos2(300.0, 200.0), pos2(350.0, 250.0));
    drag(&mut harness, pos2(200.0, 120.0), pos2(220.0, 140.0));
    drag(&mut harness, pos2(20.0, 20.0), pos2(60.0, 40.0));
    assert_eq!(selection(&harness), fixed);
    // 可以画标注。
    harness.state_mut().session.tool = Some(Tool::Pen);
    drag(&mut harness, pos2(150.0, 100.0), pos2(200.0, 150.0));
    assert_eq!(marks(&harness).len(), 1);
}

#[test]
fn editing_pin_without_marks_outputs_original_image() {
    let mut harness = harness();
    edit_pin(&mut harness);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::T);
    harness.step();
    let Some(Outcome::Pin { image, .. }) = harness.state_mut().outcome.take() else {
        panic!("应该完成编辑");
    };
    // 原图的分辨率，而不是截图里显示的 200×150。
    assert_eq!(image.dimensions(), (400, 300));
    assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
}

#[test]
fn right_click_in_edit_mode_cancels_instead_of_dropping_selection() {
    let mut harness = harness();
    edit_pin(&mut harness);
    for pressed in [true, false] {
        button(
            &mut harness,
            PointerButton::Secondary,
            pos2(150.0, 100.0),
            pressed,
        );
    }
    assert!(matches!(harness.state().outcome, Some(Outcome::Cancel)));
}

#[test]
fn right_click_steps_back_tool_then_selection() {
    let mut harness = harness();
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Pen);
    drag(&mut harness, pos2(150.0, 100.0), pos2(200.0, 150.0));
    for pressed in [true, false] {
        button(
            &mut harness,
            PointerButton::Secondary,
            pos2(150.0, 100.0),
            pressed,
        );
    }
    assert_eq!(harness.state().session.tool, None);
    assert_eq!(marks(&harness).len(), 1);
    for pressed in [true, false] {
        button(
            &mut harness,
            PointerButton::Secondary,
            pos2(150.0, 100.0),
            pressed,
        );
    }
    assert_eq!(selection(&harness), None);
    assert!(marks(&harness).is_empty());
}

#[test]
fn output_with_marks_exports_rendered_frame() {
    let mut harness = harness();
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Pen);
    drag(&mut harness, pos2(150.0, 100.0), pos2(200.0, 150.0));
    harness.key_press(Key::Enter);
    harness.step();
    // 有标注时先导出：等截下的画面。
    assert!(harness.state().outcome.is_none());
    assert!(harness.state().session.export.is_some());
    // 截下的画面送回来（这里用纯绿色代替），裁出选区。
    let frame = ColorImage::filled([400, 300], Color32::from_rgb(0, 200, 0));
    let session = &mut harness.state_mut().session;
    let Outcome::Copy(image) = session.receive_screenshot(&frame) else {
        panic!("应该复制导出的图片");
    };
    assert_eq!(image.dimensions(), (200, 150));
    assert_eq!(image.get_pixel(0, 0).0, [0, 200, 0, 255]);
    assert!(session.export.is_none());
}

/// 模拟滚动的画面：一张很长的“网页”，每截一次向下滚动 40 像素（选区 200×150）。
fn scrolling_frames(_: &Capture, rect: Rect, _: Option<u32>) -> scroll::FrameSource {
    let (width, height) = (rect.width() as u32, rect.height() as u32);
    let page = RgbaImage::from_fn(width, 2000, |x, y| {
        let line = y % 24;
        let length = 30 + (y / 24 * 37 % 150);
        if (4..16).contains(&line) && x > 8 && x < 8 + length && (x + y / 24) % 7 != 0 {
            Rgba([(30 + y % 50) as u8, 40, (60 + x % 40) as u8, 255])
        } else {
            Rgba([250, 250, 250, 255])
        }
    });
    let mut calls = 0u32;
    Box::new(move || {
        let top = (calls * 40).min(2000 - height);
        calls += 1;
        Ok(image::imageops::crop_imm(&page, 0, top, width, height).to_image())
    })
}

/// 主工具栏上“长截图”按钮的中心（选区 (100, 50)–(300, 200)）。
fn scroll_button() -> Pos2 {
    let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
    let bars = toolbar::layout(rect(100.0, 50.0, 300.0, 200.0), screen, None, Mode::Snip);
    // 7 个工具、分隔线、撤销、重做、分隔线之后是长截图。
    bars.main.left_top() + vec2(4.0 + 7.0 * 30.0 + 9.0 + 2.0 * 30.0 + 9.0 + 15.0, 4.0 + 15.0)
}

/// 等后台线程把长图拼到至少 `height` 像素高（最多等 5 秒）。
fn wait_for_height(harness: &mut Harness<'_, Test>, height: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        harness.step();
        let (_, size) = harness.state().session.scroll.as_ref().unwrap().status();
        if size[1] >= height {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "长图一直没有拼到 {height} 像素（现在 {size:?}）"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn scroll_capture_stitches_frames_and_copies_long_image() {
    let mut harness = harness();
    harness.state_mut().session.frame_source = scrolling_frames;
    select_area(&mut harness);
    assert!(harness.state().session.can_scroll());
    click(&mut harness, scroll_button());
    assert!(harness.state().session.scroll.is_some(), "应该进入长截图");
    wait_for_height(&mut harness, 400);
    harness.key_press(Key::Enter);
    harness.step();
    let Some(Outcome::Copy(image)) = harness.state_mut().outcome.take() else {
        panic!("↩ 应该复制长图");
    };
    assert_eq!(image.width(), 200);
    assert!(image.height() >= 400, "长图高度 {}", image.height());
}

#[test]
fn scroll_capture_escape_cancels() {
    let mut harness = harness();
    harness.state_mut().session.frame_source = scrolling_frames;
    select_area(&mut harness);
    click(&mut harness, scroll_button());
    assert!(harness.state().session.scroll.is_some());
    harness.key_press(Key::Escape);
    harness.step();
    assert!(matches!(harness.state().outcome, Some(Outcome::Cancel)));
}

#[test]
fn scroll_capture_needs_no_marks_and_a_large_enough_selection() {
    let mut harness = harness();
    harness.state_mut().session.frame_source = scrolling_frames;
    select_area(&mut harness);
    harness.state_mut().session.tool = Some(Tool::Pen);
    drag(&mut harness, pos2(150.0, 100.0), pos2(200.0, 150.0));
    harness.state_mut().session.tool = None;
    assert!(!harness.state().session.can_scroll());
    // 有标注时按钮不可用，点了也不进入长截图。
    click(&mut harness, scroll_button());
    assert!(harness.state().session.scroll.is_none());
    // 选区太小也不行。
    let session = &mut harness.state_mut().session;
    session.marks = Marks::default();
    session.selection = Some(rect(10.0, 10.0, 60.0, 200.0));
    assert!(!session.can_scroll());
}

/// 离屏渲染截屏遮罩（选区、各种标注、工具栏与属性栏），输出 PNG 供人工检查。
/// 需要 GPU，默认忽略：`cargo test -p devtools-hub snapshot -- --ignored`，输出到 `target/ui-snapshots/`。
#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_snipper() {
    // 800×500 点、2 倍缩放的“屏幕”：渐变的桌面上有一个白色窗口，窗口里有几行“文字”。
    let (width, height) = (1600u32, 1000u32);
    let mut image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (40 + x * 60 / width) as u8,
            (70 + y * 80 / height) as u8,
            150,
            255,
        ])
    });
    for y in 160..760 {
        for x in 300..1300 {
            let line = y > 220 && (y - 220) % 60 < 18 && x < 300 + 200 + (y % 7) * 90;
            let pixel = if line {
                [60, 60, 66, 255]
            } else {
                [252, 252, 252, 255]
            };
            image.put_pixel(x, y, Rgba(pixel));
        }
    }
    let display = Display {
        id: 0,
        frame: [0.0, 0.0, 800.0, 500.0],
        logical: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0)),
        scale: 2.0,
    };
    let windows = [ScreenWindow {
        frame: [150.0, 80.0, 500.0, 300.0],
        floating: false,
    }];
    let capture = Capture::new(display, image, &windows);
    let mut session = SnipSession::new(capture, [400.0, 250.0]);
    // 选区与标注（像素坐标，是点的 2 倍）。
    session.selection = Some(rect(260.0, 140.0, 1200.0, 700.0));
    let red = crate::snip::annotation::COLORS[0];
    session.marks.push(Mark::Shape {
        rect: rect(320.0, 200.0, 620.0, 360.0),
        ellipse: false,
        color: red,
        width: 8.0,
    });
    session.marks.push(Mark::Shape {
        rect: rect(700.0, 200.0, 1000.0, 360.0),
        ellipse: true,
        color: crate::snip::annotation::COLORS[4],
        width: 8.0,
    });
    session.marks.push(Mark::Arrow {
        from: pos2(1100.0, 600.0),
        to: pos2(880.0, 420.0),
        color: red,
        width: 8.0,
    });
    session.marks.push(Mark::Stroke {
        points: (0..40)
            .map(|i| {
                pos2(
                    330.0 + i as f32 * 8.0,
                    520.0 + (i as f32 * 0.4).sin() * 30.0,
                )
            })
            .collect(),
        color: crate::snip::annotation::COLORS[3],
        width: 8.0,
    });
    session.marks.push(Mark::Stroke {
        points: vec![pos2(330.0, 420.0), pos2(640.0, 420.0)],
        color: Style {
            color: crate::snip::annotation::COLORS[2],
            ..Style::default()
        }
        .marker_color(),
        width: 36.0,
    });
    let mut mosaic = Mosaic::new(&session.capture.image, pos2(700.0, 620.0), 60.0, 16.0);
    mosaic.extend(&session.capture.image, pos2(1000.0, 640.0));
    session.marks.push(Mark::Mosaic(mosaic));
    session.marks.push(Mark::Text {
        pos: pos2(720.0, 480.0),
        text: "截图标注".into(),
        color: red,
        size: 40.0,
        wrap: 480.0,
        extent: vec2(160.0, 48.0),
    });
    session.tool = Some(Tool::Shape);
    session.style.ellipse = true;

    let mut harness = Harness::builder()
        .with_size(vec2(800.0, 500.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, session: &mut SnipSession| {
                let _ = session.ui(ui);
            },
            session,
        );
    // 界面字体（含中文），与应用启动时一样安装。
    crate::ui::style::install(&harness.ctx);
    harness.run_steps(3);
    let rendered = harness.render().expect("离屏渲染失败");
    let dir = std::env::var_os("UI_SNAPSHOT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/ui-snapshots"
            ))
        });
    std::fs::create_dir_all(&dir).unwrap();
    rendered.save(dir.join("snipper.png")).unwrap();
}

/// 离屏渲染长截图中的遮罩（选区外的遮罩、边框、提示、工具栏与预览），输出 PNG 供人工检查。
/// 需要 GPU，默认忽略：`cargo test -p devtools-hub snapshot -- --ignored`，输出到 `target/ui-snapshots/`。
#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_scroll_capture() {
    let display = Display {
        id: 0,
        frame: [0.0, 0.0, 800.0, 500.0],
        logical: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0)),
        scale: 2.0,
    };
    let image = RgbaImage::from_pixel(1600, 1000, Rgba([90, 120, 160, 255]));
    let capture = Capture::new(display, image, &[]);
    let mut session = SnipSession::new(capture, [400.0, 250.0]);
    session.selection = Some(rect(300.0, 160.0, 1100.0, 760.0));
    let mut harness = Harness::builder()
        .with_size(vec2(800.0, 500.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, session: &mut SnipSession| {
                let _ = session.ui(ui);
            },
            session,
        );
    crate::ui::style::install(&harness.ctx);
    let source = scrolling_frames(
        &harness.state().capture,
        rect(300.0, 160.0, 1100.0, 760.0),
        None,
    );
    let ctx = harness.ctx.clone();
    harness.state_mut().scroll = Some(ScrollCapture::start(source, ctx, ViewportId::ROOT));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while harness.state().scroll.as_ref().unwrap().status().1[1] < 1200 {
        assert!(std::time::Instant::now() < deadline, "长图一直没有拼好");
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    harness.run_steps(3);
    let rendered = harness.render().expect("离屏渲染失败");
    let dir = std::env::var_os("UI_SNAPSHOT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/ui-snapshots"
            ))
        });
    std::fs::create_dir_all(&dir).unwrap();
    rendered.save(dir.join("scroll_capture.png")).unwrap();
}

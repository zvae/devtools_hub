use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use devtools_ui::{JsonLineView, ToolWindow};
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        Clipboard, Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
    },
    ComponentHandle, ModelRc, PhysicalSize, Rgb8Pixel, SharedPixelBuffer, VecModel,
};

struct TestPlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
    clipboard: Rc<RefCell<String>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.window.borrow_mut() = Some(window.clone());
        Ok(window)
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if clipboard == Clipboard::DefaultClipboard {
            *self.clipboard.borrow_mut() = text.to_owned();
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        (clipboard == Clipboard::DefaultClipboard).then(|| self.clipboard.borrow().clone())
    }
}

fn render(window: &MinimalSoftwareWindow, name: &str, width: u32, height: u32) -> Vec<u8> {
    window.set_size(PhysicalSize::new(width, height));
    window.request_redraw();
    slint::platform::update_timers_and_animations();
    let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(pixels.make_mut_slice(), width as usize);
    }));
    let bytes = pixels.as_bytes();
    let background = &bytes[..3];
    assert!(
        bytes.chunks_exact(3).filter(|p| *p != background).count() > (width * height / 10) as usize
    );
    if let Some(directory) = std::env::var_os("DEVTOOLS_UI_SNAPSHOTS") {
        std::fs::create_dir_all(&directory).unwrap();
        let path = std::path::Path::new(&directory).join(format!("{name}.png"));
        let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(bytes)
            .unwrap();
    }
    bytes.to_vec()
}

fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn shortcut(window: &MinimalSoftwareWindow, key: &str) {
    // Slint's platform events use Control for the primary command modifier on every OS.
    window.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    window.dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    window.dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    window.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
}

fn drag(window: &MinimalSoftwareWindow, from: (f32, f32), to: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerPressed {
        position: slint::LogicalPosition::new(from.0, from.1),
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(to.0, to.1),
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: slint::LogicalPosition::new(to.0, to.1),
        button: PointerEventButton::Left,
    });
}

#[test]
fn tool_window_collapse_and_json_controls_render_at_supported_sizes() {
    let adapter = Rc::new(RefCell::new(None));
    let clipboard = Rc::new(RefCell::new(String::new()));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: adapter.clone(),
        clipboard: clipboard.clone(),
    }))
    .unwrap();
    let tool = ToolWindow::new().unwrap();
    tool.set_title_text("JSON Format".into());
    tool.set_language("en".into());
    tool.set_dark_mode(true);
    let input = r#"{"users":[{"name":"Ada","active":true}],"count":1}"#;
    tool.set_input(input.into());
    tool.set_output(input.into());
    tool.set_json_output(true);
    let lines = [
        "{",
        "  \"users\": [",
        "    {",
        "      \"name\": \"Ada\",",
        "      \"active\": true",
        "    }",
        "  ],",
        "  \"count\": 1",
        "}",
    ];
    tool.set_json_display_text(lines.join("\n").into());
    tool.set_json_lines(ModelRc::new(VecModel::from(
        lines
            .iter()
            .enumerate()
            .map(|(i, text)| JsonLineView {
                line_id: i as i32,
                number: (i + 1).to_string().into(),
                text: (*text).into(),
                expandable: i < 3,
                collapsed: false,
            })
            .collect::<Vec<_>>(),
    )));
    tool.show().unwrap();
    let adapter = adapter.borrow().as_ref().unwrap().clone();
    let expanded = render(&adapter, "json-expanded", 860, 560);
    click(&adapter, 29.0, 29.0);
    assert!(tool.get_input_collapsed(), "input toggle must be clickable");
    let collapsed = render(&adapter, "json-input-collapsed", 860, 560);
    assert_ne!(expanded, collapsed);
    assert_eq!(tool.get_input().as_str(), input);
    assert_eq!(tool.get_output().as_str(), input);
    drag(&adapter, (108.0, 128.0), (108.0, 194.0));
    shortcut(&adapter, "c");
    assert_eq!(
        *clipboard.borrow(),
        format!("{}\n", lines[..4].join("\n")),
        "dragging across lines must copy a single selection without line numbers",
    );
    render(&adapter, "json-multiline-selection", 860, 560);
    shortcut(&adapter, "a");
    shortcut(&adapter, "c");
    assert_eq!(*clipboard.borrow(), lines.join("\n"));
    let folded_line = Rc::new(Cell::new(-1));
    let folded_line_callback = folded_line.clone();
    tool.on_toggle_json_fold(move |line| folded_line_callback.set(line));
    click(&adapter, 92.0, 136.0);
    assert_eq!(
        folded_line.get(),
        0,
        "fold gutter must activate the original line id"
    );
    let all_collapsed = Rc::new(Cell::new(None));
    let all_collapsed_callback = all_collapsed.clone();
    tool.on_fold_all_json(move |collapsed| all_collapsed_callback.set(Some(collapsed)));
    click(&adapter, 783.0, 99.0);
    assert_eq!(all_collapsed.get(), Some(true));
    click(&adapter, 819.0, 99.0);
    assert_eq!(all_collapsed.get(), Some(false));
    render(&adapter, "json-minimum-collapsed", 620, 420);
    click(&adapter, 29.0, 29.0);
    assert!(!tool.get_input_collapsed());
    assert_eq!(tool.get_input().as_str(), input);
    render(&adapter, "json-minimum-expanded", 620, 420);
    tool.set_dark_mode(false);
    tool.set_language("zh-CN".into());
    render(&adapter, "json-light", 860, 560);
    tool.set_json_lines(ModelRc::new(VecModel::from(vec![JsonLineView {
        line_id: 0,
        number: "1".into(),
        text: "{ ... }".into(),
        expandable: true,
        collapsed: true,
    }])));
    tool.set_json_display_text("{ ... }".into());
    render(&adapter, "json-folded", 860, 560);
    tool.set_input_collapsed(true);
    render(&adapter, "json-folded-selection", 620, 420);
    click(&adapter, 120.0, 128.0);
    shortcut(&adapter, "a");
    shortcut(&adapter, "c");
    assert_eq!(*clipboard.borrow(), "{ ... }");

    let long_lines = (0..120)
        .map(|index| format!("  \"item_{index:03}\": \"\u{4e2d}\u{6587}\","))
        .collect::<Vec<_>>();
    let long_text = long_lines.join("\n");
    tool.set_json_lines(ModelRc::new(VecModel::from(
        long_lines
            .iter()
            .enumerate()
            .map(|(index, text)| JsonLineView {
                line_id: index as i32,
                number: (index + 1).to_string().into(),
                text: text.clone().into(),
                expandable: false,
                collapsed: false,
            })
            .collect::<Vec<_>>(),
    )));
    tool.set_json_display_text(long_text.clone().into());
    render(&adapter, "json-long", 620, 420);
    adapter.dispatch_event(WindowEvent::PointerScrolled {
        position: slint::LogicalPosition::new(230.0, 200.0),
        delta_x: 0.0,
        delta_y: -400.0,
    });
    render(&adapter, "json-long-scrolled", 620, 420);
    drag(&adapter, (108.0, 128.0), (108.0, 194.0));
    shortcut(&adapter, "c");
    assert!(clipboard.borrow().lines().count() >= 4);
    assert!(long_text.contains(clipboard.borrow().as_str()));
    shortcut(&adapter, "a");
    shortcut(&adapter, "c");
    assert_eq!(
        *clipboard.borrow(),
        long_text,
        "copy must include offscreen Unicode text"
    );
    // The editable input must stay unchanged when selecting or copying output.
    assert_eq!(tool.get_input().as_str(), input);
    assert_eq!(tool.get_output().as_str(), input);
    tool.set_json_output(false);
    render(&adapter, "plain-output", 620, 420);
}

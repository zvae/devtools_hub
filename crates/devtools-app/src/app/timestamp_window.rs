use super::*;

/// 打开截图风格的时间戳工具窗口。默认单窗口模式下重复激活只聚焦现有窗口。
pub(super) fn open_timestamp_window(
    dark_mode: bool,
    request_tx: mpsc::UnboundedSender<AppRequest>,
) {
    if SINGLE_TOOL_WINDOW.load(Ordering::Relaxed) {
        let has_existing = TIMESTAMP_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            if let Some(state) = windows.first() {
                show_window_in_foreground(&state.window.window());
                true
            } else {
                false
            }
        });
        if has_existing {
            return;
        }
    }

    let window = TimestampWindow::new().expect("failed to create timestamp window");
    window.set_pinned(false);
    let now = devtools_tools::timestamp_now_with_unit(true);
    let datetime = devtools_tools::timestamp_now_datetime();
    let parts = datetime_parts(&datetime);
    window.set_dark_mode(dark_mode);
    window.set_current_unit_index(1);
    window.set_current_timestamp(now.clone().into());
    window.set_timestamp_input(now.into());
    window.set_datetime_output(datetime.clone().into());
    window.set_datetime_input(datetime.into());
    window.set_datetime_timestamp_output("".into());
    window.set_datetime_unit_index(1);
    window.set_year_input(parts[0].clone().into());
    window.set_month_input(parts[1].clone().into());
    window.set_day_input(parts[2].clone().into());
    window.set_hour_input(parts[3].clone().into());
    window.set_minute_input(parts[4].clone().into());
    window.set_second_input(parts[5].clone().into());
    window.set_parts_output("".into());
    window.set_parts_unit_index(1);

    let refresh_window = window.as_weak();
    window.on_refresh_current(move || {
        if let Some(window) = refresh_window.upgrade() {
            window.set_current_timestamp(
                devtools_tools::timestamp_now_with_unit(window.get_current_unit_index() == 1)
                    .into(),
            );
        }
    });

    let unit_window = window.as_weak();
    window.on_current_unit_changed(move |index| {
        if let Some(window) = unit_window.upgrade() {
            window
                .set_current_timestamp(devtools_tools::timestamp_now_with_unit(index == 1).into());
        }
    });

    let timer = Rc::new(Timer::default());
    let clock_window = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(window) = clock_window.upgrade() {
            window.set_current_timestamp(
                devtools_tools::timestamp_now_with_unit(window.get_current_unit_index() == 1)
                    .into(),
            );
        }
    });
    let start_timer = Rc::clone(&timer);
    window.on_start_clock(move || {
        start_timer.restart();
    });
    let stop_timer = Rc::clone(&timer);
    window.on_stop_clock(move || {
        stop_timer.stop();
    });

    let unix_tx = request_tx.clone();
    window.on_convert_timestamp(move |input| {
        let _ = unix_tx.send(AppRequest::TimestampConvertUnix {
            input: input.to_string(),
        });
    });
    let datetime_tx = request_tx.clone();
    window.on_convert_datetime(move |input, unit_index| {
        let _ = datetime_tx.send(AppRequest::TimestampConvertDatetime {
            input: input.to_string(),
            milliseconds: unit_index == 1,
        });
    });
    window.on_convert_parts(move |year, month, day, hour, minute, second, unit_index| {
        let _ = request_tx.send(AppRequest::TimestampConvertParts {
            year: year.to_string(),
            month: month.to_string(),
            day: day.to_string(),
            hour: hour.to_string(),
            minute: minute.to_string(),
            second: second.to_string(),
            milliseconds: unit_index == 1,
        });
    });

    let validate_window = window.as_weak();
    window.on_validate_parts_inputs(move || {
        let Some(window) = validate_window.upgrade() else {
            return;
        };
        let year = window.get_year_input().trim().to_string();
        let month = window.get_month_input().trim().to_string();
        let day = window.get_day_input().trim().to_string();
        let hour = window.get_hour_input().trim().to_string();
        let minute = window.get_minute_input().trim().to_string();
        let second = window.get_second_input().trim().to_string();
        let checks: [(&str, i64, i64, &str); 6] = [
            (year.as_str(), 0, 9999, "年份应为 0-9999 的数字"),
            (month.as_str(), 1, 12, "月份应为 1-12"),
            (day.as_str(), 1, 31, "日期应为 1-31"),
            (hour.as_str(), 0, 23, "小时应为 0-23"),
            (minute.as_str(), 0, 59, "分钟应为 0-59"),
            (second.as_str(), 0, 59, "秒应为 0-59"),
        ];
        let mut valid = [true; 6];
        let mut error = "";
        for (index, (text, min, max, message)) in checks.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let ok = text
                .chars()
                .all(|character: char| character.is_ascii_digit())
                && text
                    .parse::<i64>()
                    .map_or(false, |value| (*min..=*max).contains(&value));
            valid[index] = ok;
            if !ok && error.is_empty() {
                error = message;
            }
        }
        window.set_parts_year_valid(valid[0]);
        window.set_parts_month_valid(valid[1]);
        window.set_parts_day_valid(valid[2]);
        window.set_parts_hour_valid(valid[3]);
        window.set_parts_minute_valid(valid[4]);
        window.set_parts_second_valid(valid[5]);
        window.set_parts_error(error.into());
    });
    window.on_copy_text(move |text| {
        let _ = set_clipboard_text(&text);
    });

    let pin_window = window.as_weak();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            window.set_pinned(!window.get_pinned());
        }
    });

    window.show().ok();
    TIMESTAMP_WINDOWS.with(|windows| {
        windows.borrow_mut().push(TimestampWindowState {
            window,
            _timer: timer,
        });
    });
}

pub(super) fn datetime_parts(datetime: &str) -> [String; 6] {
    let values = datetime
        .split(|character: char| !character.is_ascii_digit())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    std::array::from_fn(|index| values.get(index).cloned().unwrap_or_default())
}

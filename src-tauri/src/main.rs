// 正式版不跳出黑色主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{thread, time::Duration};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

/// 長條視窗的高度（邏輯像素，會依螢幕縮放比例換算）
const STRIP_HEIGHT: f64 = 150.0;

/// 前端判斷游標在不在貓或毛球上之後，呼叫這個開關「滑鼠穿透」
#[tauri::command]
fn set_passthrough(window: WebviewWindow, on: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(on).map_err(|e| e.to_string())
}

/// 把視窗擺成主螢幕底部、工作列正上方的一條長條
fn place_strip(win: &WebviewWindow) -> tauri::Result<()> {
    let monitor = match win.primary_monitor()? {
        Some(m) => m,
        None => match win.current_monitor()? {
            Some(m) => m,
            None => return Ok(()),
        },
    };
    // work_area = 螢幕扣掉工作列的範圍，工作列放在哪一邊都適用
    let area = monitor.work_area();
    let height = (STRIP_HEIGHT * monitor.scale_factor()).round() as u32;
    let bottom = area.position.y + area.size.height as i32;
    win.set_size(PhysicalSize::new(area.size.width, height))?;
    win.set_position(PhysicalPosition::new(area.position.x, bottom - height as i32))?;
    Ok(())
}

#[derive(Clone, Serialize)]
struct Cursor {
    x: f64,
    y: f64,
}

/// 視窗穿透時收不到任何滑鼠事件，所以由這裡主動輪詢游標位置，轉成視窗內座標交給前端
fn watch_cursor(win: WebviewWindow) {
    thread::spawn(move || {
        let mut last = (f64::NAN, f64::NAN);
        loop {
            thread::sleep(Duration::from_millis(16));
            let (Ok(c), Ok(p), Ok(scale)) =
                (win.cursor_position(), win.outer_position(), win.scale_factor())
            else {
                continue;
            };
            let x = (c.x - p.x as f64) / scale;
            let y = (c.y - p.y as f64) / scale;
            if (x, y) == last {
                continue;
            }
            last = (x, y);
            let _ = win.emit("cursor", Cursor { x, y });
        }
    });
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_passthrough])
        .setup(|app| {
            let win = app
                .get_webview_window("main")
                .expect("找不到 main 視窗");
            place_strip(&win)?;
            win.set_ignore_cursor_events(true)?;
            win.show()?;
            watch_cursor(win.clone());

            // 系統匣：換玩具、安靜模式（開會、分享螢幕時讓貓去睡）、結束
            let toy_ball = CheckMenuItem::with_id(app, "toy:ball", "毛球", true, true, None::<&str>)?;
            let toy_laser = CheckMenuItem::with_id(app, "toy:laser", "雷射光", true, false, None::<&str>)?;
            let toy_wand = CheckMenuItem::with_id(app, "toy:wand", "逗貓棒", true, false, None::<&str>)?;
            let toys = Submenu::with_items(app, "玩具", true, &[&toy_ball, &toy_laser, &toy_wand])?;
            let quiet = CheckMenuItem::with_id(app, "quiet", "安靜模式", true, false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束毛球貓", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toys, &quiet, &quit])?;
            let quiet_item = quiet.clone();
            let toy_items = [toy_ball, toy_laser, toy_wand];
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().expect("缺少圖示").clone())
                .tooltip("毛球貓")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quiet" => {
                        let on = quiet_item.is_checked().unwrap_or(false);
                        let _ = app.emit("quiet", on);
                    }
                    "quit" => app.exit(0),
                    // 玩具只能選一個：點到的打勾，其他取消
                    id if id.starts_with("toy:") => {
                        for item in &toy_items {
                            let mine: &str = item.id().as_ref();
                            let _ = item.set_checked(mine == id);
                        }
                        let _ = app.emit("toy", &id[4..]);
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("毛球貓啟動失敗");
}

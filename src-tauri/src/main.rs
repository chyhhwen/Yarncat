// 正式版不跳出黑色主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    window::Monitor,
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

/// 長條視窗的高度（邏輯像素，會依螢幕縮放比例換算）
const STRIP_HEIGHT: f64 = 150.0;

/// 雷射光、逗貓棒跟著游標跑遍全螢幕，要一直回報游標位置；
/// 毛球只在乎游標有沒有在長條裡，離開長條就不用回報
static FOLLOW_EVERYWHERE: AtomicBool = AtomicBool::new(false);

/// 系統匣「螢幕」選的螢幕（用 monitor_key 辨識）；None 表示主螢幕
static CHOSEN_SCREEN: Mutex<Option<String>> = Mutex::new(None);

/// 選單換了螢幕，請監看執行緒馬上重擺，不用等下一次的每秒檢查
static PLACE_NOW: AtomicBool = AtomicBool::new(false);

/// 每隔幾格（每格 16ms）檢查一次長條的位置，約 1 秒
const PLACE_CHECK_TICKS: u32 = 60;

/// 同一個目標位置最多補擺幾次，避免系統不肯配合時每秒都在搬
const PLACE_RETRIES: u8 = 3;

/// 前端判斷游標在不在貓或毛球上之後，呼叫這個開關「滑鼠穿透」
#[tauri::command]
fn set_passthrough(window: WebviewWindow, on: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(on).map_err(|e| e.to_string())
}

/// 長條該擺在哪、多大（實體像素）：(x, y, 寬, 高)
type Rect = (i32, i32, u32, u32);

/// 辨識螢幕用的名字（Windows 是 \\.\DISPLAY1 這類）；拿不到名字就用左上角座標
fn monitor_key(m: &Monitor) -> String {
    match m.name() {
        Some(n) => n.clone(),
        None => format!("{},{}", m.position().x, m.position().y),
    }
}

/// 貓要待的螢幕：系統匣選的那個；沒選、或那個螢幕已經拔掉了，就用主螢幕
fn pick_monitor(win: &WebviewWindow) -> tauri::Result<Option<Monitor>> {
    let chosen = CHOSEN_SCREEN.lock().ok().and_then(|c| c.clone());
    if let Some(key) = chosen {
        let found = win
            .available_monitors()?
            .into_iter()
            .find(|m| monitor_key(m) == key);
        if found.is_some() {
            return Ok(found);
        }
    }
    match win.primary_monitor()? {
        Some(m) => Ok(Some(m)),
        None => win.current_monitor(),
    }
}

/// 算出長條的位置：所選螢幕的底部、工作列正上方
fn strip_rect(win: &WebviewWindow) -> tauri::Result<Option<Rect>> {
    let Some(monitor) = pick_monitor(win)? else {
        return Ok(None);
    };
    // work_area = 螢幕扣掉工作列的範圍，工作列放在哪一邊都適用
    let area = monitor.work_area();
    let height = (STRIP_HEIGHT * monitor.scale_factor()).round() as u32;
    let bottom = area.position.y + area.size.height as i32;
    Ok(Some((area.position.x, bottom - height as i32, area.size.width, height)))
}

/// 把視窗擺到 rect。
/// 先搬位置再改大小：搬到縮放比例不同的螢幕時，Windows 會自動依新比例縮放視窗，
/// 最後再設一次大小，才會是我們算好的尺寸。
fn apply_rect(win: &WebviewWindow, r: Rect) -> tauri::Result<()> {
    win.set_position(PhysicalPosition::new(r.0, r.1))?;
    win.set_size(PhysicalSize::new(r.2, r.3))?;
    Ok(())
}

/// 視窗現在實際在哪、多大
fn actual_rect(win: &WebviewWindow) -> Option<Rect> {
    let p = win.outer_position().ok()?;
    let s = win.inner_size().ok()?;
    Some((p.x, p.y, s.width, s.height))
}

/// 換螢幕、改縮放、搬工作列之後，長條要跟著重擺。
/// 這些變化不一定有事件可聽，所以定期比對「該在的位置」和「實際位置」。
struct Placer {
    want: Option<Rect>,
    retries: u8,
}

impl Placer {
    fn check(&mut self, win: &WebviewWindow) {
        let Ok(Some(want)) = strip_rect(win) else {
            return;
        };
        if actual_rect(win) == Some(want) {
            self.want = Some(want);
            self.retries = 0;
            return;
        }
        if self.want == Some(want) {
            if self.retries >= PLACE_RETRIES {
                return;
            }
            self.retries += 1;
        } else {
            self.want = Some(want);
            self.retries = 1;
        }
        let _ = apply_rect(win, want);
    }
}

#[derive(Clone, Serialize)]
struct Cursor {
    x: f64,
    y: f64,
}

/// 換算游標座標要用的視窗資訊：左上角、大小（實體像素）、縮放比例
struct Geometry {
    rect: Rect,
    scale: f64,
}

fn read_geometry(win: &WebviewWindow) -> Option<Geometry> {
    Some(Geometry {
        rect: actual_rect(win)?,
        scale: win.scale_factor().ok()?,
    })
}

/// 視窗穿透時收不到任何滑鼠事件，所以由這裡主動輪詢游標位置，轉成視窗內座標交給前端。
/// 順便每秒檢查一次長條有沒有擺對位置。
fn watch_cursor(win: WebviewWindow) {
    thread::spawn(move || {
        let mut last = (f64::NAN, f64::NAN);
        let mut was_inside = false;
        let mut placer = Placer { want: None, retries: 0 };
        let mut geo = read_geometry(&win);
        let mut tick: u32 = 0;
        loop {
            thread::sleep(Duration::from_millis(16));
            tick = tick.wrapping_add(1);
            // 視窗位置、大小、縮放只有重擺時才會變，每秒讀一次就好，不用每格都問
            let now = PLACE_NOW.swap(false, Ordering::Relaxed);
            if now || tick % PLACE_CHECK_TICKS == 0 {
                placer.check(&win);
                geo = read_geometry(&win);
            }
            let (Some(g), Ok(c)) = (&geo, win.cursor_position()) else {
                continue;
            };
            let (left, top, w, h) = g.rect;
            let inside = c.x >= left as f64
                && c.x < left as f64 + w as f64
                && c.y >= top as f64
                && c.y < top as f64 + h as f64;
            // 毛球模式：游標在長條外就不回報，只在剛離開時補一次，讓前端把穿透打開
            let report = FOLLOW_EVERYWHERE.load(Ordering::Relaxed) || inside || was_inside;
            was_inside = inside;
            if !report {
                continue;
            }
            let x = (c.x - left as f64) / g.scale;
            let y = (c.y - top as f64) / g.scale;
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
            // macOS：不在 Dock 出現，只留選單列的貓咪圖示
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let win = app
                .get_webview_window("main")
                .expect("找不到 main 視窗");
            if let Some(r) = strip_rect(&win)? {
                apply_rect(&win, r)?;
            }
            win.set_ignore_cursor_events(true)?;
            win.show()?;
            watch_cursor(win.clone());

            // 系統匣：換玩具、換螢幕、安靜模式（開會、分享螢幕時讓貓去睡）、結束
            let toy_ball = CheckMenuItem::with_id(app, "toy:ball", "毛球", true, true, None::<&str>)?;
            let toy_laser = CheckMenuItem::with_id(app, "toy:laser", "雷射光", true, false, None::<&str>)?;
            let toy_wand = CheckMenuItem::with_id(app, "toy:wand", "逗貓棒", true, false, None::<&str>)?;
            let toys = Submenu::with_items(app, "玩具", true, &[&toy_ball, &toy_laser, &toy_wand])?;
            // 螢幕：啟動當下接著的螢幕，由左到右編號。之後才接上的螢幕要重開毛球貓才會出現
            let mut monitors = win.available_monitors()?;
            monitors.sort_by_key(|m| (m.position().x, m.position().y));
            let primary_key = win.primary_monitor()?.map(|m| monitor_key(&m));
            let mut screen_items = Vec::new();
            let mut screen_keys = Vec::new();
            for (i, m) in monitors.iter().enumerate() {
                let key = monitor_key(m);
                let is_primary = primary_key.as_deref() == Some(key.as_str());
                let label = format!(
                    "螢幕 {}：{}×{}{}",
                    i + 1,
                    m.size().width,
                    m.size().height,
                    if is_primary { "（主螢幕）" } else { "" }
                );
                let id = format!("screen:{i}");
                screen_items.push(CheckMenuItem::with_id(app, id, label, true, is_primary, None::<&str>)?);
                screen_keys.push(key);
            }
            let screen_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
                screen_items.iter().map(|it| it as &dyn IsMenuItem<tauri::Wry>).collect();
            let screens = Submenu::with_items(app, "螢幕", true, &screen_refs)?;
            let quiet = CheckMenuItem::with_id(app, "quiet", "安靜模式", true, false, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "結束毛球貓", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toys, &screens, &quiet, &quit])?;
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
                        FOLLOW_EVERYWHERE.store(id != "toy:ball", Ordering::Relaxed);
                        let _ = app.emit("toy", &id[4..]);
                    }
                    // 螢幕也只能選一個；選好之後請監看執行緒馬上把長條搬過去
                    id if id.starts_with("screen:") => {
                        for item in &screen_items {
                            let mine: &str = item.id().as_ref();
                            let _ = item.set_checked(mine == id);
                        }
                        let picked = id[7..].parse::<usize>().ok().and_then(|i| screen_keys.get(i));
                        if let (Some(key), Ok(mut chosen)) = (picked, CHOSEN_SCREEN.lock()) {
                            *chosen = Some(key.clone());
                            PLACE_NOW.store(true, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("毛球貓啟動失敗");
}

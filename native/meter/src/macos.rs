//! AppKit 연동(macOS 전용). 전부 메인 스레드에서만 부른다 — `cargo test` 는 메인이 아닌
//! 스레드라서 테스트는 이 모듈을 부르지 않는다. NSArray 는 `firstObject`·`objectEnumerator`
//! 로만 읽는다: macOS 26 디버그 빌드에서 `count`·`get`·`iter` 는 objc2 타입 검사가 패닉한다.

use egui::{Pos2, Rect, Vec2};
use objc2::runtime::{AnyClass, NSObjectProtocol};
use objc2_app_kit::{NSApplication, NSEvent, NSScreen, NSWindowCollectionBehavior};
use objc2_foundation::{MainThreadMarker, NSRect};

/// Dock·Cmd-Tab 없는 앱으로 시작하고 winit 기본 메뉴를 만들지 않는다. 기본 메뉴의
/// ⌘Q(`terminate:`)는 데몬의 마지막 전송(flush)을 건너뛰고 ⌘H 는 `hidden` 을 모른 채 숨긴다.
pub fn accessory_event_loop(b: &mut eframe::EventLoopBuilder<eframe::UserEvent>) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    b.with_activation_policy(ActivationPolicy::Accessory).with_default_menu(false);
}

/// 주 디스플레이(원점 (0,0), 메뉴바가 있는 화면) 높이. winit 도 이 높이로 y 를 뒤집는다.
fn primary_height(mtm: MainThreadMarker) -> f64 {
    unsafe { NSScreen::screens(mtm).firstObject() }
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0)
}

/// AppKit 사각형(아래-왼쪽 원점)을 egui 좌표(위-왼쪽 원점)로.
fn to_egui(r: NSRect, h: f64) -> Rect {
    Rect::from_min_size(
        Pos2::new(r.origin.x as f32, (h - r.origin.y - r.size.height) as f32),
        Vec2::new(r.size.width as f32, r.size.height as f32),
    )
}

/// winit 이 만든 창(메인·설정)에 "모든 Space · 전체화면 보조"를 붙이거나 뗀다. 레벨로 거르지 않는다
/// (창을 NSStatusWindowLevel 로 올리면 메뉴바 창과 구별되지 않는다). 이름 문자열 비교도 하지 않는다
/// (KVO 때문에 실행 중 이름은 NSKVONotifying_WinitWindow 다).
pub fn set_all_spaces(on: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(cls) = AnyClass::get("WinitWindow") else { return };
    let flags = NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary;
    let windows = NSApplication::sharedApplication(mtm).windows();
    let mut each = unsafe { windows.objectEnumerator() };
    while let Some(w) = each.nextObject() {
        if !w.isKindOfClass(cls) {
            continue;
        }
        let mut b = unsafe { w.collectionBehavior() };
        if on {
            b |= flags;
        } else {
            b &= !flags;
        }
        unsafe { w.setCollectionBehavior(b) };
    }
}

/// 전역 커서가 메인 창(`viewport().outer_rect`, egui 좌표) 안인지. 앱이 비활성이어도 된다.
pub fn cursor_over(outer: Rect) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let p = unsafe { NSEvent::mouseLocation() };
    outer.contains(Pos2::new(p.x as f32, (primary_height(mtm) - p.y) as f32))
}

/// 연결된 모든 모니터의 보이는 영역(egui 좌표). 못 읽으면 저장 위치를 그대로 쓰게 EVERYTHING.
pub fn screens() -> Vec<Rect> {
    let Some(mtm) = MainThreadMarker::new() else { return vec![Rect::EVERYTHING] };
    let h = primary_height(mtm);
    let all = NSScreen::screens(mtm);
    let mut out = Vec::new();
    let mut each = unsafe { all.objectEnumerator() };
    while let Some(s) = each.nextObject() {
        out.push(to_egui(s.visibleFrame(), h));
    }
    if out.is_empty() {
        out.push(Rect::EVERYTHING);
    }
    out
}

//! AppKit 연동(macOS 전용). 전부 메인 스레드에서만 부른다 — `cargo test` 는 메인이 아닌
//! 스레드라서 테스트는 이 모듈을 부르지 않는다. NSArray 는 `firstObject`·`objectEnumerator`
//! 로만 읽는다: macOS 26 디버그 빌드에서 `count`·`get`·`iter` 는 objc2 타입 검사가 패닉한다.

use egui::{Pos2, Rect, Vec2};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSAccessibility, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSBitmapFormat, NSBitmapImageRep, NSCellImagePosition, NSColor, NSControlStateValueOff, NSControlStateValueOn,
    NSDeviceRGBColorSpace, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSFont, NSFontAttributeName,
    NSFontWeightRegular, NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSScreen, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSWindowCollectionBehavior,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSAttributedString, NSCopying, NSDictionary, NSPoint, NSRect, NSSize,
    NSString,
};
use std::cell::OnceCell;
use std::sync::atomic::{AtomicU8, Ordering};

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

// ───────────── 메뉴바 미터 ─────────────

/// 클릭·메뉴 선택이 `update()` 로 넘어가는 자리(80ms 안에 두 번 오는 일은 무시한다).
static CMD: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StatusCmd {
    Toggle = 1,
    Settings = 3,
    ValueRate = 4,
    ValueCost = 5,
    BarAlways = 6,
    BarFolded = 7,
    AllSpaces = 8,
    Quit = 9,
}

/// 메뉴 순서와 명령. 1번 문구는 창이 떠 있으면 "TokenMeter 접기"로 바뀐다.
const MENU: [(&str, StatusCmd); 8] = [
    ("TokenMeter 열기", StatusCmd::Toggle),
    ("설정", StatusCmd::Settings),
    ("숫자: 속도", StatusCmd::ValueRate),
    ("숫자: 오늘 비용", StatusCmd::ValueCost),
    ("메뉴바 미터: 항상 표시", StatusCmd::BarAlways),
    ("메뉴바 미터: 창을 접었을 때만", StatusCmd::BarFolded),
    ("전체화면·모든 데스크톱에 표시", StatusCmd::AllSpaces),
    ("TokenMeter 종료 · 측정 중지", StatusCmd::Quit),
];
/// 이 순번 항목 뒤에 구분선.
const SEPARATOR_AFTER: [usize; 3] = [1, 3, 6];

pub fn take_cmd() -> Option<StatusCmd> {
    let v = CMD.swap(0, Ordering::Relaxed);
    MENU.iter().map(|(_, c)| *c).find(|c| *c as u8 == v)
}

pub struct TargetIvars {
    item: Retained<NSStatusItem>,
    menu: OnceCell<Retained<NSMenu>>,
}

declare_class!(
    struct Target;

    unsafe impl ClassType for Target {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "TokenMeterStatusTarget";
    }

    impl DeclaredClass for Target {
        type Ivars = TargetIvars;
    }

    unsafe impl Target {
        /// 버튼은 왼쪽·오른쪽 떼기 모두 여기로 온다(sendActionOn). 오른쪽·Ctrl-클릭이면 메뉴.
        #[method(statusClicked:)]
        fn status_clicked(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::from(self);
            let right = NSApplication::sharedApplication(mtm)
                .currentEvent()
                .map(|ev| {
                    let ctrl = unsafe { ev.modifierFlags() }.contains(NSEventModifierFlags::NSEventModifierFlagControl);
                    (unsafe { ev.r#type() }) == NSEventType::RightMouseUp || ctrl
                })
                .unwrap_or(false);
            if !right {
                CMD.store(StatusCmd::Toggle as u8, Ordering::Relaxed);
                return;
            }
            let (Some(menu), Some(button)) = (self.ivars().menu.get(), unsafe { self.ivars().item.button(mtm) }) else {
                return;
            };
            // 버튼은 flipped 라 y 가 아래로 커진다: 버튼 바로 아래에 띄운다.
            let below = NSPoint::new(0.0, button.bounds().size.height + 5.0);
            unsafe { menu.popUpMenuPositioningItem_atLocation_inView(None, below, Some(&button)) };
        }

        /// 메뉴 항목 선택. 명령은 항목의 tag 로 가린다(이때 현재 이벤트도 마우스 떼기라 구별이 안 된다).
        #[method(menuPicked:)]
        fn menu_picked(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                CMD.store(unsafe { item.tag() } as u8, Ordering::Relaxed);
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker, item: Retained<NSStatusItem>) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(TargetIvars { item, menu: OnceCell::new() });
        unsafe { msg_send_id![super(this), init] }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuState {
    pub open: bool,
    pub cost: bool,
    pub folded_only: bool,
    pub all_spaces: bool,
    pub lang: String,
}

/// 메뉴바 항목 하나. 값이 바뀐 것만 AppKit 에 넘긴다.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    /// NSControl·NSMenuItem 의 target 은 약한 참조라 여기서 붙잡아 둔다.
    _target: Retained<Target>,
    entries: Vec<Retained<NSMenuItem>>,
    bar: Option<(usize, bool)>,
    caption: String,
    tooltip: String,
    visible: Option<bool>,
    menu_state: Option<MenuState>,
}

impl StatusItem {
    pub fn create() -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let bar = unsafe { NSStatusBar::systemStatusBar() };
        let item = unsafe { bar.statusItemWithLength(NSVariableStatusItemLength) };
        // Cmd-드래그로 옮긴 자리를 macOS 가 기억한다(노치 뒤로 숨었을 때 빼낼 수 있게).
        unsafe { item.setAutosaveName(Some(ns_string!("TokenMeter"))) };
        let target = Target::new(mtm, item.clone());
        let button = unsafe { item.button(mtm) }?;
        unsafe {
            button.setImagePosition(NSCellImagePosition::NSImageLeft);
            // 기본은 왼쪽 떼기에만 액션을 보낸다. 이게 없으면 오른쪽 클릭이 오지 않는다.
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(statusClicked:)));
        }
        let menu = NSMenu::new(mtm);
        let mut entries = Vec::new();
        for (i, (label, cmd)) in MENU.iter().enumerate() {
            let mi = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(label),
                    Some(sel!(menuPicked:)),
                    ns_string!(""),
                )
            };
            unsafe {
                mi.setTarget(Some(&target));
                mi.setTag(*cmd as isize);
            }
            menu.addItem(&mi);
            if SEPARATOR_AFTER.contains(&i) {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            entries.push(mi);
        }
        let _ = target.ivars().menu.set(menu);
        Some(Self {
            item,
            _target: target,
            entries,
            bar: None,
            caption: String::new(),
            tooltip: String::new(),
            visible: None,
            menu_state: None,
        })
    }

    /// 메뉴바가 어두운지. NSApp 이 아니라 버튼의 appearance 를 본다(배경화면에 따라 메뉴바만 어둡다).
    pub fn dark(&self) -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return true };
        let Some(button) = (unsafe { self.item.button(mtm) }) else { return true };
        unsafe {
            let names = NSArray::from_vec(vec![NSAppearanceNameAqua.copy(), NSAppearanceNameDarkAqua.copy()]);
            let best = button.effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
            best.map(|n| &*n == NSAppearanceNameDarkAqua).unwrap_or(true)
        }
    }

    pub fn set_bar(&mut self, lit: usize, dark: bool) {
        if self.bar == Some((lit, dark)) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else { return };
        let img = crate::menubar::bar_image(lit, dark);
        let (w, h) = (img.w as isize, img.h as isize);
        // planes 를 비워 두면 rep 이 자기 버퍼를 만든다. 거기에 복사한다.
        let Some(rep) = (unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false,
                NSDeviceRGBColorSpace, NSBitmapFormat::empty(), w * 4, 32,
            )
        }) else {
            return;
        };
        unsafe { std::ptr::copy_nonoverlapping(img.rgba.as_ptr(), rep.bitmapData(), img.rgba.len()) };
        // 점 크기를 주지 않으면 78×36pt 로 잡힌다.
        let size = NSSize::new(w as f64 / 2.0, h as f64 / 2.0);
        unsafe { rep.setSize(size) };
        let image = unsafe { NSImage::initWithSize(NSImage::alloc(), size) };
        unsafe { image.addRepresentation(&rep) };
        if let Some(button) = unsafe { self.item.button(mtm) } {
            unsafe { button.setImage(Some(&image)) };
            self.bar = Some((lit, dark));
        }
    }

    /// 숫자(완전 등폭 글꼴, 오른쪽 정렬된 문자열)와 툴팁·손쉬운 사용 라벨.
    pub fn set_text(&mut self, caption: &str, tooltip: &str) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(button) = (unsafe { self.item.button(mtm) }) else { return };
        if self.caption != caption {
            unsafe {
                let font = NSFont::monospacedSystemFontOfSize_weight(0.0, NSFontWeightRegular);
                // 색을 주지 않으면 검정으로 그려져 어두운 메뉴바에서 흐리게 보인다.
                let keys: [&NSString; 2] = [NSFontAttributeName, NSForegroundColorAttributeName];
                let values: Vec<Retained<AnyObject>> = vec![Retained::cast(font), Retained::cast(NSColor::labelColor())];
                let attrs = NSDictionary::from_vec(&keys, values);
                let title = NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(caption),
                    Some(&*attrs),
                );
                button.setAttributedTitle(&title);
            }
            self.caption = caption.to_string();
        }
        if self.tooltip != tooltip {
            let text = NSString::from_str(tooltip);
            unsafe {
                button.setToolTip(Some(&text));
                button.setAccessibilityLabel(Some(&text));
            }
            self.tooltip = tooltip.to_string();
        }
    }

    pub fn set_visible(&mut self, on: bool) {
        if self.visible != Some(on) {
            unsafe { self.item.setVisible(on) };
            self.visible = Some(on);
        }
    }

    pub fn set_menu(&mut self, state: &MenuState) {
        if self.menu_state.as_ref() == Some(state) {
            return;
        }
        for (mi, (label, cmd)) in self.entries.iter().zip(MENU) {
            let key = if cmd == StatusCmd::Toggle && state.open { "TokenMeter 접기" } else { label };
            let on = match cmd {
                StatusCmd::ValueRate => !state.cost,
                StatusCmd::ValueCost => state.cost,
                StatusCmd::BarAlways => !state.folded_only,
                StatusCmd::BarFolded => state.folded_only,
                StatusCmd::AllSpaces => state.all_spaces,
                _ => false,
            };
            unsafe {
                mi.setTitle(&NSString::from_str(&crate::i18n::tr(&state.lang, key)));
                mi.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
            }
        }
        self.menu_state = Some(state.clone());
    }
}

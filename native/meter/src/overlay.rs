//! TokenMeter 계기판. 좌표와 색은 `overlay_contract_pins_snapshot_state_and_layout` 계약을 따른다.

use crate::attention::{attention_label, SessionView};
use crate::history::{
    day_noon, load_hours, load_rates, rate_series, rate_summary, series, summary, RateSeries,
    Series, RATE_SPANS, RATE_SPAN_TITLES, SPANS, SPAN_TITLES,
};
use crate::live_rate::LiveRate;
use crate::quota;
use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Key, Pos2, Rect,
    Stroke, Ui, Vec2, ViewportCommand,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokenmeter_hook::data_dir;

pub const DEFAULT_FULL_SCALE: f64 = 3000.0;
pub const SEGMENTS: i32 = 28;
const SPACE_1: f32 = 4.0;
const SPACE_2: f32 = 8.0;
pub const BASE_W: f32 = 340.0;
const HEADER_H: f32 = 28.0;
const SEARCH_H: f32 = 38.0;
const METER_H: f32 = 128.0;
const METER_H_L: f32 = 104.0;
const METER_H_S: f32 = 92.0;
const ROW_H: f32 = 28.0;
const FOOT_H: f32 = 22.0;
const PAD: f32 = 12.0;
const HEAD_H: f32 = 24.0;
const FILTER_H: f32 = 24.0;
const CHIP_H: f32 = 18.0;
const COLHEAD_H: f32 = 16.0;
const DETAIL_H: f32 = 44.0;
const MINI_W: f32 = 0.62;
const MINI_H: f32 = 30.0;
const MINI_RATE_W: f32 = 60.0;
const MODE_BTN: f32 = 28.0;
const HINT_SEC: f64 = 20.0;
const PALETTE_W: f32 = 420.0;
const PALETTE_ROW_H: f32 = 46.0;
const PALETTE_ROWS: usize = 7;
const SETTING_HEAD_H: f32 = 20.0;
const SETTING_ROW_H: f32 = 38.0;
const SETTING_CHIP_H: f32 = 36.0;
const SETTINGS_W: f32 = 380.0;
const SETTINGS_SEC_GAP: f32 = 18.0;
const SETTINGS_GROUP_GAP: f32 = 8.0;
const SETTINGS_CHROME: f32 = 14.0;
const SETTINGS_PAD: f32 = 16.0;
const EXPAND_W: f32 = 1.7;
const EXPAND_ROWS: i32 = 2;
const GRAPH_H: f32 = 130.0;
const GRAPH_TOP: f32 = 8.0;
const GRAPH_PEAK: f32 = 12.0;
const GRAPH_AXIS: f32 = 16.0;
const GRAPH_GAP: f32 = 1.6;
const BTN_W: f32 = 38.0;
const BTN_H: f32 = 24.0;
const RATE_BTN_W: f32 = 44.0;
const CELL_PAD: f32 = 4.0;
const CTX_WARN: f64 = 0.70;
const CTX_HOT: f64 = 0.90;
const WHEEL_LINE: f32 = 60.0;
const M_SESSION_ROWS: i32 = 6;
const PANELS: &[&str] = &["sessions", "projects", "quota", "rates", "days", "board"];
const BASIC_PANELS: &[&str] = &["sessions", "projects", "quota", "board"];
const SESSION_FILTERS: &[&str] = &["live", "archive", "all"];

#[derive(Clone)]
pub(crate) struct Theme {
    background_primary: Color32,
    surface_glass: Color32,
    surface_glass_elevated: Color32,
    surface_hover: Color32,
    text_primary: Color32,
    text_secondary: Color32,
    text_tertiary: Color32,
    tint: Color32,
    separator: Color32,
    destructive: Color32,
    success: Color32,
    warning: Color32,
    surface_alpha: u8,
    line_alpha: u8,
}

fn hex(s: &str, a: u8) -> Color32 {
    let t = s.trim_start_matches('#');
    let n = u32::from_str_radix(t, 16).unwrap_or(0);
    rgba(
        ((n >> 16) & 255) as u8,
        ((n >> 8) & 255) as u8,
        (n & 255) as u8,
        a,
    )
}

pub(crate) fn theme_named(name: &str) -> Theme {
    if name == "light" {
        Theme {
            background_primary: hex("#E7ECF2", 255),
            surface_glass: hex("#F8FAFD", 255),
            surface_glass_elevated: hex("#FFFFFF", 255),
            surface_hover: hex("#172033", 255),
            text_primary: hex("#18202B", 255),
            text_secondary: hex("#4F5B6D", 255),
            text_tertiary: hex("#5F6B7B", 255),
            tint: hex("#006EBE", 255),
            separator: hex("#172033", 255),
            destructive: hex("#B72D3D", 255),
            success: hex("#247B46", 255),
            warning: hex("#8A5B00", 255),
            surface_alpha: 240,
            line_alpha: 24,
        }
    } else {
        Theme {
            background_primary: hex("#0D0E13", 255),
            surface_glass: hex("#0D0E13", 255),
            surface_glass_elevated: hex("#15161F", 255),
            surface_hover: hex("#E8EAF2", 255),
            text_primary: hex("#E8EAF2", 255),
            text_secondary: hex("#C6CCDC", 255),
            text_tertiary: hex("#8B93A7", 255),
            tint: hex("#2BD9E5", 255),
            separator: hex("#232634", 255),
            destructive: hex("#FF5F6D", 255),
            success: hex("#3BE06A", 255),
            warning: hex("#FFC53D", 255),
            surface_alpha: 236,
            line_alpha: 255,
        }
    }
}

pub(crate) fn fade(c: Color32, a: u8) -> Color32 {
    rgba(c.r(), c.g(), c.b(), a)
}

/// Qt 처럼 sRGB 값에 알파를 곱한다. egui 0.31 의 `from_rgba_unmultiplied` 는 선형 공간에서 곱해서
/// 반투명 채움이 파이썬 계기판보다 2~3배 밝게 나온다.
fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let m = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(m(r), m(g), m(b), a)
}

const BOLD_FAMILY: &str = "bold";
const MONO_BOLD_FAMILY: &str = "mono-bold";

fn pretendard_regular() -> Option<FontData> {
    const BYTES: &[u8] = include_bytes!("../fonts/Pretendard-Regular.otf");
    (BYTES.len() > 100).then(|| FontData::from_static(BYTES))
}

fn pretendard_semibold() -> Option<FontData> {
    const BYTES: &[u8] = include_bytes!("../fonts/Pretendard-SemiBold.otf");
    (BYTES.len() > 100).then(|| FontData::from_static(BYTES))
}

/// 한글이 있는 시스템 폰트. egui 기본 폰트(Proggy 등)는 한글 글리프가 없다.
pub fn cjk_font_candidates() -> Vec<(&'static str, u32)> {
    #[cfg(target_os = "macos")]
    {
        vec![
            ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0),
            ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
            ("/Library/Fonts/Arial Unicode.ttf", 0),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![
            ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
            ("/usr/share/fonts/truetype/noto/NotoSansKR-Regular.otf", 0),
        ]
    }
}

fn load_cjk_font() -> Option<(FontData, String)> {
    for (path, index) in cjk_font_candidates() {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() < 100 {
                continue;
            }
            let mut font = FontData::from_owned(bytes);
            font.index = index;
            return Some((font, "cjk".into()));
        }
    }
    None
}

fn install_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(font) = pretendard_regular() {
        fonts
            .font_data
            .insert("pretendard".into(), std::sync::Arc::new(font));
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
            fam.insert(0, "pretendard".into());
        }
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
            fam.push("pretendard".into());
        }
    }
    if let Some(font) = pretendard_semibold() {
        fonts
            .font_data
            .insert("pretendard-bold".into(), std::sync::Arc::new(font));
    }
    #[cfg(target_os = "macos")]
    {
        if fonts.font_data.get("pretendard").is_none() {
            if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
                fonts
                    .font_data
                    .insert("sf".into(), std::sync::Arc::new(FontData::from_owned(bytes)));
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "sf".into());
                }
            }
        }
        if let Ok(bytes) = std::fs::read("/System/Library/Fonts/Menlo.ttc") {
            let mut bold = FontData::from_owned(bytes.clone());
            bold.index = 1;
            fonts
                .font_data
                .insert("menlo-bold".into(), std::sync::Arc::new(bold));
            let mut font = FontData::from_owned(bytes);
            font.index = 0;
            fonts
                .font_data
                .insert("menlo".into(), std::sync::Arc::new(font));
            if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
                fam.insert(0, "menlo".into());
            }
        }
    }
    if let Some((font, name)) = load_cjk_font() {
        fonts
            .font_data
            .insert(name.clone(), std::sync::Arc::new(font));
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
            let at = match fam.first().map(String::as_str) {
                Some("pretendard") | Some("sf") => 1,
                _ => 0,
            };
            fam.insert(at, name.clone());
        }
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
            fam.push(name);
        }
    }
    // 파이썬 `_f(size, True)`: 같은 대체 순서에서 Pretendard·Menlo 만 굵은 면으로 바꾼다.
    let bolder = |name: &String| match name.as_str() {
        "pretendard" if fonts.font_data.contains_key("pretendard-bold") => "pretendard-bold".to_string(),
        "menlo" if fonts.font_data.contains_key("menlo-bold") => "menlo-bold".to_string(),
        _ => name.clone(),
    };
    for (base, bold) in [
        (FontFamily::Proportional, BOLD_FAMILY),
        (FontFamily::Monospace, MONO_BOLD_FAMILY),
    ] {
        let list: Vec<String> = fonts
            .families
            .get(&base)
            .map(|l| l.iter().map(bolder).collect())
            .unwrap_or_default();
        fonts.families.insert(FontFamily::Name(bold.into()), list);
    }
    ctx.set_fonts(fonts);
}

/// 미니의 불투명도. 투명도 줄이기·설정/팔레트·커서가 먼저고, 그 밖엔 고른 농도.
fn mini_alpha(mini: bool, reduce_transparency: bool, panels_open: bool, hover: bool, level: &str) -> f32 {
    if !mini || reduce_transparency || panels_open || hover {
        return 1.0;
    }
    match level {
        "light" => 0.85,
        "strong" => 0.55,
        _ => 0.70,
    }
}

/// eframe 기본 지움 색(`epi.rs:215-219`, 덮어쓰면 기본 구현을 부를 수 없어 같은 식을 옮김).
/// 미니와 숨긴 창은 투명하게 지운다: 기본값(알파 180)이 깔리면 미니가 70% 아래로 흐려지지 않고,
/// 숨긴 채 시작하는 첫 프레임에 어두운 사각이 비친다.
fn clear_rgba(mini_look: bool, hidden: bool) -> [f32; 4] {
    if mini_look || hidden {
        [0.0; 4]
    } else {
        Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32()
    }
}

/// 커서가 메인 창 위에 있는지. macOS 는 전역 커서로 본다(winit 은 이동 이벤트를 키 창에만 보내
/// 비활성 앱에서는 egui hover 가 오지 않는다).
fn cursor_over(ctx: &egui::Context) -> bool {
    #[cfg(target_os = "macos")]
    {
        ctx.input(|i| i.viewport().outer_rect).map(crate::macos::cursor_over).unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        ctx.input(|i| i.pointer.hover_pos().is_some())
    }
}

/// 저장한 창 윗부분 띠(창 너비 × 24pt)가 화면 하나와 겹치면 그 자리, 아니면 기본 (40, 80).
fn restore_pos(saved: [f32; 2], screens: &[Rect], width: f32) -> Pos2 {
    let at = Pos2::new(saved[0], saved[1]);
    let strip = Rect::from_min_size(at, Vec2::new(width, 24.0));
    if screens.iter().any(|s| s.intersects(strip)) {
        at
    } else {
        Pos2::new(40.0, 80.0)
    }
}

/// 창을 둘 수 있는 화면(egui 좌표). macOS 는 모든 모니터, 그 밖은 지금 모니터 하나.
/// 모니터 정보가 없으면 저장 위치를 그대로 쓴다.
fn screens(ctx: &egui::Context) -> Vec<Rect> {
    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        crate::macos::screens()
    }
    #[cfg(not(target_os = "macos"))]
    {
        match ctx.input(|i| i.viewport().monitor_size) {
            Some(size) => vec![Rect::from_min_size(Pos2::ZERO, size)],
            None => vec![Rect::EVERYTHING],
        }
    }
}

pub fn gauge_target(rate: f64, full_scale: f64) -> f64 {
    let scale = full_scale.max(1.0);
    ((rate / scale).clamp(0.0, 1.0)).sqrt()
}

fn pulse_phase(active: bool, seconds: f64) -> f32 {
    if !active {
        return 0.0;
    }
    (0.5 + 0.5 * (seconds * std::f64::consts::TAU).sin()) as f32
}

pub fn compact_num(n: f64) -> String {
    if n >= 1_000_000_000_000.0 {
        format!("{:.1}T", n / 1_000_000_000_000.0)
    } else if n >= 1_000_000_000.0 {
        format!("{:.1}B", n / 1_000_000_000.0)
    } else if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{}", n as i64)
    }
}

pub fn ctx_status_caption(ratio: f64, has_window: bool) -> String {
    if !has_window {
        return "미상".into();
    }
    let pct = format!("{:.0}%", ratio.max(0.0) * 100.0);
    if ratio >= 0.90 {
        format!("{pct} · 높음")
    } else {
        pct
    }
}

fn money_short(v: f64) -> String {
    if v >= 100.0 {
        format!("${}", comma_int(v.round() as i64))
    } else if v >= 0.1 {
        format!("${v:.2}")
    } else {
        format!("${v:.3}")
    }
}

fn comma_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let digits: String = out.chars().rev().collect();
    if n < 0 {
        format!("-{digits}")
    } else {
        digits
    }
}

fn comma_rate(rate: f64) -> String {
    let rounded = (rate * 10.0).round() / 10.0;
    let int = rounded.trunc() as i64;
    let frac = ((rounded - int as f64).abs() * 10.0).round() as i64;
    format!("{}.{}", comma_int(int), frac)
}

pub(crate) fn money_caption(approx: bool, amount: f64) -> String {
    let text = format!("${:.2}", amount);
    let text = if amount.abs() >= 1000.0 {
        let int = amount.trunc() as i64;
        let frac = ((amount - int as f64).abs() * 100.0).round() as i64;
        format!("${}.{:02}", comma_int(int), frac)
    } else {
        text
    };
    if approx {
        format!("환산 {text}")
    } else {
        text
    }
}

fn stamp(ts: f64) -> String {
    if ts <= 0.0 {
        return String::new();
    }
    let c = crate::history::civil_of(ts);
    format!("{:02}-{:02} {:02}:{:02}", c.month, c.day, c.hour, c.minute)
}

fn age_caption(ts: f64, now: f64) -> String {
    if ts <= 0.0 {
        return "갱신 시각 미상".into();
    }
    let sec = ((now - ts).max(0.0)) as i64;
    if sec < 60 {
        "방금 갱신".into()
    } else if sec < 3600 {
        format!("{}분 전 갱신", sec / 60)
    } else if sec < 86400 {
        format!("{}시간 전 갱신", sec / 3600)
    } else {
        format!("{}일 전 갱신", sec / 86400)
    }
}

fn short_model(name: &str) -> String {
    let mut s = name.trim().to_string();
    if let Some((_, rest)) = s.split_once('/') {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("claude-") {
        s = rest.to_string();
    }
    if s.len() >= 9 {
        let tail = &s[s.len() - 9..];
        if tail.starts_with("-20") && tail.as_bytes()[3..].iter().all(|b| b.is_ascii_digit()) {
            s.truncate(s.len() - 9);
        }
    }
    if let Some(rest) = s.strip_suffix("-build") {
        s = rest.to_string();
    }
    if s.is_empty() {
        "측정 전".into()
    } else {
        s
    }
}

fn short_effort(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "minimal" => "min".into(),
        "low" => "low".into(),
        "medium" => "med".into(),
        "high" => "high".into(),
        "xhigh" => "xhi".into(),
        "max" => "max".into(),
        "ultra" => "ult".into(),
        other => other.chars().take(4).collect(),
    }
}

fn rate_text(v: f64) -> String {
    if v >= 0.5 {
        format!("{}/s", compact_num(v))
    } else {
        String::new()
    }
}

fn s_skin_name(value: &str) -> &'static str {
    match value {
        "dial" => "dial",
        "loot" | "horse" => "loot",
        _ => "bar",
    }
}

pub(crate) fn mini_rate_caption(value: f64) -> String {
    let mut scaled = value.max(0.0);
    let mut unit = "";
    for suffix in ["k", "M", "G", "T", "P"] {
        if scaled < 999.5 {
            break;
        }
        scaled /= 1000.0;
        unit = suffix;
    }
    if scaled >= 999.5 {
        return "MAX/s".into();
    }
    let decimals = if scaled < (if unit.is_empty() { 99.95 } else { 9.95 }) {
        1
    } else {
        0
    };
    format!("{scaled:.decimals$}{unit}/s")
}

fn project_label(project: &str, cwd: &str) -> String {
    let name = if !cwd.is_empty() {
        tokenmeter_hook::project_key(cwd)
    } else {
        project.to_string()
    };
    if name.is_empty() || name == "(unknown)" {
        return "폴더 미상".into();
    }
    if let Ok(home) = std::env::var("HOME") {
        if name == tokenmeter_hook::project_key(&home) {
            return "홈 폴더".into();
        }
    }
    name
}

fn search_score(query: &str, parts: &[&str]) -> i32 {
    let needle = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    if needle.is_empty() {
        return 1;
    }
    let haystack = parts
        .iter()
        .map(|p| p.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if haystack.starts_with(&needle) {
        return 300 - (haystack.len() as i32 - needle.len() as i32).min(100);
    }
    if let Some(at) = haystack.find(&needle) {
        return 220 - (at as i32).min(100);
    }
    let compact_needle: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
    let compact_hay: String = haystack.chars().filter(|c| !c.is_whitespace()).collect();
    let mut cursor: i32 = -1;
    let mut gap = 0;
    let hay: Vec<char> = compact_hay.chars().collect();
    for ch in compact_needle.chars() {
        let start = (cursor + 1) as usize;
        let found = hay[start..].iter().position(|c| *c == ch);
        let Some(rel) = found else {
            return 0;
        };
        let at = start as i32 + rel as i32;
        gap += at - cursor - 1;
        cursor = at;
    }
    (120 - gap).max(1)
}

fn empty_loop(kind: &str, now: f64) -> String {
    let lines: &[&str] = match kind {
        "sessions.live" => &[
            "실시간 세션 없음 — 실행 중인 에이전트가 여기에 표시됩니다",
            "GOGOGO! 프롬프트 하나면 미터가 살아납니다",
            "아직 조용하다. 코딩 에이전트를 한 번 굴려 보세요",
        ],
        "sessions.archive" => &[
            "보관 세션 없음 — 종료된 에이전트가 여기에 표시됩니다",
            "끝난 대화는 여기로 내려옵니다. 지금 한 판 더?",
            "GOGOGO! 새 세션이 끝나면 보관이 채워집니다",
        ],
        "sessions.all" => &[
            "기록된 세션 없음 — 세션은 에이전트 대화 하나입니다",
            "GOGOGO! 에이전트를 재시작하고 프롬프트를 보내 보세요",
            "미터가 기다립니다. 첫 토큰이 오는 순간 여기가 켜집니다",
        ],
        "projects" => &[
            "에이전트를 실행한 폴더가 아직 없습니다",
            "GOGOGO! 프로젝트 폴더에서 에이전트를 한 번 돌리면 쌓입니다",
            "폴더는 실행 위치입니다. 지금 그 디렉터리에서 시작해 보세요",
        ],
        "quota" => &[
            "자격 없음 · Claude/Codex/Grok 로그인",
            "한도는 로그인한 플랜에서 읽습니다. GOGOGO!",
            "로그인된 CLI가 있으면 잔여 창이 여기 뜹니다",
        ],
        "rates" => &[
            "아직 작업 속도 기록이 없습니다",
            "GOGOGO! 출력이 흐르면 모델별 속도가 쌓입니다",
            "메인 모델이 토큰을 뱉는 순간 여기가 움직입니다",
        ],
        "days" => &[
            "히스토리 없음 — 하루가 지나면 쌓입니다",
            "GOGOGO! 오늘을 쓰면 내일 여기 막대가 생깁니다",
            "한 시간이 지나면 그래프가 그려집니다",
        ],
        "board.login" => &[
            "리그를 보려면 로그인이 필요합니다",
            "GOGOGO! 터미널에서 tokenmeter league login",
            "로그인 한 줄이면 초대 링크를 만들 수 있습니다",
        ],
        "board.open" => &[
            "아직 참가한 방이 없습니다",
            "GOGOGO! 초대 링크를 만들거나 받은 링크를 열어 보세요",
            "설정에서 초대 링크를 복사하면 방이 열립니다",
        ],
        "board.empty" => &[
            "아직 리그 데이터가 없습니다",
            "GOGOGO! 링크를 공유하면 게이지에 눈금이 생깁니다",
            "친구가 들어오는 순간 여기가 레이스가 됩니다",
        ],
        _ => return String::new(),
    };
    let index = ((now.max(0.0) / 4.0) as usize) % lines.len();
    lines[index].into()
}

fn check_reason(event: &str) -> &'static str {
    match event {
        "PermissionRequest" | "permission.asked" | "permission.v2.asked" => "권한",
        "question.asked" | "question.v2.asked" => "질문",
        "Stop" | "session.idle" => "중지",
        "Notification" => "알림",
        _ => "",
    }
}

fn wait_caption(now: f64, at: f64) -> String {
    if at <= 0.0 {
        return String::new();
    }
    let sec = ((now - at).max(0.0)) as i64;
    if sec < 60 {
        "방금".into()
    } else if sec < 3600 {
        format!("{}분", sec / 60)
    } else {
        format!("{}시간", sec / 3600)
    }
}

fn league_color(uid: &str) -> Color32 {
    const COLORS: [&str; 8] = [
        "#FF5F6D", "#FFC53D", "#3BE06A", "#7B8CFF", "#E879F9", "#FF8A3D", "#5EEAD4", "#F472B6",
    ];
    let mut n: u32 = 0;
    for ch in uid.chars() {
        n = n.wrapping_mul(33).wrapping_add(ch as u32);
    }
    hex(COLORS[(n as usize) % COLORS.len()], 255)
}

fn tokens_of(v: &Value) -> i64 {
    let t = v
        .get("totals")
        .and_then(Value::as_object)
        .or_else(|| v.as_object());
    t.map(|m| {
        ["input_tokens", "output_tokens", "cache_read", "cache_write"]
            .iter()
            .map(|k| m.get(*k).and_then(Value::as_f64).unwrap_or(0.0) as i64)
            .sum()
    })
    .unwrap_or(0)
}

fn health_note(status: &Value, now: f64) -> String {
    let live = status.get("live_count").and_then(Value::as_i64).unwrap_or(0);
    let has = status
        .get("sessions")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty());
    if live <= 0 && !has {
        return "첫 세션 대기 중 · 에이전트를 재시작하세요".into();
    }
    let updated = status.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0);
    if live > 0 && updated > 0.0 && now - updated > 120.0 {
        return "측정이 멈춤 · tokenmeter doctor".into();
    }
    String::new()
}

fn panel_title(name: &str) -> &str {
    match name {
        "board" => "리그",
        "days" => "일별",
        "sessions" => "세션",
        "projects" => "프로젝트",
        "rates" => "속도",
        "quota" => "한도",
        _ => name,
    }
}

fn filter_title(name: &str) -> &str {
    match name {
        "live" => "실시간",
        "archive" => "보관",
        "all" => "전체",
        _ => name,
    }
}

fn span_title(name: &str) -> &str {
    SPAN_TITLES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .unwrap_or(name)
}

fn rate_span_title(name: &str) -> &str {
    RATE_SPAN_TITLES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .unwrap_or(name)
}

fn panel_rows_cap(panel: &str) -> i32 {
    match panel {
        "board" => 5,
        "days" => 7,
        "sessions" => 10,
        "projects" => 7,
        "rates" => 6,
        "quota" => 8,
        _ => 5,
    }
}

/// state.json 스냅샷 계약. 이 파일을 쪼개지 말고, 필드·상수가 바뀌면
/// `overlay_contract_pins_snapshot_state_and_layout` 를 먼저 맞춘다.
#[derive(Clone, Default)]
pub struct MeterSnapshot {
    pub rate: f64,
    pub full_scale: f64,
    pub incoming: i64,
    pub status: Value,
    pub sessions: Vec<SessionView>,
    pub session_rates: HashMap<String, f64>,
    pub output_tokens: i64,
    pub input_tokens: i64,
    pub cache_tokens: i64,
    pub cache_saved: f64,
    pub today_output: i64,
    pub total_output: i64,
    pub total_input: i64,
    pub total_cache: i64,
    pub total_saved: f64,
    pub projects: Vec<(String, i64, f64)>,
    pub days: Vec<(String, i64, f64)>,
    pub quota: Vec<(String, String, String, String)>,
    pub league_room: bool,
    pub league_rows: Vec<(String, f64, bool)>,
    pub league_note: String,
    pub rates: Vec<(String, f64)>,
}

pub type SharedMeter = Arc<Mutex<MeterSnapshot>>;

fn totals_of(state: &Value, path: &str) -> (i64, i64, i64, i64, f64) {
    let t = state.pointer(path).and_then(Value::as_object);
    let n = |k: &str| {
        t.and_then(|m| m.get(k))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    (
        n("input_tokens") as i64,
        n("output_tokens") as i64,
        n("cache_read") as i64,
        n("cache_write") as i64,
        n("cache_saved_usd"),
    )
}

pub fn project_rows(state: &Value) -> Vec<(String, i64, f64)> {
    let Some(book) = state.get("projects").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut rows: Vec<(String, i64, f64)> = book
        .iter()
        .map(|(name, rec)| {
            let tokens = rec
                .get("totals")
                .and_then(Value::as_object)
                .map(|t| {
                    ["input_tokens", "output_tokens", "cache_read", "cache_write"]
                        .iter()
                        .map(|k| t.get(*k).and_then(Value::as_f64).unwrap_or(0.0) as i64)
                        .sum()
                })
                .unwrap_or(0);
            let last = rec.get("last_seen").and_then(Value::as_f64).unwrap_or(0.0);
            (name.clone(), tokens, last)
        })
        .collect();
    rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

pub fn day_rows(state: &Value) -> Vec<(String, i64, f64)> {
    let mut book = state
        .get("days")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(today) = state.get("today").and_then(Value::as_object) {
        if let (Some(day), Some(totals)) = (
            today.get("date").and_then(Value::as_str),
            today.get("totals"),
        ) {
            if !day.is_empty() {
                book.insert(day.to_string(), totals.clone());
            }
        }
    }
    let mut rows: Vec<(String, i64, f64)> = book
        .iter()
        .map(|(day, rec)| {
            let totals = rec.as_object();
            let tokens = totals
                .map(|t| {
                    ["input_tokens", "output_tokens", "cache_read", "cache_write"]
                        .iter()
                        .map(|k| t.get(*k).and_then(Value::as_f64).unwrap_or(0.0) as i64)
                        .sum()
                })
                .unwrap_or(0);
            let cost = totals
                .and_then(|t| t.get("cost_usd"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            (day.clone(), tokens, cost)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows
}

pub fn filter_session_rows<'a>(rows: &'a [SessionView], mode: &str) -> Vec<&'a SessionView> {
    match mode {
        "archive" => rows.iter().filter(|r| !r.live).collect(),
        "all" => rows.iter().collect(),
        "check" | "working" | "waiting" | "done" => {
            rows.iter().filter(|r| r.attention == mode).collect()
        }
        _ => rows.iter().filter(|r| r.live).collect(),
    }
}

pub fn load_quota_rows() -> Vec<(String, String, String, String)> {
    quota::panel_rows(
        quota::load()
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or(&[]),
        None,
    )
    .into_iter()
    .map(|(t, l, u, r, s)| {
        (
            t,
            l,
            if u >= 0.0 {
                format!("{:.0}%", u * 100.0)
            } else {
                "-".into()
            },
            s + &format!(" {r}"),
        )
    })
    .collect()
}

fn load_league_rows() -> (bool, Vec<(String, f64, bool)>, String) {
    if !crate::league::enabled() || crate::league::list_rooms().is_empty() {
        return (false, Vec::new(), String::new());
    }
    let my_uid = std::fs::read_to_string(data_dir().join("league-auth.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("uid").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let cache = std::fs::read_to_string(data_dir().join("league-cache.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let now = crate::watch::now_secs();
    let mut rows: Vec<(String, f64, bool)> = cache
        .as_ref()
        .and_then(|value| value.get("members"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(uid, row)| {
            let at = row.get("at").and_then(Value::as_f64)?;
            if now - at > 15.0 {
                return None;
            }
            let handle = row
                .get("handle")
                .and_then(Value::as_str)
                .unwrap_or(uid)
                .to_string();
            let tps = row.get("tps").and_then(Value::as_f64).unwrap_or(0.0);
            Some((handle, tps, uid == &my_uid))
        })
        .collect();
    rows.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let note = if rows.is_empty() {
        "리그 동기화 중"
    } else {
        "라이브 tok/s"
    };
    (true, rows, note.into())
}

pub fn snapshot_from(state: &Value, rate: &LiveRate) -> MeterSnapshot {
    snapshot_from_scale(state, rate, DEFAULT_FULL_SCALE)
}

pub fn snapshot_from_scale(state: &Value, rate: &LiveRate, full_scale: f64) -> MeterSnapshot {
    let now = crate::watch::now_secs();
    let sessions = crate::attention::session_views(state, now);
    let (tin, tout, cread, cwrite, saved) = totals_of(state, "/today/totals");
    let (tin_all, tout_all, cread_all, cwrite_all, saved_all) = totals_of(state, "/total/totals");
    let (league_room, league_rows, league_note) = load_league_rows();
    MeterSnapshot {
        rate: rate.rate,
        full_scale: full_scale.max(1.0),
        incoming: 0,
        status: state.clone(),
        sessions,
        session_rates: rate.rates.clone(),
        output_tokens: tout,
        input_tokens: tin,
        cache_tokens: cread + cwrite,
        cache_saved: saved,
        today_output: tout,
        total_output: tout_all,
        total_input: tin_all,
        total_cache: cread_all + cwrite_all,
        total_saved: saved_all,
        projects: project_rows(state),
        days: day_rows(state),
        quota: load_quota_rows(),
        league_room,
        league_rows,
        league_note,
        rates: {
            let mut rows: Vec<(String, f64)> =
                rate.rates.iter().map(|(k, v)| (k.clone(), *v)).collect();
            rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            rows
        },
    }
}

struct ViewportState {
    hidden: bool,
    width: f32,
    height: f32,
    on_top: bool,
}

impl ViewportState {
    fn new(hidden: bool) -> Self {
        Self {
            hidden,
            width: 340.0,
            height: 430.0,
            on_top: true,
        }
    }

    fn commands(
        &mut self,
        hidden: bool,
        width: f32,
        height: f32,
        on_top: bool,
    ) -> Vec<ViewportCommand> {
        let mut commands = Vec::new();
        if self.hidden != hidden {
            self.hidden = hidden;
            commands.push(ViewportCommand::Visible(!hidden));
        }
        if (self.width - width).abs() > 0.5 || (self.height - height).abs() > 0.5 {
            self.width = width;
            self.height = height;
            commands.push(ViewportCommand::InnerSize(Vec2::new(width, height)));
        }
        if self.on_top != on_top {
            self.on_top = on_top;
            commands.push(ViewportCommand::WindowLevel(if on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
        }
        commands
    }
}

struct OverlayApp {
    shared: SharedMeter,
    lang: String,
    theme_mode: String,
    reduce_transparency: bool,
    reduce_motion: bool,
    mini_opacity: String,
    mini_hover: bool,
    all_spaces: bool,
    settings_frames: u32,
    menubar: String,
    menubar_value: String,
    #[cfg(target_os = "macos")]
    status: Option<crate::macos::StatusItem>,
    scale: f32,
    rows_on: bool,
    on_top: bool,
    scope_today: bool,
    panel: String,
    expanded: bool,
    span: String,
    rate_span: String,
    filter: String,
    quota_reps: Map<String, Value>,
    mini: bool,
    s_skin: String,
    face_phase: f64,
    money_stage: i32,
    hint_seen: bool,
    hint_until: f64,
    focus: Option<(String, String)>,
    open_key: String,
    scroll: i32,
    wheel: f32,
    settings_open: bool,
    hidden: bool,
    palette_open: bool,
    palette_index: usize,
    palette_query: String,
    gauge: f64,
    peak: f64,
    pulse: f64,
    league_marks: HashMap<String, f64>,
    viewport: ViewportState,
    pos: [f32; 2],
    placed: bool,
    feedback: String,
    reset_arm: bool,
    note: String,
    pending_copy: String,
    shot_dir: Option<PathBuf>,
    shot_queue: Vec<String>,
    shot_current: String,
    shot_warm: u8,
}

fn prefs_path() -> PathBuf {
    data_dir().join("overlay.json")
}

fn load_prefs() -> Value {
    std::fs::read_to_string(prefs_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

impl OverlayApp {
    fn mode(&self) -> char {
        if self.expanded {
            'L'
        } else if self.rows_on {
            'M'
        } else {
            'S'
        }
    }

    fn set_mode(&mut self, name: char) {
        if name == self.mode() {
            return;
        }
        self.expanded = name == 'L';
        self.rows_on = name != 'S';
        if !self.expanded {
            self.focus = None;
        }
        self.ensure_panel();
        self.save_prefs();
    }

    fn panel_choices(&self) -> Vec<&'static str> {
        if crate::league::enabled() || crate::board::online() {
            PANELS.to_vec()
        } else {
            PANELS.iter().copied().filter(|p| *p != "board").collect()
        }
    }

    fn visible_panels(&self) -> Vec<&'static str> {
        let choices = self.panel_choices();
        if self.expanded {
            return choices;
        }
        if self.league_room_now() && choices.contains(&"board") {
            return vec!["board"];
        }
        choices
            .into_iter()
            .filter(|p| BASIC_PANELS.contains(p))
            .collect()
    }

    fn league_room_now(&self) -> bool {
        crate::league::enabled() && !crate::league::list_rooms().is_empty()
    }

    fn ensure_panel(&mut self) {
        let panels = self.visible_panels();
        if !panels.contains(&self.panel.as_str()) {
            if let Some(first) = panels.first() {
                self.panel = (*first).into();
                self.scroll = 0;
                self.open_key.clear();
            }
        }
    }

    fn cap(&self, panel: &str) -> i32 {
        let n = panel_rows_cap(panel);
        if panel == "sessions" {
            if self.expanded {
                n
            } else {
                M_SESSION_ROWS
            }
        } else if self.expanded {
            n * EXPAND_ROWS
        } else {
            n
        }
    }

    fn shows_graph(&self) -> bool {
        self.expanded && matches!(self.panel.as_str(), "rates" | "days")
    }

    fn shows_session_filters(&self, empty: bool) -> bool {
        self.panel == "sessions" && (empty || self.filter != "live")
    }

    fn chip_h(&self, marks: usize) -> f32 {
        if self.rows_on && marks > 0 {
            CHIP_H
        } else {
            0.0
        }
    }

    /// 픽셀 얼굴은 미니 전용이다(v0.7.2). S 는 늘 기본 계기판.
    fn widget_skin(&self) -> Option<&'static str> {
        if self.palette_open || !self.mini || self.s_skin == "bar" {
            return None;
        }
        Some(s_skin_name(&self.s_skin))
    }

    fn skin_size(&self) -> Option<(f32, f32)> {
        self.widget_skin()?;
        let (w, h) = crate::pixel_faces::MINI_SIZE;
        Some((w * self.scale, h * self.scale))
    }

    fn window_size(&self, row_count: i32, marks: usize, palette_n: usize) -> (f32, f32) {
        let s = self.scale;
        if self.palette_open {
            let rows = palette_n.clamp(3, PALETTE_ROWS) as f32;
            let w = BASE_W.max(PALETTE_W) * s;
            let meter_h = if self.mode() == 'S' {
                METER_H_S
            } else if self.expanded {
                METER_H_L
            } else {
                METER_H
            };
            let h = (PAD * 2.0 + meter_h + SPACE_2 + SEARCH_H + SPACE_2 + rows * PALETTE_ROW_H
                + FOOT_H)
                * s;
            return (w, h);
        }
        if let Some(sz) = self.skin_size() {
            return sz;
        }
        if self.mini {
            return (BASE_W * MINI_W * s, MINI_H * s);
        }
        let extra = if self.panel == "sessions" {
            COLHEAD_H
                + if self.shows_session_filters(row_count == 0) {
                    FILTER_H + SPACE_1
                } else {
                    0.0
                }
        } else if self.panel == "rates" {
            COLHEAD_H + if self.expanded { 0.0 } else { BTN_H }
        } else if matches!(self.panel.as_str(), "quota" | "projects") {
            COLHEAD_H
        } else {
            0.0
        };
        let head = HEAD_H
            + SPACE_1
            + extra
            + if self.panel == "sessions" && self.shows_session_filters(row_count == 0) {
                SPACE_1
            } else {
                0.0
            };
        let meter_h = if self.mode() == 'S' {
            METER_H_S
        } else if self.expanded {
            METER_H_L
        } else {
            METER_H
        };
        let mut h = PAD * 2.0
            + meter_h
            + self.chip_h(marks)
            + if row_count > 0 {
                head + row_count as f32 * ROW_H + 8.0
            } else {
                0.0
            }
            + FOOT_H;
        let w = BASE_W * if self.expanded { EXPAND_W } else { 1.0 };
        if self.shows_graph() {
            h += GRAPH_H;
        }
        if !self.open_key.is_empty() && self.panel == "sessions" {
            h += DETAIL_H;
        }
        (w * s, h * s)
    }

    fn shooting(&self) -> bool {
        self.shot_dir.is_some()
    }

    fn apply_shot(&mut self, name: &str) {
        self.mini = false;
        self.settings_open = false;
        self.palette_open = false;
        self.open_key.clear();
        self.focus = None;
        self.scroll = 0;
        self.hint_until = 0.0;
        self.hint_seen = true;
        match name {
            "L-quota" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "quota".into();
            }
            "L-sessions" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "sessions".into();
                self.filter = "archive".into();
            }
            "L-projects" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "projects".into();
            }
            "L-days" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "days".into();
            }
            "L-rates" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "rates".into();
            }
            "M-sessions" => {
                self.expanded = false;
                self.rows_on = true;
                self.panel = "sessions".into();
                self.filter = "live".into();
            }
            "S" => {
                self.expanded = false;
                self.rows_on = false;
            }
            "mini" => {
                self.mini = true;
            }
            "settings" => {
                self.expanded = true;
                self.rows_on = true;
                self.panel = "sessions".into();
                self.settings_open = true;
            }
            _ => {}
        }
    }

    fn save_prefs(&self) {
        if self.shooting() {
            return;
        }
        let v = json!({
            "pos": self.pos,
            "scale": (self.scale as f64 * 1000.0).round() / 1000.0,
            "rows": self.rows_on,
            "on_top": self.on_top,
            "scope": if self.scope_today { "today" } else { "total" },
            "panel": self.panel,
            "expanded": self.expanded,
            "span": self.span,
            "rate_span": self.rate_span,
            "filter": self.filter,
            "quota_representatives": self.quota_reps,
            "mini": self.mini,
            "s_skin": self.s_skin,
            "hint_seen": self.hint_seen,
            "theme": self.theme_mode,
            "lang": self.lang,
            "reduce_transparency": self.reduce_transparency,
            "reduce_motion": self.reduce_motion,
            "mini_opacity": self.mini_opacity,
            "mini_hover": self.mini_hover,
            "all_spaces": self.all_spaces,
            "menubar": self.menubar,
            "menubar_value": self.menubar_value,
        });
        let _ = std::fs::write(prefs_path(), v.to_string());
    }

    fn from_prefs(shared: SharedMeter, hidden: bool) -> Self {
        let p = load_prefs();
        let theme_mode = p
            .get("theme")
            .and_then(Value::as_str)
            .unwrap_or("dark")
            .to_string();
        let theme_mode = if matches!(theme_mode.as_str(), "system" | "dark" | "light") {
            theme_mode
        } else {
            "dark".into()
        };
        let panel = p
            .get("panel")
            .and_then(Value::as_str)
            .unwrap_or("sessions")
            .to_string();
        let panel = if PANELS.contains(&panel.as_str()) {
            panel
        } else {
            "sessions".into()
        };
        let span = p
            .get("span")
            .and_then(Value::as_str)
            .unwrap_or("today")
            .to_string();
        let filter = p
            .get("filter")
            .and_then(Value::as_str)
            .unwrap_or("live")
            .to_string();
        let pos = p
            .get("pos")
            .and_then(Value::as_array)
            .and_then(|a| {
                Some([
                    a.first().and_then(Value::as_f64).unwrap_or(40.0) as f32,
                    a.get(1).and_then(Value::as_f64).unwrap_or(80.0) as f32,
                ])
            })
            .unwrap_or([40.0, 80.0]);
        let hint_seen = p.get("hint_seen").and_then(Value::as_bool).unwrap_or(false);
        let reps = p
            .get("quota_representatives")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let shot_dir = std::env::var("TOKENMETER_SHOT_DIR").ok().map(PathBuf::from);
        let mut shot_queue = if shot_dir.is_some() {
            [
                "L-quota",
                "L-sessions",
                "L-projects",
                "L-days",
                "L-rates",
                "M-sessions",
                "S",
                "mini",
                "settings",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let shot_current = if shot_queue.is_empty() {
            String::new()
        } else {
            shot_queue.remove(0)
        };
        let lang = crate::i18n::normalize(
            p.get("lang").and_then(Value::as_str).unwrap_or("ko"),
        )
        .to_string();
        Self {
            shared,
            lang,
            theme_mode,
            reduce_transparency: p
                .get("reduce_transparency")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            reduce_motion: p
                .get("reduce_motion")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            mini_opacity: p
                .get("mini_opacity")
                .and_then(Value::as_str)
                .filter(|s| matches!(*s, "light" | "mid" | "strong"))
                .unwrap_or("mid")
                .into(),
            mini_hover: p.get("mini_hover").and_then(Value::as_bool).unwrap_or(true),
            all_spaces: p.get("all_spaces").and_then(Value::as_bool).unwrap_or(true),
            settings_frames: 0,
            menubar: if p.get("menubar").and_then(Value::as_str) == Some("folded") { "folded".into() } else { "always".into() },
            menubar_value: if p.get("menubar_value").and_then(Value::as_str) == Some("cost") { "cost".into() } else { "rate".into() },
            #[cfg(target_os = "macos")]
            status: None,
            scale: p
                .get("scale")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(1.0, 2.0) as f32,
            rows_on: p.get("rows").and_then(Value::as_bool).unwrap_or(true),
            on_top: p.get("on_top").and_then(Value::as_bool).unwrap_or(true),
            scope_today: p.get("scope").and_then(Value::as_str) != Some("total"),
            panel,
            expanded: p.get("expanded").and_then(Value::as_bool).unwrap_or(false),
            span: if SPANS.contains(&span.as_str()) {
                span
            } else {
                "today".into()
            },
            rate_span: p
                .get("rate_span")
                .and_then(Value::as_str)
                .filter(|s| RATE_SPANS.contains(s))
                .unwrap_or("1h")
                .into(),
            filter: if SESSION_FILTERS.contains(&filter.as_str()) {
                filter
            } else {
                "live".into()
            },
            quota_reps: reps,
            mini: p.get("mini").and_then(Value::as_bool).unwrap_or(false),
            s_skin: s_skin_name(p.get("s_skin").and_then(Value::as_str).unwrap_or("bar")).into(),
            face_phase: 0.0,
            money_stage: 0,
            hint_seen,
            hint_until: if hint_seen {
                0.0
            } else {
                crate::watch::now_secs() + HINT_SEC
            },
            focus: None,
            open_key: String::new(),
            scroll: 0,
            wheel: 0.0,
            settings_open: false,
            hidden,
            palette_open: false,
            palette_index: 0,
            palette_query: String::new(),
            gauge: 0.0,
            peak: 0.0,
            pulse: 0.0,
            league_marks: HashMap::new(),
            // 첫 프레임에 Visible(false)를 내야 숨는다: eframe은 첫 프레임 뒤 창을 강제로 보이게 하고(epi_integration.rs:306-311) 그 다음에 앱의 창 명령을 적용한다(glow_integration.rs:707 → :732).
            viewport: ViewportState::new(false),
            pos,
            placed: false,
            feedback: String::new(),
            reset_arm: false,
            note: String::new(),
            pending_copy: String::new(),
            shot_dir,
            shot_queue,
            shot_current,
            shot_warm: 0,
        }
    }

    fn resolved_theme(&self, dark_system: bool) -> Theme {
        let name = match self.theme_mode.as_str() {
            "light" => "light",
            "system" => {
                if dark_system {
                    "dark"
                } else {
                    "light"
                }
            }
            _ => "dark",
        };
        theme_named(name)
    }

    fn advance(&mut self, dt: f64, target: f64, racers: &[(String, f64, Color32, String)]) {
        if self.reduce_motion {
            self.gauge = target;
            self.peak = 0.0;
            self.pulse = 0.0;
            self.league_marks = racers
                .iter()
                .map(|(_, tps, _, uid)| (uid.clone(), gauge_target(*tps, DEFAULT_FULL_SCALE)))
                .collect();
            return;
        }
        let step = dt.min(0.05);
        self.gauge +=
            (target - self.gauge) * (step * if target > self.gauge { 9.0 } else { 1.6 }).min(1.0);
        if target <= 0.0 && self.gauge < 0.004 {
            self.gauge = 0.0;
        }
        self.peak = target.max(self.peak - step * 0.22);
        self.pulse = (self.pulse - step * 2.4).max(0.0);
        let mut nxt = HashMap::new();
        for (_, tps, _, uid) in racers {
            let want = gauge_target(*tps, DEFAULT_FULL_SCALE);
            let mut pos = self.league_marks.get(uid).copied().unwrap_or(0.0);
            pos += (want - pos) * (step * if want > pos { 9.0 } else { 1.6 }).min(1.0);
            if want <= 0.0 && pos < 0.004 {
                pos = 0.0;
            }
            nxt.insert(uid.clone(), pos);
        }
        self.league_marks = nxt;
    }

    fn end_hint(&mut self) {
        if !self.hint_seen || self.hint_until > 0.0 {
            self.hint_until = 0.0;
            self.hint_seen = true;
            self.save_prefs();
        }
    }
}

struct Paint<'a> {
    ui: &'a mut Ui,
    hits: &'a mut Vec<(String, Rect)>,
    theme: Theme,
    scale: f32,
    lang: &'a str,
}

impl Paint<'_> {
    fn s(&self) -> f32 {
        self.scale
    }

    fn fill(&self, r: Rect, c: Color32) {
        self.ui.painter().rect_filled(r, CornerRadius::ZERO, c);
    }

    /// 파이썬 `_f(size, bold, mono)` 와 같은 글꼴. bold 는 `install_cjk_fonts` 가 묶은 가족을 쓴다.
    fn font(&self, size: f32, mono: bool, bold: bool) -> FontId {
        FontId::new(
            (size * self.scale).max(6.0),
            match (mono, bold) {
                (false, false) => FontFamily::Proportional,
                (true, false) => FontFamily::Monospace,
                (false, true) => FontFamily::Name(BOLD_FAMILY.into()),
                (true, true) => FontFamily::Name(MONO_BOLD_FAMILY.into()),
            },
        )
    }

    fn measure(&self, text: &str, size: f32, mono: bool, bold: bool) -> f32 {
        let font = self.font(size, mono, bold);
        self.ui
            .fonts(|f| f.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x)
    }

    fn elide(&self, text: &str, width: f32, size: f32, mono: bool, bold: bool) -> String {
        if self.measure(text, size, mono, bold) <= width || text.is_empty() {
            return text.to_string();
        }
        let mut t: String = text.chars().collect();
        while !t.is_empty() {
            let shown = format!("{t}…");
            if self.measure(&shown, size, mono, bold) <= width {
                return shown;
            }
            t.pop();
        }
        "…".into()
    }

    fn text(
        &self,
        r: Rect,
        text: &str,
        color: Color32,
        size: f32,
        right: bool,
        center: bool,
        elide: bool,
        mono: bool,
        bold: bool,
    ) {
        let localized = crate::i18n::tr(self.lang, text);
        let shown = if elide {
            self.elide(&localized, r.width().max(0.0), size, mono, bold)
        } else {
            localized
        };
        let align = if right {
            Align2::RIGHT_CENTER
        } else if center {
            Align2::CENTER_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let pos = if right {
            r.right_center()
        } else if center {
            r.center()
        } else {
            r.left_center()
        };
        self.ui
            .painter()
            .text(pos, align, shown, self.font(size, mono, bold), color);
    }

    fn hit(&mut self, name: impl Into<String>, r: Rect) {
        self.hits.push((name.into(), r));
    }
}

pub(crate) fn seg_color(theme: &Theme, position: f32) -> Color32 {
    if position < 0.55 {
        theme.success
    } else if position < 0.8 {
        theme.warning
    } else {
        theme.destructive
    }
}

fn kind_color(theme: &Theme, kind: &str) -> Color32 {
    match kind {
        "input" => theme.success,
        "output" => theme.tint,
        "cache_read" => theme.text_tertiary,
        "cache_write" => theme.warning,
        _ => theme.text_secondary,
    }
}

fn state_color(theme: &Theme, attention: &str) -> Color32 {
    match attention {
        "check" => theme.destructive,
        "working" => theme.success,
        "waiting" => theme.text_secondary,
        _ => theme.text_tertiary,
    }
}

fn ctx_color(theme: &Theme, ratio: f64) -> Color32 {
    if ratio >= CTX_HOT {
        theme.destructive
    } else if ratio >= CTX_WARN {
        theme.warning
    } else {
        theme.text_tertiary
    }
}

fn league_racers() -> Vec<(String, f64, Color32, String)> {
    if !crate::league::enabled() {
        return Vec::new();
    }
    let my_uid = std::fs::read_to_string(data_dir().join("league-auth.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("uid").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let cache = std::fs::read_to_string(data_dir().join("league-cache.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let now = crate::watch::now_secs();
    let mut rows = Vec::new();
    if let Some(members) = cache.as_ref().and_then(|v| v.get("members")).and_then(Value::as_object)
    {
        for (uid, row) in members {
            if uid == &my_uid {
                continue;
            }
            let Some(at) = row.get("at").and_then(Value::as_f64) else {
                continue;
            };
            if now - at > 15.0 {
                continue;
            }
            let handle = row
                .get("handle")
                .and_then(Value::as_str)
                .unwrap_or(uid)
                .to_string();
            let tps = row.get("tps").and_then(Value::as_f64).unwrap_or(0.0);
            rows.push((handle, tps, league_color(uid), uid.clone()));
        }
    }
    rows.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    rows
}

fn hit_test(hits: &[(String, Rect)], pos: Pos2) -> Option<String> {
    hits.iter()
        .find(|(_, r)| r.contains(pos))
        .map(|(n, _)| n.clone())
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // 숨김은 창을 실제로 숨길 수 있는 macOS 에서만 투명하게 지운다. Wayland 는 Visible(false) 가
        // 아무것도 하지 않아(winit wayland/window/mod.rs:253) 투명하게 지우면 보이지 않는 창이 클릭을 먹는다.
        clear_rgba(
            self.mini && !self.palette_open && !self.hidden,
            self.hidden && cfg!(target_os = "macos"),
        )
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if crate::daemon::stopping() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(80));
        let show = data_dir().join("overlay.show");
        if show.exists() {
            let _ = std::fs::remove_file(&show);
            self.hidden = false;
            ctx.request_repaint();
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.hidden = true;
            self.settings_open = false;
            self.palette_open = false;
            self.save_prefs();
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        // 숨긴 동안 설정 뷰포트는 그려지지 않아 egui가 지운다. 다시 열면 새 창이라 첫 프레임을 다시 숨긴다.
        if self.hidden || !self.settings_open { self.settings_frames = 0; }
        #[cfg(target_os = "macos")]
        self.tick_status(ctx);
        if self.hidden {
            for cmd in self
                .viewport
                .commands(true, self.viewport.width, self.viewport.height, self.on_top)
            {
                ctx.send_viewport_cmd(cmd);
            }
            return;
        }
        if !self.pending_copy.is_empty() {
            ctx.copy_text(std::mem::take(&mut self.pending_copy));
        }
        let snap = self.shared.lock().map(|g| g.clone()).unwrap_or_default();
        if snap.incoming > 0 {
            self.pulse = 1.0;
        }
        let dt = ctx.input(|i| i.unstable_dt as f64).clamp(0.001, 0.2);
        let racers = league_racers();
        let target = gauge_target(snap.rate, snap.full_scale);
        self.advance(dt, target, &racers);
        if !self.reduce_motion && self.widget_skin().is_some() && snap.rate > 0.01 {
            self.face_phase = (self.face_phase + dt.min(0.05) * (0.7 + target * 2.0)) % 120.0;
        }
        self.money_stage = crate::pixel_faces::money_stage(snap.rate, self.money_stage);
        if self.hint_until > 0.0 && crate::watch::now_secs() > self.hint_until {
            self.end_hint();
        }
        self.ensure_panel();
        let dark_system = ctx.style().visuals.dark_mode;
        let theme = self.resolved_theme(dark_system);
        let quota_snap = quota::load();
        let windows: Vec<Value> = quota_snap
            .get("windows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let marks = if self.rows_on {
            quota::chips(&windows, &self.quota_reps)
        } else {
            Vec::new()
        };
        let row_n = self.preview_row_count(&snap, &windows);
        let palette = if self.palette_open {
            self.palette_items(&snap)
        } else {
            Vec::new()
        };
        let (ww, hh) = self.window_size(row_n, marks.len(), palette.len());
        let alpha = if self.reduce_transparency {
            255
        } else {
            theme.surface_alpha
        };
        let mut hits = Vec::new();
        let skin = self.widget_skin().is_some();
        let hover = self.mini_hover && cursor_over(ctx);
        let k_target = mini_alpha(
            self.mini,
            self.reduce_transparency,
            self.settings_open || self.palette_open,
            hover,
            &self.mini_opacity,
        );
        // 전환 중에는 egui 가 스스로 다시 그려 화면 주사율로 바뀐다. S/M/L 은 애니메이션 없이 늘 1.0.
        let k = if self.reduce_motion || !self.mini {
            k_target
        } else {
            ctx.animate_value_with_time(egui::Id::new("tokenmeter-mini-alpha"), k_target, 0.15)
        };
        let fill = if skin {
            Color32::TRANSPARENT
        } else {
            fade(theme.surface_glass, alpha)
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(fill.gamma_multiply(k)))
            .show(ctx, |ui| {
                ui.multiply_opacity(k);
                ui.set_min_size(Vec2::new(ww, hh));
                let full = ui.max_rect();
                if !skin {
                    self.fill_hairline(ui, &theme, full);
                }
                let lang = self.lang.clone();
                let mut p = Paint {
                    ui,
                    hits: &mut hits,
                    theme: theme.clone(),
                    scale: self.scale,
                    lang: &lang,
                };
                if skin {
                    paint_s_skin(&mut p, self, &snap, alpha);
                } else if self.mini {
                    paint_mini(&mut p, self, &snap);
                } else {
                    paint_body(&mut p, self, &snap, &marks, &windows, &palette, &racers);
                }
            });
        self.handle_input(ctx, &hits, &snap, &windows, &palette);
        if self.settings_open {
            paint_settings_viewport(ctx, self);
        }
        if !self.placed {
            let at = restore_pos(self.pos, &screens(ctx), ww);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(at));
            self.pos = [at.x, at.y];
            self.placed = true;
            self.save_prefs();
        }
        for cmd in self.viewport.commands(false, ww, hh, self.on_top) {
            ctx.send_viewport_cmd(cmd);
        }
        if ctx.input(|i| i.pointer.primary_released()) {
            if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
                self.pos = [rect.min.x, rect.min.y];
                self.save_prefs();
            }
        }
        self.tick_shots(ctx);
    }
}

impl OverlayApp {
    /// 메뉴 명령을 처리하고 메뉴바 막대·숫자·메뉴를 맞춘다. 창을 숨겨 둬도 80ms마다 돈다.
    #[cfg(target_os = "macos")]
    fn tick_status(&mut self, ctx: &egui::Context) {
        use crate::macos::{MenuState, StatusCmd};
        use crate::menubar::{caption, lit, padded, tooltip, Readout};
        if let Some(cmd) = crate::macos::take_cmd() {
            match cmd {
                StatusCmd::Toggle if self.hidden => {
                    self.hidden = false;
                    ctx.request_repaint();
                }
                StatusCmd::Toggle => self.fold(),
                StatusCmd::Settings => {
                    self.hidden = false;
                    self.settings_open = true;
                    ctx.request_repaint();
                }
                StatusCmd::ValueRate | StatusCmd::ValueCost => {
                    self.menubar_value = if cmd == StatusCmd::ValueCost { "cost" } else { "rate" }.into();
                    self.save_prefs();
                }
                StatusCmd::BarAlways | StatusCmd::BarFolded => {
                    self.menubar = if cmd == StatusCmd::BarFolded { "folded" } else { "always" }.into();
                    self.save_prefs();
                }
                StatusCmd::AllSpaces => {
                    self.all_spaces = !self.all_spaces;
                    crate::macos::set_all_spaces(self.all_spaces);
                    self.save_prefs();
                }
                StatusCmd::Quit => {
                    self.save_prefs();
                    crate::daemon::request_stop();
                }
            }
        }
        let (rate, full, today) = self
            .shared
            .lock()
            .map(|g| (g.rate, g.full_scale, g.status.pointer("/today/totals/cost_usd").and_then(Value::as_f64).unwrap_or(0.0)))
            .unwrap_or((0.0, DEFAULT_FULL_SCALE, 0.0));
        let readout = if self.menubar_value == "cost" { Readout::Cost } else { Readout::Rate };
        let menu = MenuState {
            open: !self.hidden,
            cost: readout == Readout::Cost,
            folded_only: self.menubar == "folded",
            all_spaces: self.all_spaces,
            lang: self.lang.clone(),
        };
        let Some(status) = self.status.as_mut() else { return };
        let dark = status.dark();
        status.set_bar(lit(gauge_target(rate, full)), dark);
        status.set_text(&padded(readout, &caption(readout, rate, today)), &tooltip(&self.lang, rate, today));
        status.set_visible(!menu.folded_only || self.hidden);
        status.set_menu(&menu);
    }

    /// 창을 메뉴바로 접는다(기존 닫기와 같은 상태).
    #[cfg(target_os = "macos")]
    fn fold(&mut self) {
        self.hidden = true;
        self.settings_open = false;
        self.palette_open = false;
        self.save_prefs();
    }

    fn tick_shots(&mut self, _ctx: &egui::Context) {
        let Some(dir) = self.shot_dir.clone() else {
            return;
        };
        if !self.shot_current.is_empty() && self.shot_warm == 0 {
            let name = self.shot_current.clone();
            self.apply_shot(&name);
        }
        self.shot_warm = self.shot_warm.saturating_add(1);
        if self.shot_warm == 15 {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("need.txt"), &self.shot_current);
        }
        if self.shot_warm < 15 {
            return;
        }
        let done = std::fs::read_to_string(dir.join("done.txt")).unwrap_or_default();
        if done.trim() != self.shot_current {
            return;
        }
        let _ = std::fs::remove_file(dir.join("need.txt"));
        let _ = std::fs::remove_file(dir.join("done.txt"));
        self.shot_warm = 0;
        if self.shot_queue.is_empty() {
            self.shot_dir = None;
            self.shot_current.clear();
            return;
        }
        self.shot_current = self.shot_queue.remove(0);
    }

    fn fill_hairline(&self, ui: &Ui, theme: &Theme, full: Rect) {
        ui.painter().rect_filled(
            Rect::from_min_size(full.min, Vec2::new(full.width(), 1.0 * self.scale)),
            CornerRadius::ZERO,
            fade(hex("#FFFFFF", 255), 22),
        );
        let _ = theme;
    }

    fn preview_row_count(&self, snap: &MeterSnapshot, windows: &[Value]) -> i32 {
        if !self.rows_on || self.mini {
            return 0;
        }
        let n = match self.panel.as_str() {
            "sessions" => filter_session_rows(&snap.sessions, &self.filter).len() as i32,
            "projects" => display_projects(&snap.status).len() as i32,
            "quota" => windows.len() as i32,
            "rates" => {
                let data = rate_series(&load_rates(Some(&snap.status)), &self.rate_span, crate::watch::now_secs());
                data.rows.len() as i32
            }
            "days" => snap.days.len() as i32,
            "board" => league_racers().len() as i32,
            _ => 0,
        };
        n.max(1).min(self.cap(&self.panel))
    }

    fn palette_items(&self, snap: &MeterSnapshot) -> Vec<(String, String, String, String)> {
        let mut items = Vec::new();
        let visible = self.visible_panels();
        for panel in self.panel_choices() {
            let (title, sub) = match panel {
                "sessions" => ("세션", "현재 에이전트 상태와 컨텍스트"),
                "projects" => ("프로젝트", "측정 토큰 · 최근 세션 순"),
                "rates" => ("속도", "모델별 출력 처리량"),
                "days" => ("사용량 히스토리", "오늘 · 7일 · 30일"),
                "quota" => ("플랜 한도", "Claude · Codex · Grok"),
                "board" => ("리그", "Token League · 초대와 사용량"),
                _ => continue,
            };
            let tag = if panel == self.panel {
                "현재"
            } else if visible.contains(&panel) {
                "보기"
            } else {
                "상세"
            };
            items.push((
                format!("panel:{panel}"),
                title.into(),
                sub.into(),
                tag.into(),
            ));
        }
        items.extend([
            (
                "filter:live".into(),
                "실시간 세션".into(),
                "현재 실행 중인 에이전트".into(),
                "필터".into(),
            ),
            (
                "filter:archive".into(),
                "보관 세션".into(),
                "종료된 에이전트".into(),
                "필터".into(),
            ),
            (
                "filter:all".into(),
                "모든 세션".into(),
                "종료된 기록 포함".into(),
                "필터".into(),
            ),
            (
                "mode:S".into(),
                "미터만 보기".into(),
                "콘텐츠 목록 접기".into(),
                "보기".into(),
            ),
            (
                "mode:M".into(),
                "기본 보기".into(),
                "미터와 핵심 목록".into(),
                "보기".into(),
            ),
            (
                "mode:L".into(),
                "상세 보기".into(),
                "넓은 히스토리와 더 많은 결과".into(),
                "보기".into(),
            ),
            (
                "command:scope".into(),
                if self.scope_today {
                    "누적 보기"
                } else {
                    "오늘 보기"
                }
                .into(),
                "사용량 범위 전환".into(),
                "명령".into(),
            ),
            (
                "command:settings".into(),
                "빠른 설정".into(),
                "모양 · 동작 · 창 크기".into(),
                "⌘,".into(),
            ),
        ]);
        let mut sessions = snap.sessions.clone();
        sessions.sort_by(session_ord);
        for rec in sessions {
            let project = project_label(&rec.project, &rec.cwd);
            let ctx = ctx_status_caption(
                if rec.ctx_win > 0 {
                    rec.ctx as f64 / rec.ctx_win as f64
                } else {
                    0.0
                },
                rec.ctx_win > 0,
            );
            items.push((
                format!("session:{}", rec.key),
                project,
                format!(
                    "{} · {} · 컨텍스트 {ctx}",
                    attention_label(&rec.attention),
                    short_model(&rec.model)
                ),
                "세션".into(),
            ));
        }
        let q = self.palette_query.clone();
        let mut ranked: Vec<(i32, (String, String, String, String))> = items
            .into_iter()
            .filter_map(|item| {
                let score = search_score(&q, &[&item.1, &item.2, &item.0]);
                (score > 0).then_some((score, item))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0));
        ranked.into_iter().take(PALETTE_ROWS).map(|(_, i)| i).collect()
    }

    fn handle_input(
        &mut self,
        ctx: &egui::Context,
        hits: &[(String, Rect)],
        snap: &MeterSnapshot,
        windows: &[Value],
        palette: &[(String, String, String, String)],
    ) {
        let command = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
        if command && ctx.input(|i| i.key_pressed(Key::K)) {
            self.palette_open = !self.palette_open;
            self.settings_open = false;
            return;
        }
        if command && ctx.input(|i| i.key_pressed(Key::Comma)) {
            self.settings_open = !self.settings_open;
            self.palette_open = false;
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.palette_open {
                self.palette_open = false;
            } else if self.settings_open {
                self.settings_open = false;
            } else if self.focus.is_some() || !self.open_key.is_empty() {
                self.focus = None;
                self.open_key.clear();
            } else {
                self.hidden = true;
                self.save_prefs();
            }
            return;
        }
        if self.palette_open {
            if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
                self.palette_index = (self.palette_index + 1).min(palette.len().saturating_sub(1));
            }
            if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
                self.palette_index = self.palette_index.saturating_sub(1);
            }
            if ctx.input(|i| i.key_pressed(Key::Enter)) {
                if let Some(item) = palette.get(self.palette_index) {
                    self.activate(&item.0, snap, windows);
                    self.palette_open = false;
                }
            }
        }
        if ctx.input(|i| i.pointer.any_click() && i.pointer.button_double_clicked(egui::PointerButton::Primary))
            && self.mini
        {
            self.mini = false;
            self.save_prefs();
            return;
        }
        if let Some(pos) = ctx.pointer_interact_pos() {
            if let Some(delta) = ctx.input(|i| {
                let y = i.raw_scroll_delta.y;
                (y.abs() > 0.0).then_some(y)
            })
            {
                let target = hit_test(hits, pos).unwrap_or_default();
                if self.rows_on && target.starts_with("row:") {
                    self.wheel += delta;
                    let step = (self.wheel / WHEEL_LINE) as i32;
                    if step != 0 {
                        self.wheel -= step as f32 * WHEEL_LINE;
                        self.scroll = (self.scroll - step).max(0);
                    }
                } else {
                    self.scale = (self.scale * 1.08_f32.powf(delta / 120.0)).clamp(1.0, 2.0);
                    self.save_prefs();
                }
            }
            if ctx.input(|i| i.pointer.secondary_clicked()) {
                self.activate("menu", snap, windows);
            }
            if ctx.input(|i| i.pointer.primary_pressed()) {
                self.end_hint();
                if let Some(name) = hit_test(hits, pos) {
                    if name.starts_with("palette:") {
                        if let Ok(i) = name[8..].parse::<usize>() {
                            self.palette_index = i;
                            if let Some(item) = palette.get(i) {
                                self.activate(&item.0, snap, windows);
                                self.palette_open = false;
                            }
                        }
                    } else {
                        self.activate(&name, snap, windows);
                    }
                } else if !self.palette_open {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
            }
        }
    }

    fn advance_league(&mut self) {
        match crate::league::overlay_step() {
            "login" => {
                crate::league::spawn_login_cli();
                self.feedback = "브라우저에서 로그인하세요".into();
            }
            "open" => {
                let _ = crate::league::open_room("cost");
                let rid = crate::league::current_focus();
                let url = crate::league::invite_link(&rid);
                if url.is_empty() {
                    self.feedback = "방을 여는 중…".into();
                } else {
                    self.pending_copy = url;
                    self.feedback = "초대 링크를 복사했습니다".into();
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self, target: &str, snap: &MeterSnapshot, windows: &[Value]) {
        if target == "scope" || target == "command:scope" {
            self.scope_today = !self.scope_today;
            self.save_prefs();
            return;
        }
        if target == "menu" || target == "command:settings" {
            self.settings_open = !self.settings_open;
            self.palette_open = false;
            return;
        }
        if target == "close" {
            self.hidden = true;
            self.settings_open = false;
            self.palette_open = false;
            self.save_prefs();
            return;
        }
        if target == "search" {
            self.palette_open = true;
            return;
        }
        if let Some(name) = target.strip_prefix("mode:") {
            if let Some(ch) = name.chars().next() {
                self.set_mode(ch);
            }
            return;
        }
        if let Some(name) = target.strip_prefix("lang:") {
            self.lang = crate::i18n::normalize(name).into();
            self.save_prefs();
            return;
        }
        if let Some(name) = target.strip_prefix("theme:") {
            self.theme_mode = name.into();
            self.save_prefs();
            return;
        }
        if let Some(name) = target.strip_prefix("skin:") {
            self.s_skin = s_skin_name(name).into();
            self.mini = self.s_skin != "bar";
            self.face_phase = 0.0;
            self.money_stage = 0;
            self.save_prefs();
            return;
        }
        if let Some(level) = target.strip_prefix("mini_opacity:") {
            self.mini_opacity = level.into();
            self.save_prefs();
            return;
        }
        if target == "command:mini_hover" {
            self.mini_hover = !self.mini_hover;
            self.save_prefs();
            return;
        }
        if target == "command:transparency" {
            self.reduce_transparency = !self.reduce_transparency;
            self.save_prefs();
            return;
        }
        if target == "command:motion" {
            self.reduce_motion = !self.reduce_motion;
            self.save_prefs();
            return;
        }
        if target == "command:mini" {
            self.mini = !self.mini;
            self.settings_open = false;
            self.save_prefs();
            return;
        }
        if target == "command:top" {
            self.on_top = !self.on_top;
            self.save_prefs();
            return;
        }
        if target == "command:scale_up" {
            self.scale = (self.scale * 1.1).min(2.0);
            self.save_prefs();
            return;
        }
        if target == "command:scale_down" {
            self.scale = (self.scale / 1.1).max(1.0);
            self.save_prefs();
            return;
        }
        if target == "command:scale_reset" {
            self.scale = 1.0;
            self.save_prefs();
            return;
        }
        if target == "command:price" {
            self.feedback = "단가는 tokenmeter price".into();
            return;
        }
        if target == "command:reset" {
            if !self.reset_arm {
                self.reset_arm = true;
                self.feedback = "다시 누르면 통계를 지웁니다".into();
                return;
            }
            let _ = std::fs::write(data_dir().join("reset.request"), b"1");
            self.reset_arm = false;
            self.feedback = "통계를 초기화했습니다".into();
            return;
        }
        if target == "command:quit" {
            self.save_prefs();
            crate::daemon::request_stop();
            return;
        }
        if target == "command:invite" {
            let rid = crate::league::current_focus();
            if rid.is_empty() {
                self.advance_league();
            } else {
                self.pending_copy = crate::league::invite_link(&rid);
                self.feedback = "초대 링크를 복사했습니다".into();
            }
            return;
        }
        if target == "command:leave" {
            let _ = crate::league::leave(None);
            return;
        }
        if target == "command:close_room" {
            let _ = crate::league::close_room(None);
            return;
        }
        if let Some(rid) = target.strip_prefix("room:") {
            crate::league::focus_room(rid);
            return;
        }
        if target == "back" {
            self.focus = None;
            return;
        }
        if let Some(name) = target.strip_prefix("panel:") {
            self.panel = name.into();
            self.scroll = 0;
            self.open_key.clear();
            if !self.rows_on {
                self.rows_on = true;
            }
            if name == "board" {
                self.advance_league();
            }
            self.save_prefs();
            return;
        }
        if let Some(name) = target.strip_prefix("filter:") {
            self.panel = "sessions".into();
            self.rows_on = true;
            self.filter = name.into();
            self.scroll = 0;
            self.open_key.clear();
            self.focus = None;
            self.save_prefs();
            return;
        }
        if target.starts_with("chip:") {
            self.panel = "quota".into();
            self.rows_on = true;
            self.save_prefs();
            return;
        }
        if let Some(name) = target.strip_prefix("rate:") {
            self.rate_span = name.into();
            self.save_prefs();
            return;
        }
        if let Some(name) = target.strip_prefix("span:") {
            if self.expanded {
                self.span = name.into();
                if self.focus.as_ref().map(|(k, _)| k.as_str()) == Some("day") {
                    self.focus = None;
                }
                self.save_prefs();
            }
            return;
        }
        if target == "act:copy" {
            if let Some(text) = copy_receipt(&snap.status, &self.open_key) {
                self.pending_copy = text;
                self.feedback = "영수증을 복사했습니다".into();
            }
            return;
        }
        if let Some(key) = target.strip_prefix("session:") {
            self.open_key = if self.open_key == key {
                String::new()
            } else {
                key.into()
            };
            self.panel = "sessions".into();
            self.rows_on = true;
            return;
        }
        if let Some(idx) = target.strip_prefix("row:").and_then(|s| s.parse::<usize>().ok()) {
            self.focus_row(idx, snap, windows);
        }
    }

    fn focus_row(&mut self, index: usize, snap: &MeterSnapshot, windows: &[Value]) {
        if self.panel == "quota" {
            if let Some(row) = windows.get(index) {
                if quota::can_represent(row) {
                    if let Some(source) = row.get("source").and_then(Value::as_str) {
                        self.quota_reps
                            .insert(source.into(), Value::String(quota::window_key(row)));
                        self.feedback = format!(
                            "{} {} · 대표 설정",
                            row.get("title").and_then(Value::as_str).unwrap_or(source),
                            row.get("label").and_then(Value::as_str).unwrap_or("")
                        );
                        self.save_prefs();
                    }
                }
            }
            return;
        }
        if self.panel == "sessions" {
            let mut rows = filter_session_rows(&snap.sessions, &self.filter);
            rows.sort_by(|a, b| session_ord(a, b));
            let cap = self.cap("sessions") as usize;
            self.scroll = self
                .scroll
                .min((rows.len() as i32 - cap as i32).max(0));
            if let Some(row) = rows.get(self.scroll as usize + index) {
                self.open_key = if self.open_key == row.key {
                    String::new()
                } else {
                    row.key.clone()
                };
                self.focus = None;
            }
            return;
        }
        if self.panel == "days" {
            if let Some((day, _, _)) = snap.days.get(index) {
                let next = ("day".into(), day.clone());
                self.focus = if self.focus.as_ref() == Some(&next) {
                    None
                } else {
                    Some(next)
                };
                if self.focus.is_some() && !self.expanded {
                    self.expanded = true;
                    self.save_prefs();
                }
            }
        }
    }
}

fn session_ord(a: &SessionView, b: &SessionView) -> std::cmp::Ordering {
    let rank = |r: &SessionView| match r.attention.as_str() {
        "check" => 0,
        "working" => 1,
        "waiting" => 2,
        _ => 3,
    };
    rank(a)
        .cmp(&rank(b))
        .then((!a.live).cmp(&(!b.live)))
        .then(
            b.started_at
                .partial_cmp(&a.started_at)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
        .then(a.key.cmp(&b.key))
}

fn display_projects(state: &Value) -> Vec<(String, i64, f64)> {
    let book = state
        .get("projects")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut scoped: HashMap<String, Vec<String>> = HashMap::new();
    for name in book.keys() {
        if let Some((_, leaf)) = name.rsplit_once('/') {
            scoped.entry(leaf.into()).or_default().push(name.clone());
        }
    }
    let mut merged: HashMap<String, (i64, f64)> = HashMap::new();
    for (name, node) in &book {
        let measured = tokens_of(node);
        if measured <= 0 {
            continue;
        }
        let target = scoped
            .get(name)
            .and_then(|v| (v.len() == 1).then(|| v[0].clone()))
            .unwrap_or_else(|| name.clone());
        let label = project_label(&target, "");
        let e = merged.entry(label).or_insert((0, 0.0));
        e.0 += measured;
        e.1 = e.1.max(node.get("last_seen").and_then(Value::as_f64).unwrap_or(0.0));
    }
    let mut rows: Vec<(String, i64, f64)> = merged
        .into_iter()
        .map(|(n, (t, s))| (n, t, s))
        .collect();
    rows.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    rows
}

fn copy_receipt(state: &Value, key: &str) -> Option<String> {
    let rec = state
        .get("sessions")
        .and_then(Value::as_object)
        .and_then(|s| s.get(key))?;
    let totals = rec.get("totals").cloned().unwrap_or(json!({}));
    let project = rec
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("(unknown)");
    let service = rec.get("service").and_then(Value::as_str).unwrap_or("");
    let model = rec.get("model").and_then(Value::as_str).unwrap_or("");
    let cost = totals.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
    let saved = totals
        .get("cache_saved_usd")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(format!(
        "### TokenMeter 영수증\n- {project} · {service} · {model}\n- 입력 {} · 캐시 읽기 {} · 캐시 쓰기 {} · 출력 {}\n- API 환산 ${cost:.2} · 캐시 절감 ${saved:.2}",
        totals.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
        totals.get("cache_read").and_then(Value::as_i64).unwrap_or(0),
        totals.get("cache_write").and_then(Value::as_i64).unwrap_or(0),
        totals.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
    ))
}

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
}

fn paint_mini(p: &mut Paint, app: &mut OverlayApp, snap: &MeterSnapshot) {
    let s = p.s();
    let full = p.ui.max_rect();
    let x = full.min.x + 6.0 * s;
    let y = full.min.y;
    let w = full.width() - 12.0 * s;
    let h = full.height();
    let rate = mini_rate_caption(snap.rate);
    let rate_w = MINI_RATE_W * s;
    p.fill(
        r(x, y + 3.0 * s, rate_w, h - 6.0 * s),
        fade(p.theme.tint, if snap.rate > 0.0 { 18 } else { 8 }),
    );
    p.text(
        r(x + 4.0 * s, y, rate_w - 8.0 * s, h),
        &rate,
        if snap.rate > 0.0 {
            p.theme.text_primary
        } else {
            p.theme.text_tertiary
        },
        9.5,
        true,
        false,
        false,
        true,
        true,
    );
    let tot = scoped(snap, app.scope_today);
    let money = money_caption(approx(&snap.status), tot.4);
    p.text(
        r(x, y, w - MODE_BTN * s, h),
        &money,
        p.theme.text_secondary,
        8.5,
        true,
        false,
        false,
        true,
        true,
    );
    let menu_x = x + w - MODE_BTN * s;
    p.text(
        r(menu_x, y, MODE_BTN * s, h),
        "⋯",
        p.theme.text_tertiary,
        10.0,
        true,
        false,
        false,
        false,
        true,
    );
    p.hit("menu", r(menu_x, y, MODE_BTN * s, h));
    let segments = SEGMENTS / 2;
    let gx0 = rate_w + 5.0 * s;
    let gx = x + gx0;
    let gw = (w - gx0 - 90.0 * s).max(10.0);
    let bar_h = 6.0 * s;
    let by = y + (h - 6.0 * s) / 2.0;
    let lit = app.gauge * segments as f64;
    for i in 0..segments {
        let on = (i as f64) < lit;
        let mut color = fade(seg_color(&p.theme, i as f32 / (segments - 1) as f32), if on { 255 } else { 26 });
        if on && (i as f64) >= lit - 1.0 {
            color = fade(hex("#FFFFFF", 255), (120.0 + 135.0 * app.pulse) as u8);
        }
        p.fill(
            r(
                gx + i as f32 * gw / segments as f32,
                by,
                gw / segments as f32 - 1.6 * s,
                bar_h,
            ),
            color,
        );
    }
}

/// v0.7.2 픽셀 미니: 창 전체가 카드이고 그림과 tok/s 만 있다. 설정은 우클릭, 나가기는 두 번 클릭.
fn paint_s_skin(p: &mut Paint, app: &OverlayApp, snap: &MeterSnapshot, alpha: u8) {
    let s = p.s();
    let full = p.ui.max_rect();
    let skin = app.widget_skin().unwrap_or("dial");
    if skin == "dial" {
        p.fill(full, hex("#080808", alpha));
        p.ui.painter().rect_stroke(
            full,
            CornerRadius::ZERO,
            Stroke::new(1.0, hex("#505050", 255)),
            egui::StrokeKind::Inside,
        );
    } else {
        paint_display_well(p, full, alpha, snap.rate, app.pulse);
    }
    paint_pixel_face(
        p,
        r(full.min.x + 12.0 * s, full.min.y + 12.0 * s, 56.0 * s, full.height() - 24.0 * s),
        skin,
        app.gauge,
        app.face_phase,
        snap.rate > 0.01 && !app.reduce_motion,
        app.money_stage,
    );
    let value = if snap.rate < 999.5 {
        format!("{:.1}", snap.rate.max(0.0))
    } else {
        mini_rate_caption(snap.rate).trim_end_matches("/s").to_string()
    };
    let muted = hex(if skin == "dial" { "#8C8C8C" } else { "#7B8497" }, 255);
    let x = full.min.x + 80.0 * s;
    p.text(
        r(x, full.min.y + 13.0 * s, 76.0 * s, 26.0 * s),
        &value,
        if snap.rate > 0.0 { hex("#F0F0F0", 255) } else { muted },
        19.0,
        false,
        false,
        false,
        false,
        true,
    );
    p.text(r(x, full.min.y + 39.0 * s, 76.0 * s, 11.0 * s), "tok/s", muted, 8.0, false, false, false, true, true);
}

/// v0.7.2 `_draw_well`: 위가 더 어두운 계기판 창. 테두리와 왼쪽 바는 유입에 맞춰 숨쉰다.
fn paint_display_well(p: &Paint, rect: Rect, alpha: u8, rate: f64, pulse: f64) {
    let (top, bottom) = (hex("#040508", alpha), hex("#0C0F16", alpha));
    let mut mesh = egui::Mesh::default();
    for (pos, c) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.left_bottom(), bottom),
        (rect.right_bottom(), bottom),
    ] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.ui.painter().add(egui::Shape::mesh(mesh));
    p.fill(r(rect.min.x, rect.max.y - 1.0, rect.width(), 1.0), fade(Color32::WHITE, 16));
    let tint = hex("#2BD9E5", if rate > 0.0 { (90.0 + 110.0 * pulse) as u8 } else { 48 });
    p.ui.painter()
        .rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, tint), egui::StrokeKind::Inside);
    p.fill(r(rect.min.x, rect.min.y, 2.0 * p.s(), rect.height()), tint);
}

/// v0.7.2 `draw_face`: 칸 경계를 기기 픽셀에서 반올림해 배율이 달라도 칸이 고르다.
/// 안티에일리어싱 없는 메시로 그려야 칸 사이가 번지지 않는다.
fn paint_pixel_face(p: &Paint, rect: Rect, skin: &str, gauge: f64, phase: f64, active: bool, stage: i32) {
    let pixels = crate::pixel_faces::cells(skin, gauge, phase, active, stage);
    let dpr = p.ui.ctx().pixels_per_point() as f64;
    let (gw, gh) = crate::pixel_faces::GRID;
    let unit = (rect.width() as f64 / gw as f64).min(rect.height() as f64 / gh as f64) * dpr;
    if pixels.is_empty() || unit <= 0.0 {
        return;
    }
    let left = pixels.keys().map(|c| c.0).min().unwrap_or(0);
    let top = pixels.keys().map(|c| c.1).min().unwrap_or(0);
    let width = (pixels.keys().map(|c| c.0).max().unwrap_or(left) - left + 1) as f64;
    let height = (pixels.keys().map(|c| c.1).max().unwrap_or(top) - top + 1) as f64;
    let ox = (rect.center().x as f64 * dpr - width * unit / 2.0).round_ties_even();
    let oy = (rect.center().y as f64 * dpr - height * unit / 2.0).round_ties_even();
    let at = |o: f64, n: i32| ((o + (n as f64 * unit).round_ties_even()) / dpr) as f32;
    let mut mesh = egui::Mesh::default();
    for ((cx, cy), key) in pixels {
        let Some(color) = crate::pixel_faces::palette_hex(skin, key) else {
            continue;
        };
        let min = Pos2::new(at(ox, cx - left), at(oy, cy - top));
        let max = Pos2::new(at(ox, cx - left + 1), at(oy, cy - top + 1));
        mesh.add_colored_rect(Rect::from_min_max(min, max), hex(color, 255));
    }
    p.ui.painter().add(egui::Shape::mesh(mesh));
}

fn scoped(snap: &MeterSnapshot, today: bool) -> (i64, i64, i64, f64, f64) {
    if today {
        (
            snap.input_tokens,
            snap.output_tokens,
            snap.cache_tokens,
            snap.cache_saved,
            snap.status
                .pointer("/today/totals/cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        )
    } else {
        (
            snap.total_input,
            snap.total_output,
            snap.total_cache,
            snap.total_saved,
            snap.status
                .pointer("/total/totals/cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        )
    }
}

fn approx(status: &Value) -> bool {
    tokens_of(
        status
            .pointer("/plans/subscription")
            .unwrap_or(&Value::Null),
    ) > 0
}

fn paint_body(
    p: &mut Paint,
    app: &mut OverlayApp,
    snap: &MeterSnapshot,
    marks: &[(String, String)],
    windows: &[Value],
    palette: &[(String, String, String, String)],
    racers: &[(String, f64, Color32, String)],
) {
    let s = p.s();
    let full = p.ui.max_rect();
    let x = full.min.x + PAD * s;
    let w = full.width() - PAD * 2.0 * s;
    let mut y = paint_meter(p, app, snap, racers, x, full.min.y + PAD * s, w);
    if app.palette_open {
        y = paint_search(p, app, x, y, w);
        paint_palette(p, app, palette, x, y, w);
        p.text(
            r(x, full.max.y - FOOT_H * s, w, FOOT_H * s),
            "↑↓ 이동 · ↵ 열기 · esc 닫기",
            p.theme.text_tertiary,
            8.0,
            false,
            false,
            true,
            false,
            false,
        );
        return;
    }
    y = paint_chips(p, marks, x, y, w);
    if app.shows_graph() {
        y = paint_graph(p, app, snap, x, y, w);
    }
    if app.rows_on {
        y = paint_rows(p, app, snap, windows, x, y, w);
    }
    let _ = y;
    let now = crate::watch::now_secs();
    let hint = app.hint_until > now;
    let sick = health_note(&snap.status, now);
    let foot = if hint {
        String::new()
    } else if !sick.is_empty() {
        sick.clone()
    } else if !app.feedback.is_empty() {
        app.feedback.clone()
    } else if app.rows_on {
        app.note.clone()
    } else {
        String::new()
    };
    let fy = full.max.y - FOOT_H * s;
    if hint {
        p.text(
            r(x, fy, w, FOOT_H * s),
            "오늘/누적 · ",
            p.theme.text_tertiary,
            7.5,
            false,
            false,
            false,
            false,
            false,
        );
        let lead = p.measure("오늘/누적 · ", 7.5, false, false);
        p.text(
            r(x + lead, fy, w - lead, FOOT_H * s),
            "S/M/L · ⋯ 메뉴",
            p.theme.tint,
            7.5,
            false,
            false,
            true,
            true,
            true,
        );
    } else {
        p.text(
            r(x, fy, w, FOOT_H * s),
            &foot,
            if !sick.is_empty() {
                p.theme.destructive
            } else {
                p.theme.text_tertiary
            },
            7.5,
            false,
            false,
            true,
            false,
            false,
        );
    }
}

fn paint_meter(
    p: &mut Paint,
    app: &mut OverlayApp,
    snap: &MeterSnapshot,
    racers: &[(String, f64, Color32, String)],
    x: f32,
    y0: f32,
    w: f32,
) -> f32 {
    let s = p.s();
    let simple = app.mode() == 'S';
    let detailed = app.expanded;
    p.text(
        r(x, y0, w, MODE_BTN * s),
        "TOKENMETER",
        p.theme.text_tertiary,
        7.5,
        false,
        false,
        false,
        true,
        true,
    );
    let bar = MODE_BTN * s * 5.0;
    let label = if app.scope_today { "오늘" } else { "누적" };
    let lw = 40.0 * s;
    let scope_x = x + w - bar - lw - SPACE_1 * s;
    p.fill(r(scope_x, y0, lw, MODE_BTN * s), fade(p.theme.tint, 24));
    p.text(
        r(scope_x, y0, lw, MODE_BTN * s),
        label,
        p.theme.tint,
        7.5,
        false,
        true,
        false,
        true,
        true,
    );
    p.hit("scope", r(scope_x, y0, lw, MODE_BTN * s));
    paint_modes(p, app, x + w - bar, y0);

    let display_y = y0 + 27.0 * s;
    let display_h = (if detailed { 37.0 } else { 43.0 }) * s;
    let pulse_a = if snap.rate > 0.0 {
        (90.0 + 110.0 * app.pulse) as u8
    } else {
        42
    };
    p.fill(
        r(x, display_y, w, display_h),
        p.theme.background_primary,
    );
    p.ui.painter().rect_stroke(
        r(x + 0.5 * s, display_y + 0.5 * s, w - s, display_h - s),
        CornerRadius::ZERO,
        Stroke::new(1.0, fade(p.theme.tint, pulse_a)),
        egui::StrokeKind::Inside,
    );
    p.fill(r(x, display_y, 2.0 * s, display_h), fade(p.theme.tint, pulse_a));
    p.text(
        r(x + 8.0 * s, display_y + 3.0 * s, w - 16.0 * s, 10.0 * s),
        "전체 출력",
        fade(p.theme.tint, 190),
        7.0,
        false,
        false,
        true,
        true,
        true,
    );
    let unit_w = 34.0 * s;
    p.text(
        r(
            x + 8.0 * s,
            display_y + (if detailed { 7.0 } else { 9.0 }) * s,
            w - 16.0 * s - unit_w,
            (if detailed { 26.0 } else { 31.0 }) * s,
        ),
        &comma_rate(snap.rate),
        if snap.rate > 0.0 {
            p.theme.text_primary
        } else {
            p.theme.text_tertiary
        },
        if detailed { 21.5 } else { 25.5 },
        true,
        false,
        false,
        true,
        true,
    );
    p.text(
        r(
            x + w - unit_w - 4.0 * s,
            display_y + (if detailed { 16.0 } else { 20.0 }) * s,
            unit_w,
            14.0 * s,
        ),
        "tok/s",
        p.theme.text_tertiary,
        7.0,
        true,
        false,
        false,
        true,
        true,
    );

    let gy = y0 + (if detailed { 68.0 } else { 76.0 }) * s;
    let gw = w / SEGMENTS as f32;
    let bar_h = (if detailed { 8.0 } else { 11.0 }) * s;
    let lit = app.gauge * SEGMENTS as f64;
    for i in 0..SEGMENTS {
        let on = (i as f64) < lit;
        let mut color = fade(
            seg_color(&p.theme, i as f32 / (SEGMENTS - 1) as f32),
            if on { 255 } else { 26 },
        );
        if on && (i as f64) >= lit - 1.0 {
            color = fade(hex("#FFFFFF", 255), (120.0 + 135.0 * app.pulse) as u8);
        }
        p.fill(
            r(x + i as f32 * gw, gy, gw - 1.6 * s, bar_h),
            color,
        );
    }
    if app.peak > 0.01 {
        let px = x + (app.peak as f32).clamp(0.0, 1.0) * w - 1.5 * s;
        p.fill(
            r(px, gy - 2.0 * s, 1.6 * s, bar_h + 4.0 * s),
            fade(p.theme.text_primary, 150),
        );
    }
    for (_, _, color, uid) in racers {
        let frac = *app
            .league_marks
            .get(uid)
            .unwrap_or(&gauge_target(0.0, DEFAULT_FULL_SCALE));
        let thick = 2.0 * s;
        p.fill(
            r(x + frac as f32 * w - thick / 2.0, gy - 2.0 * s, thick, bar_h + 4.0 * s),
            *color,
        );
    }
    if simple {
        return y0 + METER_H_S * s;
    }
    if !detailed {
        p.text(
            r(x, y0 + 89.0 * s, w, 10.0 * s),
            "출력 흐름",
            fade(p.theme.text_tertiary, 170),
            6.5,
            false,
            false,
            false,
            true,
            true,
        );
        p.text(
            r(x, y0 + 89.0 * s, w, 10.0 * s),
            &format!("상한 {} tok/s", compact_num(snap.full_scale)),
            fade(p.theme.text_tertiary, 170),
            6.5,
            true,
            false,
            false,
            true,
            true,
        );
    }
    let y = y0 + (if detailed { 83.0 } else { 104.0 }) * s;
    let tot = scoped(snap, app.scope_today);
    let cw = w / 4.0;
    for (i, (mark, val, kind)) in [
        ("입력", compact_num(tot.0 as f64), "input"),
        ("출력", compact_num(tot.1 as f64), "output"),
        ("캐시", compact_num(tot.2 as f64), "cache_write"),
        ("절감", money_short(tot.3), "input"),
    ]
    .iter()
    .enumerate()
    {
        let cell_x = x + cw * i as f32;
        let label = format!("{mark} ");
        p.text(
            r(cell_x, y, cw - 3.0 * s, 15.0 * s),
            &label,
            p.theme.text_tertiary,
            7.5,
            false,
            false,
            false,
            false,
            false,
        );
        let lw = p.measure(&label, 7.5, false, false);
        p.text(
            r(cell_x + lw, y, cw - 3.0 * s - lw, 15.0 * s),
            val,
            if i < 3 {
                kind_color(&p.theme, kind)
            } else {
                p.theme.text_secondary
            },
            7.5,
            false,
            false,
            true,
            true,
            true,
        );
    }
    y0 + (if detailed { METER_H_L } else { METER_H }) * s
}

fn paint_modes(p: &mut Paint, app: &OverlayApp, x: f32, y: f32) {
    let s = p.s();
    let bw = MODE_BTN * s;
    let h = MODE_BTN * s;
    let current = app.mode();
    for (i, name) in ['S', 'M', 'L'].iter().enumerate() {
        let bx = x + i as f32 * bw;
        let on = *name == current;
        p.fill(
            r(bx, y, bw - 2.0 * s, h),
            fade(
                if on { p.theme.tint } else { p.theme.surface_hover },
                if on { 30 } else { 12 },
            ),
        );
        p.text(
            r(bx, y, bw - 2.0 * s, h),
            &name.to_string(),
            if on { p.theme.tint } else { p.theme.text_tertiary },
            7.5,
            false,
            true,
            false,
            true,
            true,
        );
        p.hit(format!("mode:{name}"), r(bx, y, bw, h));
    }
    let mut bx = x + 3.0 * bw;
    p.text(r(bx, y, bw, h), "⋯", p.theme.text_tertiary, 10.0, false, true, false, false, true);
    p.hit("menu", r(bx, y, bw, h));
    bx += bw;
    p.text(r(bx, y, bw, h), "×", p.theme.text_tertiary, 9.0, false, true, false, false, true);
    p.hit("close", r(bx, y, bw, h));
}

fn paint_chips(p: &mut Paint, marks: &[(String, String)], x: f32, y: f32, w: f32) -> f32 {
    if marks.is_empty() {
        return y;
    }
    let s = p.s();
    let cw = w / marks.len() as f32;
    for (i, (text, status)) in marks.iter().enumerate() {
        let color = if status == "exhausted" {
            p.theme.destructive
        } else if status == "warn" || status == "stale" {
            p.theme.warning
        } else if status == "underused" {
            p.theme.success
        } else {
            p.theme.text_tertiary
        };
        p.text(
            r(x + i as f32 * cw, y, cw, CHIP_H * s),
            text,
            color,
            7.5,
            false,
            true,
            true,
            true,
            true,
        );
        p.hit(format!("chip:{i}"), r(x + i as f32 * cw, y, cw, CHIP_H * s));
    }
    y + CHIP_H * s
}

fn paint_search(p: &mut Paint, app: &mut OverlayApp, x: f32, y: f32, w: f32) -> f32 {
    let s = p.s();
    p.fill(r(x, y, w, SEARCH_H * s), fade(p.theme.tint, 18));
    p.hit("search", r(x, y, w, SEARCH_H * s));
    let resp = p.ui.put(
        r(x + 10.0 * s, y, w - 20.0 * s, SEARCH_H * s),
        egui::TextEdit::singleline(&mut app.palette_query)
            .hint_text("세션 또는 명령 · ⌘K")
            .font(p.font(12.0, false, false))
            .text_color(p.theme.text_primary),
    );
    resp.request_focus();
    y + SEARCH_H * s + 12.0 * s
}

fn paint_palette(
    p: &mut Paint,
    app: &OverlayApp,
    items: &[(String, String, String, String)],
    x: f32,
    y: f32,
    w: f32,
) {
    let s = p.s();
    if items.is_empty() {
        p.text(
            r(x, y, w, PALETTE_ROW_H * 2.0 * s),
            "일치하는 세션이나 명령이 없습니다",
            p.theme.text_secondary,
            9.0,
            false,
            false,
            false,
            false,
            false,
        );
        return;
    }
    for (i, item) in items.iter().enumerate() {
        let ry = y + i as f32 * PALETTE_ROW_H * s;
        let h = (PALETTE_ROW_H - 3.0) * s;
        let selected = i == app.palette_index;
        if selected {
            p.fill(r(x, ry, w, h), fade(p.theme.tint, 24));
            p.fill(r(x, ry + 9.0 * s, 2.0 * s, h - 18.0 * s), fade(p.theme.tint, 180));
        }
        p.text(
            r(x + 14.0 * s, ry + 3.0 * s, w - 92.0 * s, 20.0 * s),
            &item.1,
            p.theme.text_primary,
            9.5,
            false,
            false,
            true,
            false,
            true,
        );
        p.text(
            r(x + 14.0 * s, ry + 21.0 * s, w - 92.0 * s, 18.0 * s),
            &item.2,
            p.theme.text_secondary,
            7.5,
            false,
            false,
            true,
            false,
            false,
        );
        p.text(
            r(x, ry, w - 14.0 * s, h),
            &item.3,
            if selected { p.theme.tint } else { p.theme.text_tertiary },
            7.5,
            true,
            false,
            false,
            false,
            true,
        );
        p.hit(format!("palette:{i}"), r(x, ry, w, h));
    }
}

fn current_series(app: &OverlayApp, snap: &MeterSnapshot) -> Series {
    let hours = load_hours(Some(&snap.status));
    match app.focus.as_ref() {
        Some((k, v)) if k == "day" => series(&hours, "today", None, day_noon(v)),
        Some((k, v)) if k == "project" => {
            let key = if v == "폴더 미상" { "(unknown)" } else { v.as_str() };
            series(&hours, &app.span, Some(key), crate::watch::now_secs())
        }
        _ => series(&hours, &app.span, None, crate::watch::now_secs()),
    }
}

fn current_rates(app: &OverlayApp, snap: &MeterSnapshot) -> RateSeries {
    rate_series(
        &load_rates(Some(&snap.status)),
        &app.rate_span,
        crate::watch::now_secs(),
    )
}

fn paint_graph(p: &mut Paint, app: &mut OverlayApp, snap: &MeterSnapshot, x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    p.fill(
        r(x, y0 - 4.0 * s, w, 1.0),
        fade(p.theme.separator, p.theme.line_alpha),
    );
    let mut y = y0 + GRAPH_TOP * s;
    if app.panel == "rates" {
        return paint_rate_graph(p, app, snap, x, y, w);
    }
    let data = current_series(app, snap);
    let span_w = BTN_W * 3.0 * s;
    let (kind, value) = app
        .focus
        .as_ref()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .unwrap_or(("", ""));
    let title = if kind == "project" || kind == "day" {
        format!("‹ 전체 · {value}")
    } else {
        "시간별 API 환산 비용 · USD".into()
    };
    p.text(
        r(x, y, w * 0.46, HEAD_H * s),
        &title,
        if !kind.is_empty() {
            p.theme.tint
        } else {
            p.theme.text_tertiary
        },
        8.0,
        false,
        false,
        true,
        false,
        true,
    );
    if !kind.is_empty() {
        p.hit("back", r(x, y, w * 0.46, HEAD_H * s));
    }
    p.text(
        r(x + w * 0.46, y, (w * 0.54 - span_w - 6.0 * s).max(0.0), HEAD_H * s),
        &summary(&data),
        p.theme.text_tertiary,
        7.5,
        true,
        false,
        true,
        true,
        false,
    );
    paint_spans(p, app, x + w - span_w, y, kind == "day");
    y += HEAD_H * s + 3.0 * s;
    let bh = GRAPH_H * s - GRAPH_TOP * s - HEAD_H * s - GRAPH_AXIS * s;
    if data.peak <= 0.0 {
        p.text(
            r(x, y, w, bh),
            "아직 쌓인 시간이 없습니다 — 한 시간이 지나면 그려집니다",
            fade(p.theme.text_tertiary, 150),
            8.0,
            false,
            false,
            true,
            false,
            false,
        );
        return y + bh + 11.0 * s;
    }
    let usable = (bh - GRAPH_PEAK * s).max(8.0 * s);
    let bw = w / data.bars.len().max(1) as f32;
    let tallest = data
        .bars
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total.partial_cmp(&b.1.total).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    for (i, bar) in data.bars.iter().enumerate() {
        let bx = x + i as f32 * bw;
        if bar.total <= 0.0 {
            p.fill(
                r(bx, y + bh - 1.0 * s, bw - GRAPH_GAP * s, 1.0 * s),
                fade(p.theme.separator, p.theme.line_alpha),
            );
            continue;
        }
        let part = usable * (bar.total / data.peak) as f32;
        let cy = y + bh - part;
        p.fill(r(bx, cy, bw - GRAPH_GAP * s, part), fade(p.theme.tint, 190));
        if i == tallest {
            let label = money_short(bar.total);
            let lw = p.measure(&label, 7.0, true, false);
            let lx = (bx + (bw - lw) / 2.0).clamp(x, x + (w - lw).max(0.0));
            p.text(r(lx, cy - 10.0 * s, lw, 10.0 * s), &label, p.theme.text_primary, 7.0, false, false, false, true, false);
        }
    }
    y += bh + 1.0 * s;
    let step = (data.bars.len() / 8).max(1);
    for (i, bar) in data.bars.iter().enumerate() {
        if i % step == 0 {
            p.text(
                r(x + i as f32 * bw, y, bw * 2.0, 9.0 * s),
                &bar.label,
                fade(p.theme.text_tertiary, 140),
                7.0,
                false,
                false,
                false,
                true,
                false,
            );
        }
    }
    y + 10.0 * s
}

fn paint_rate_graph(p: &mut Paint, app: &OverlayApp, snap: &MeterSnapshot, x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    let data = current_rates(app, snap);
    let span_w = RATE_BTN_W * 4.0 * s;
    let tok = format!("{} 토큰", compact_num(data.total_tokens as f64));
    let tok_w = p.measure(&tok, 7.5, true, false).min((w * 0.22).max(48.0 * s)) + 8.0 * s;
    let title_w = (w - span_w - tok_w - 8.0 * s).max(0.0);
    let mut y = y0;
    p.text(
        r(x, y, title_w, HEAD_H * s),
        "메인 모델 출력 처리량 · tok/s",
        p.theme.text_tertiary,
        8.0,
        false,
        false,
        true,
        false,
        true,
    );
    p.text(
        r(x + title_w, y, tok_w, HEAD_H * s),
        &tok,
        p.theme.text_tertiary,
        7.5,
        true,
        false,
        true,
        true,
        false,
    );
    paint_rate_spans(p, app, x + w - span_w, y);
    y += HEAD_H * s + 3.0 * s;
    let bh = GRAPH_H * s - GRAPH_TOP * s - HEAD_H * s - GRAPH_AXIS * s;
    if data.peak <= 0.0 {
        p.text(
            r(x, y, w, bh),
            "작업이 이어지면 속도가 쌓입니다",
            fade(p.theme.text_tertiary, 150),
            8.0,
            false,
            false,
            true,
            false,
            false,
        );
        return y + bh + 11.0 * s;
    }
    let usable = (bh - GRAPH_PEAK * s).max(8.0 * s);
    let bw = w / data.bars.len().max(1) as f32;
    let tallest = data
        .bars
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.rate.partial_cmp(&b.1.rate).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    for (i, bar) in data.bars.iter().enumerate() {
        let bx = x + i as f32 * bw;
        if bar.rate <= 0.0 {
            p.fill(
                r(bx, y + bh - 1.0 * s, bw - GRAPH_GAP * s, 1.0 * s),
                fade(p.theme.separator, p.theme.line_alpha),
            );
            continue;
        }
        let part = usable * (bar.rate / data.peak) as f32;
        p.fill(
            r(bx, y + bh - part, bw - GRAPH_GAP * s, part),
            fade(kind_color(&p.theme, "output"), 200),
        );
        if i == tallest {
            let label = format!("{:.0}", bar.rate);
            let lw = p.measure(&label, 7.0, true, false);
            let lx = (bx + (bw - lw) / 2.0).clamp(x, x + (w - lw).max(0.0));
            p.text(
                r(lx, y + bh - part - 10.0 * s, lw, 10.0 * s),
                &label,
                p.theme.text_primary,
                7.0,
                false,
                false,
                false,
                true,
                false,
            );
        }
    }
    y += bh + 1.0 * s;
    let step = (data.bars.len() / 8).max(1);
    for (i, bar) in data.bars.iter().enumerate() {
        if i % step == 0 {
            p.text(
                r(x + i as f32 * bw, y, bw * 2.0, 9.0 * s),
                &bar.label,
                fade(p.theme.text_tertiary, 140),
                7.0,
                false,
                false,
                false,
                true,
                false,
            );
        }
    }
    y + 10.0 * s
}

fn paint_spans(p: &mut Paint, app: &OverlayApp, x: f32, y: f32, disabled: bool) {
    let s = p.s();
    for (i, name) in SPANS.iter().enumerate() {
        let bx = x + i as f32 * BTN_W * s;
        let on = !disabled && app.span == *name;
        p.fill(
            r(bx, y, BTN_W * s - 2.0 * s, BTN_H * s),
            fade(
                if on { p.theme.tint } else { p.theme.surface_hover },
                if on { 30 } else { 12 },
            ),
        );
        p.text(
            r(bx, y, BTN_W * s - 2.0 * s, BTN_H * s),
            span_title(name),
            fade(
                if on { p.theme.tint } else { p.theme.text_tertiary },
                if disabled { 90 } else { 255 },
            ),
            8.0,
            false,
            true,
            false,
            false,
            true,
        );
        p.hit(format!("span:{name}"), r(bx, y, BTN_W * s - 2.0 * s, BTN_H * s));
    }
}

fn paint_rate_spans(p: &mut Paint, app: &OverlayApp, x: f32, y: f32) {
    let s = p.s();
    for (i, name) in RATE_SPANS.iter().enumerate() {
        let bx = x + i as f32 * RATE_BTN_W * s;
        let on = app.rate_span == *name;
        p.fill(
            r(bx, y, RATE_BTN_W * s - 2.0 * s, BTN_H * s),
            fade(
                if on { p.theme.tint } else { p.theme.surface_hover },
                if on { 30 } else { 12 },
            ),
        );
        p.text(
            r(bx, y, RATE_BTN_W * s - 2.0 * s, BTN_H * s),
            rate_span_title(name),
            if on { p.theme.tint } else { p.theme.text_tertiary },
            8.0,
            false,
            true,
            false,
            false,
            true,
        );
        p.hit(format!("rate:{name}"), r(bx, y, RATE_BTN_W * s - 2.0 * s, BTN_H * s));
    }
}

fn paint_rows(
    p: &mut Paint,
    app: &mut OverlayApp,
    snap: &MeterSnapshot,
    windows: &[Value],
    x: f32,
    y0: f32,
    w: f32,
) -> f32 {
    let s = p.s();
    p.fill(r(x, y0 - 4.0 * s, w, 1.0), fade(p.theme.separator, p.theme.line_alpha));
    let panels = app.visible_panels();
    let tab_w = w / panels.len().max(1) as f32;
    let mut y = y0;
    for (i, name) in panels.iter().enumerate() {
        let bx = x + i as f32 * tab_w;
        let on = app.panel == *name;
        let title = panel_title(name);
        p.text(
            r(bx, y, tab_w, HEAD_H * s),
            title,
            if on { p.theme.tint } else { p.theme.text_tertiary },
            8.0,
            false,
            true,
            true,
            false,
            true,
        );
        if on {
            let mark_w = (p.measure(title, 8.0, false, true) + SPACE_2 * s).min(tab_w);
            p.fill(
                r(bx + (tab_w - mark_w) / 2.0, y + (HEAD_H - 2.0) * s, mark_w, 2.0 * s),
                p.theme.tint,
            );
        }
        p.hit(format!("panel:{name}"), r(bx, y, tab_w, HEAD_H * s));
    }
    y += (HEAD_H + SPACE_1) * s;
    match app.panel.as_str() {
        "sessions" => paint_sessions(p, app, snap, x, y, w),
        "projects" => paint_projects(p, app, snap, x, y, w),
        "rates" => paint_rate_rows(p, app, snap, x, y, w),
        "quota" => paint_quota(p, app, windows, x, y, w),
        "board" => paint_board(p, app, x, y, w),
        _ => paint_days(p, app, snap, x, y, w),
    }
}

fn empty_kind(app: &OverlayApp) -> String {
    if app.panel == "sessions" {
        format!("sessions.{}", app.filter)
    } else if app.panel == "board" {
        if !crate::league::enabled() {
            "board.empty".into()
        } else if !crate::league::has_auth() {
            "board.login".into()
        } else if crate::league::list_rooms().is_empty() {
            "board.open".into()
        } else {
            "board.empty".into()
        }
    } else {
        app.panel.clone()
    }
}

fn paint_empty(p: &mut Paint, app: &OverlayApp, x: f32, y: f32, w: f32) -> f32 {
    let s = p.s();
    p.text(
        r(x, y, w, ROW_H * s),
        &empty_loop(&empty_kind(app), crate::watch::now_secs()),
        fade(p.theme.text_tertiary, 180),
        8.0,
        false,
        false,
        true,
        false,
        false,
    );
    y + ROW_H * s + 6.0 * s
}

fn paint_sessions(
    p: &mut Paint,
    app: &mut OverlayApp,
    snap: &MeterSnapshot,
    x: f32,
    y0: f32,
    w: f32,
) -> f32 {
    let s = p.s();
    let mut rows = filter_session_rows(&snap.sessions, &app.filter);
    rows.sort_by(|a, b| session_ord(a, b));
    let empty = rows.is_empty();
    let mut y = y0;
    if app.shows_session_filters(empty) {
        let width = 58.0 * s;
        for (i, name) in SESSION_FILTERS.iter().enumerate() {
            let bx = x + i as f32 * width;
            let on = app.filter == *name;
            p.fill(
                r(bx, y, width - 3.0 * s, FILTER_H * s),
                fade(
                    if on { p.theme.tint } else { p.theme.surface_hover },
                    if on { 28 } else { 10 },
                ),
            );
            p.text(
                r(bx, y, width - 3.0 * s, FILTER_H * s),
                filter_title(name),
                if on { p.theme.tint } else { p.theme.text_tertiary },
                8.0,
                false,
                true,
                false,
                false,
                true,
            );
            p.hit(format!("filter:{name}"), r(bx, y, width - 3.0 * s, FILTER_H * s));
        }
        y += (FILTER_H + SPACE_1) * s;
    }
    let wide = app.expanded;
    let heads: &[&str] = if wide {
        &["상태", "프로젝트", "모델", "메인", "누적", "컨텍스트", "시각"]
    } else {
        &["상태", "프로젝트", "메인", "누적", "컨텍스트"]
    };
    let cols: &[(f32, f32, bool, f32)] = if wide {
        &[
            (0.00, 0.10, false, 8.0),
            (0.10, 0.32, false, 9.0),
            (0.32, 0.54, false, 8.0),
            (0.54, 0.64, true, 8.5),
            (0.64, 0.74, true, 8.0),
            (0.74, 0.87, true, 8.0),
            (0.87, 1.00, true, 7.5),
        ]
    } else {
        &[
            (0.00, 0.16, false, 8.0),
            (0.16, 0.50, false, 9.0),
            (0.50, 0.65, true, 8.5),
            (0.65, 0.81, true, 8.0),
            (0.81, 1.00, true, 7.5),
        ]
    };
    for (name, (start, end, right, _)) in heads.iter().zip(cols.iter()) {
        p.text(
            r(x + start * w + CELL_PAD * s, y, (end - start) * w - CELL_PAD * s, COLHEAD_H * s),
            name,
            fade(p.theme.text_tertiary, 180),
            7.5,
            *right,
            false,
            false,
            false,
            true,
        );
    }
    y += COLHEAD_H * s;
    let cap = app.cap("sessions") as usize;
    app.scroll = app.scroll.min((rows.len() as i32 - cap as i32).max(0));
    let shown: Vec<&SessionView> = rows
        .iter()
        .copied()
        .skip(app.scroll as usize)
        .take(cap)
        .collect();
    if shown.is_empty() {
        app.note = match app.filter.as_str() {
            "live" => "실시간 세션 없음 — 실행 중인 에이전트가 여기에 표시됩니다",
            "archive" => "보관 세션 없음 — 종료된 에이전트가 여기에 표시됩니다",
            _ => "기록된 세션 없음 — 세션은 에이전트 대화 하나입니다",
        }
        .into();
        return paint_empty(p, app, x, y, w);
    }
    app.note = format!(
        "세션 {}개 · 라이브 {}개{}",
        rows.len(),
        snap.status.get("live_count").and_then(Value::as_i64).unwrap_or(0),
        if rows.len() > cap {
            format!(" · {}/{}", app.scroll as usize + shown.len(), rows.len())
        } else {
            String::new()
        }
    );
    let top = shown
        .iter()
        .map(|row| snap.session_rates.get(&row.key).copied().unwrap_or(0.0))
        .fold(0.0, f64::max)
        .max(1.0);
    let focused = app
        .focus
        .as_ref()
        .filter(|(k, _)| k == "project")
        .map(|(_, v)| v.clone());
    for (i, row) in shown.iter().enumerate() {
        let ry = y + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        let rate = snap.session_rates.get(&row.key).copied().unwrap_or(0.0);
        let ctx = if row.ctx_win > 0 {
            row.ctx as f64 / row.ctx_win as f64
        } else {
            0.0
        };
        let sc = state_color(&p.theme, &row.attention);
        if focused.as_deref() == Some(&project_label(&row.project, &row.cwd)) || app.open_key == row.key
        {
            p.fill(r(x, ry, w, h), fade(p.theme.tint, 20));
        }
        p.fill(r(x, ry, w * cols[0].1, h), fade(sc, 18));
        p.fill(r(x, ry, 2.0 * s, h), sc);
        let rate_col = if wide { cols[3] } else { cols[2] };
        if rate >= 0.01 {
            p.fill(
                r(
                    x + rate_col.0 * w,
                    ry + h - 2.0 * s,
                    (rate_col.1 - rate_col.0) * w * (rate / top).clamp(0.0, 1.0) as f32,
                    2.0 * s,
                ),
                p.theme.tint,
            );
        }
        let ctx_col = if wide { cols[5] } else { cols[4] };
        if row.ctx_win > 0 {
            p.fill(
                r(
                    x + ctx_col.0 * w,
                    ry + h - 2.0 * s,
                    (ctx_col.1 - ctx_col.0) * w * ctx.clamp(0.0, 1.0) as f32,
                    2.0 * s,
                ),
                ctx_color(&p.theme, ctx),
            );
        }
        p.hit(format!("row:{i}"), r(x, ry, w, ROW_H * s));
        let speed = if rate >= 0.01 {
            let t = rate_text(rate);
            if t.is_empty() {
                "—".into()
            } else {
                t
            }
        } else {
            "—".into()
        };
        let context = ctx_status_caption(ctx, row.ctx_win > 0);
        let project = project_label(&row.project, &row.cwd);
        let effort = short_effort(&row.effort);
        let engine = if wide {
            if effort.is_empty() {
                short_model(&row.model)
            } else {
                format!("{} · {effort}", short_model(&row.model))
            }
        } else {
            String::new()
        };
        let cells: Vec<(String, Color32, bool)> = if wide {
            vec![
                (attention_label(&row.attention).into(), sc, true),
                (
                    project,
                    if row.live {
                        p.theme.text_primary
                    } else {
                        p.theme.text_secondary
                    },
                    row.live,
                ),
                (engine, p.theme.text_secondary, false),
                (
                    speed,
                    if rate >= 0.01 {
                        p.theme.tint
                    } else {
                        p.theme.text_tertiary
                    },
                    rate >= 0.01,
                ),
                (compact_num(row.total_tokens as f64), p.theme.text_tertiary, false),
                (
                    context,
                    if row.ctx_win > 0 {
                        ctx_color(&p.theme, ctx)
                    } else {
                        p.theme.text_tertiary
                    },
                    ctx >= CTX_WARN,
                ),
                (stamp(row.started_at), p.theme.text_tertiary, false),
            ]
        } else {
            vec![
                (attention_label(&row.attention).into(), sc, true),
                (
                    project,
                    if row.live {
                        p.theme.text_primary
                    } else {
                        p.theme.text_secondary
                    },
                    row.live,
                ),
                (
                    speed,
                    if rate >= 0.01 {
                        p.theme.tint
                    } else {
                        p.theme.text_tertiary
                    },
                    rate >= 0.01,
                ),
                (compact_num(row.total_tokens as f64), p.theme.text_tertiary, false),
                (
                    context,
                    if row.ctx_win > 0 {
                        ctx_color(&p.theme, ctx)
                    } else {
                        p.theme.text_tertiary
                    },
                    ctx >= CTX_WARN,
                ),
            ]
        };
        for (index, ((text, color, bold), (start, end, right, size))) in cells.iter().zip(cols.iter()).enumerate()
        {
            if text.is_empty() {
                continue;
            }
            p.text(
                r(x + start * w + CELL_PAD * s, ry, (end - start) * w - CELL_PAD * s, h),
                text,
                *color,
                *size,
                *right,
                false,
                true,
                index >= 2,
                *bold,
            );
        }
    }
    let mut bottom = y + shown.len() as f32 * ROW_H * s + 6.0 * s;
    if !app.open_key.is_empty() {
        bottom = paint_session_detail(p, app, snap, x, bottom, w);
    }
    bottom
}

fn paint_session_detail(
    p: &mut Paint,
    app: &OverlayApp,
    snap: &MeterSnapshot,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    let Some(rec) = snap.sessions.iter().find(|r| r.key == app.open_key) else {
        return y;
    };
    let s = p.s();
    let h = (DETAIL_H - 4.0) * s;
    p.fill(r(x, y, w, h), fade(p.theme.text_primary, 12));
    let now = crate::watch::now_secs();
    let why = check_reason(&rec.event);
    let waited = wait_caption(now, if rec.attention_at > 0.0 { rec.attention_at } else { rec.last_seen });
    let sub = if rec.cost_usd > 0.0 {
        rec.sub_cost / rec.cost_usd
    } else {
        0.0
    };
    let bits: Vec<String> = [
        why.to_string(),
        if rec.attention == "check" && !waited.is_empty() {
            format!("확인 {waited}")
        } else {
            String::new()
        },
        if rec.cost_usd > 0.0 {
            if approx(&snap.status) {
                format!("API 환산 ${:.2}", rec.cost_usd)
            } else {
                format!("비용 ${:.2}", rec.cost_usd)
            }
        } else {
            String::new()
        },
        short_model(&rec.model),
        rec.provider.clone(),
        if sub > 0.0 {
            format!("서브 {:.0}%", sub * 100.0)
        } else {
            String::new()
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    p.text(
        r(x + 6.0 * s, y, w - 12.0 * s, h * 0.5),
        &if bits.is_empty() {
            "세션 상세".into()
        } else {
            bits.join(" · ")
        },
        p.theme.text_primary,
        8.0,
        false,
        false,
        true,
        false,
        true,
    );
    p.text(
        r(x + 6.0 * s, y + h * 0.5, 56.0 * s, h * 0.5),
        "영수증",
        p.theme.tint,
        8.0,
        false,
        false,
        false,
        false,
        true,
    );
    p.hit("act:copy", r(x + 6.0 * s, y + h * 0.5, 56.0 * s, h * 0.5));
    y + DETAIL_H * s
}

fn paint_projects(p: &mut Paint, app: &mut OverlayApp, snap: &MeterSnapshot, x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    p.text(r(x, y0, w * 0.54, COLHEAD_H * s), "프로젝트", fade(p.theme.text_tertiary, 180), 7.5, false, false, false, false, true);
    p.text(r(x + w * 0.54, y0, w * 0.23, COLHEAD_H * s), "최근 세션", fade(p.theme.text_tertiary, 180), 7.5, false, false, false, false, true);
    p.text(r(x + w * 0.77, y0, w * 0.23 - 6.0 * s, COLHEAD_H * s), "누적 토큰", fade(p.theme.text_tertiary, 180), 7.5, true, false, false, false, true);
    let mut y = y0 + COLHEAD_H * s;
    let rows = display_projects(&snap.status);
    if rows.is_empty() {
        app.note.clear();
        return paint_empty(p, app, x, y, w);
    }
    let cap = app.cap("projects") as usize;
    app.scroll = app.scroll.min((rows.len() as i32 - cap as i32).max(0));
    let total: i64 = rows.iter().map(|r| r.1).sum();
    let shown: Vec<(String, i64, f64)> = rows
        .iter()
        .cloned()
        .skip(app.scroll as usize)
        .take(cap)
        .collect();
    app.note = format!(
        "프로젝트 {}개 · 누적 측정 {} 토큰",
        rows.len(),
        compact_num(total as f64)
    );
    let top = shown.iter().map(|r| r.1).max().unwrap_or(1).max(1);
    for (i, (project, tokens, last)) in shown.iter().enumerate() {
        let ry = y + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        p.fill(r(x, ry, w * (*tokens as f32 / top as f32), h), fade(p.theme.tint, 14));
        if i == 0 {
            p.fill(r(x, ry, 2.0 * s, h), p.theme.tint);
        }
        p.text(
            r(x + 6.0 * s, ry, w * 0.54 - 12.0 * s, h),
            project,
            if i == 0 { p.theme.text_primary } else { p.theme.text_secondary },
            9.0,
            false,
            false,
            true,
            false,
            i == 0,
        );
        p.text(r(x + w * 0.54, ry, w * 0.23, h), &stamp(*last), p.theme.text_tertiary, 7.5, false, false, false, true, false);
        p.text(
            r(x + w * 0.77, ry, w * 0.23 - 6.0 * s, h),
            &compact_num(*tokens as f64),
            p.theme.tint,
            9.0,
            true,
            false,
            false,
            true,
            true,
        );
    }
    y + shown.len().max(1) as f32 * ROW_H * s + 6.0 * s
}

fn paint_rate_rows(p: &mut Paint, app: &mut OverlayApp, snap: &MeterSnapshot, x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    let mut y = y0;
    if !app.expanded {
        paint_rate_spans(p, app, x + w - RATE_BTN_W * 4.0 * s, y);
        y += BTN_H * s + 2.0 * s;
    }
    p.text(r(x, y, w * 0.34, COLHEAD_H * s), "프로바이더", fade(p.theme.text_tertiary, 180), 7.5, false, false, false, false, true);
    p.text(r(x + w * 0.34, y, w * 0.30, COLHEAD_H * s), "메인 모델", fade(p.theme.text_tertiary, 180), 7.5, false, false, false, false, true);
    p.text(r(x + w * 0.64, y, w * 0.18 - 4.0 * s, COLHEAD_H * s), "누적", fade(p.theme.text_tertiary, 180), 7.5, true, false, false, false, true);
    p.text(r(x + w * 0.82, y, w * 0.18 - 6.0 * s, COLHEAD_H * s), "tok/s", fade(p.theme.text_tertiary, 180), 7.5, true, false, false, false, true);
    y += COLHEAD_H * s;
    let data = current_rates(app, snap);
    app.note = rate_summary(&data);
    let rows: Vec<_> = data.rows.into_iter().take(app.cap("rates") as usize).collect();
    if rows.is_empty() {
        return paint_empty(p, app, x, y, w);
    }
    let top = rows.iter().map(|r| r.rate).fold(0.0, f64::max).max(1.0);
    for (i, row) in rows.iter().enumerate() {
        let ry = y + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        p.fill(
            r(
                x + w * 0.82,
                ry + h - 2.0 * s,
                w * 0.18 * (row.rate / top).clamp(0.0, 1.0) as f32,
                2.0 * s,
            ),
            p.theme.tint,
        );
        p.text(r(x + 6.0 * s, ry, w * 0.34 - 6.0 * s, h), &row.vendor, p.theme.text_primary, 8.5, false, false, true, false, true);
        p.text(r(x + w * 0.34, ry, w * 0.30, h), &short_model(&row.model), p.theme.text_secondary, 8.5, false, false, true, true, false);
        p.text(r(x + w * 0.64, ry, w * 0.18 - 4.0 * s, h), &compact_num(row.tokens as f64), p.theme.text_tertiary, 8.5, true, false, false, true, false);
        p.text(r(x + w * 0.82, ry, w * 0.18 - 6.0 * s, h), &format!("{:.1}", row.rate), p.theme.tint, 9.0, true, false, false, true, true);
    }
    y + rows.len().max(1) as f32 * ROW_H * s + 6.0 * s
}

fn paint_quota(p: &mut Paint, app: &mut OverlayApp, windows: &[Value], x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    let rows = quota::panel_rows(windows, None);
    let heads = ["서비스", "기간", "사용", "페이스", "리셋"];
    let cols = [
        (0.00, 0.27, false, 8.5),
        (0.27, 0.55, false, 8.5),
        (0.55, 0.67, true, 8.5),
        (0.67, 0.87, true, 8.5),
        (0.87, 1.00, true, 8.5),
    ];
    for (name, (start, end, right, _)) in heads.iter().zip(cols.iter()) {
        p.text(
            r(x + start * w + CELL_PAD * s, y0, (end - start) * w - CELL_PAD * s * 2.0, COLHEAD_H * s),
            name,
            fade(p.theme.text_tertiary, 180),
            7.5,
            *right,
            false,
            false,
            false,
            true,
        );
    }
    let mut y = y0 + COLHEAD_H * s;
    if rows.is_empty() {
        app.note.clear();
        return paint_empty(p, app, x, y, w);
    }
    let now = crate::watch::now_secs();
    let age = age_caption(
        quota::load().get("updated_at").and_then(Value::as_f64).unwrap_or(0.0),
        now,
    );
    let warn = windows
        .iter()
        .filter(|row| matches!(row.get("status").and_then(Value::as_str), Some("warn" | "exhausted")))
        .count();
    let mut note = format!("한도 {}개 · {age} · 행 클릭=대표", rows.len());
    if warn > 0 {
        note.push_str(&format!(" · 경고 {warn}"));
    }
    if quota::load()
        .get("errors")
        .and_then(Value::as_object)
        .is_some_and(|e| !e.is_empty())
    {
        note.push_str(" · 일부 갱신 실패");
    }
    app.note = note;
    let reps: std::collections::HashSet<String> = quota::representative_windows(windows, &app.quota_reps)
        .iter()
        .map(quota::window_key)
        .collect();
    for (i, (title, label, used, reset, status)) in rows.iter().enumerate() {
        let ry = y + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        let raw = windows.get(i).cloned().unwrap_or(json!({}));
        let gap = quota::pace_gap(&raw, Some(now));
        let under = gap.is_some_and(|g| g > 0.0);
        if *used >= 0.0 {
            let hot = status == "warn" || status == "exhausted";
            p.fill(
                r(x, ry, w * (*used as f32).clamp(0.0, 1.0), h),
                fade(
                    if status == "exhausted" {
                        p.theme.destructive
                    } else {
                        p.theme.warning
                    },
                    if hot { 22 } else { 12 },
                ),
            );
        }
        if under {
            p.fill(r(x, ry, w, h), fade(p.theme.success, 18));
            p.fill(r(x, ry, 2.0 * s, h), p.theme.success);
        }
        let color = if status == "exhausted" {
            p.theme.destructive
        } else if status == "warn" {
            p.theme.warning
        } else {
            p.theme.text_primary
        };
        let star = if quota::can_represent(&raw) && reps.contains(&quota::window_key(&raw)) {
            "★ "
        } else {
            ""
        };
        let cells = [
            (format!("{star}{title}"), color, true),
            (label.clone(), p.theme.text_secondary, false),
            (
                if *used >= 0.0 {
                    format!("{:.0}%", used * 100.0)
                } else {
                    "미상".into()
                },
                p.theme.text_tertiary,
                false,
            ),
            (
                if under {
                    format!("여유 {:.0}%p", gap.unwrap() * 100.0)
                } else {
                    "—".into()
                },
                if under { p.theme.success } else { p.theme.text_tertiary },
                false,
            ),
            (
                if reset.is_empty() { "미상".into() } else { reset.clone() },
                p.theme.text_tertiary,
                false,
            ),
        ];
        for (index, ((text, color, bold), (start, end, right, size))) in cells.iter().zip(cols.iter()).enumerate()
        {
            p.text(
                r(x + start * w + CELL_PAD * s, ry, (end - start) * w - CELL_PAD * s * 2.0, h),
                text,
                *color,
                *size,
                *right,
                false,
                true,
                index >= 2,
                *bold,
            );
        }
        if quota::can_represent(&raw) {
            p.hit(format!("row:{i}"), r(x, ry, w, ROW_H * s));
        }
    }
    y + rows.len().max(1) as f32 * ROW_H * s + 6.0 * s
}

fn paint_days(p: &mut Paint, app: &mut OverlayApp, snap: &MeterSnapshot, x: f32, y0: f32, w: f32) -> f32 {
    let s = p.s();
    let today = snap
        .status
        .pointer("/today/date")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let rows: Vec<(String, i64, f64, bool)> = snap
        .days
        .iter()
        .take(app.cap("days") as usize)
        .map(|(d, t, c)| (d.clone(), *t, *c, *d == today))
        .collect();
    if rows.is_empty() {
        app.note.clear();
        return paint_empty(p, app, x, y0, w);
    }
    let spent: f64 = rows.iter().map(|r| r.2).sum();
    app.note = format!("최근 {}일 합계 {}", rows.len(), if spent < 10.0 {
        format!("${spent:.4}")
    } else {
        money_caption(false, spent)
    });
    let top = rows.iter().map(|r| r.2.abs()).fold(0.0, f64::max).max(1.0);
    let mw = 36.0;
    for (i, (day, tokens, cost, hot)) in rows.iter().enumerate() {
        let ry = y0 + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        p.fill(
            r(x, ry, w * (*cost / top).clamp(0.0, 1.0) as f32, h),
            fade(
                if *hot { p.theme.tint } else { p.theme.surface_hover },
                if *hot { 20 } else { 10 },
            ),
        );
        p.hit(format!("row:{i}"), r(x, ry, w, ROW_H * s));
        if *hot {
            p.fill(r(x, ry, 2.0 * s, h), p.theme.tint);
        }
        let mark = if day.len() == 10 { &day[5..] } else { day.as_str() };
        p.text(
            r(x + 6.0 * s, ry, mw * s, h),
            mark,
            if i == 0 { p.theme.tint } else { p.theme.text_tertiary },
            8.5,
            false,
            false,
            false,
            true,
            true,
        );
        p.text(
            r(x + (10.0 + mw) * s, ry, w * 0.38, h),
            if *hot { "진행 중" } else { "" },
            if *hot { p.theme.text_primary } else { p.theme.text_secondary },
            9.0,
            false,
            false,
            true,
            false,
            *hot,
        );
        p.text(
            r(x, ry, w - 58.0 * s, h),
            &compact_num(*tokens as f64),
            p.theme.text_tertiary,
            8.5,
            true,
            false,
            false,
            true,
            false,
        );
        p.text(
            r(x, ry, w - 6.0 * s, h),
            &money_short(*cost),
            if *hot { p.theme.text_primary } else { p.theme.text_secondary },
            8.5,
            true,
            false,
            false,
            true,
            true,
        );
    }
    y0 + rows.len() as f32 * ROW_H * s + 6.0 * s
}

fn paint_board(p: &mut Paint, app: &mut OverlayApp, x: f32, y: f32, w: f32) -> f32 {
    let s = p.s();
    let rows = league_racers();
    if rows.is_empty() {
        app.note.clear();
        return paint_empty(p, app, x, y, w);
    }
    app.note = format!("{}명", rows.len());
    for (i, (handle, tps, color, _)) in rows.iter().enumerate() {
        let ry = y + i as f32 * ROW_H * s;
        let h = (ROW_H - 3.0) * s;
        p.ui.painter().circle_filled(
            Pos2::new(x + 12.0 * s, ry + h / 2.0),
            4.0 * s,
            *color,
        );
        p.text(
            r(x + 20.0 * s, ry, w * 0.58, h),
            handle,
            p.theme.text_secondary,
            9.0,
            false,
            false,
            true,
            false,
            false,
        );
        p.text(
            r(x, ry, w - 6.0 * s, h),
            &format!("{tps:.1}/s"),
            p.theme.text_tertiary,
            8.5,
            true,
            false,
            false,
            true,
            true,
        );
    }
    y + rows.len().max(1) as f32 * ROW_H * s + 6.0 * s
}

fn settings_extra_rows(_app: &OverlayApp) -> Vec<(String, String, bool)> {
    let mut rows = Vec::new();
    if crate::league::enabled() {
        let rooms = crate::league::list_rooms();
        let focus = crate::league::current_focus();
        if rooms.is_empty() {
            rows.push(("note".into(), "참가한 방이 없습니다".into(), false));
        }
        for room in &rooms {
            let rid = room.get("room_id").and_then(Value::as_str).unwrap_or("");
            let host = if room.get("host").and_then(Value::as_bool) == Some(true) {
                " · 호스트"
            } else {
                ""
            };
            rows.push((format!("room:{rid}"), format!("{rid}{host}"), rid == focus));
        }
        rows.push(("command:invite".into(), "초대 링크 복사".into(), false));
        if !rooms.is_empty() {
            rows.push(("command:leave".into(), "이 방 나가기".into(), false));
        }
        if rooms.iter().any(|r| {
            r.get("host").and_then(Value::as_bool) == Some(true)
                && r.get("room_id").and_then(Value::as_str) == Some(focus.as_str())
        }) {
            rows.push(("command:close_room".into(), "이 방 닫기".into(), false));
        }
    }
    rows
}

fn settings_body_h(app: &OverlayApp) -> f32 {
    let mut h = SETTING_HEAD_H + SETTING_CHIP_H + SETTINGS_SEC_GAP
        + SETTING_HEAD_H + SETTING_CHIP_H + SETTINGS_GROUP_GAP
        + SETTING_HEAD_H + SETTING_CHIP_H + SETTINGS_GROUP_GAP
        + SETTING_HEAD_H + SETTING_CHIP_H + SETTINGS_GROUP_GAP
        + SETTING_ROW_H * 5.0 + SETTINGS_SEC_GAP
        + SETTING_HEAD_H + SETTING_CHIP_H + SETTINGS_SEC_GAP
        + SETTING_HEAD_H + SETTING_ROW_H * 4.0;
    let extra = settings_extra_rows(app);
    if !extra.is_empty() {
        h += SETTING_HEAD_H + SETTING_ROW_H * extra.len() as f32 + SETTINGS_SEC_GAP;
    }
    h
}

fn settings_dark(theme: &Theme) -> bool {
    theme.text_primary.r() > 180
}

fn paint_card(p: &Paint<'_>, rect: Rect, track: bool) {
    let s = p.scale;
    let dark = settings_dark(&p.theme);
    let (fill, edge) = if track {
        (
            fade(p.theme.surface_hover, if dark { 16 } else { 12 }),
            fade(
                if dark { p.theme.surface_hover } else { p.theme.separator },
                if dark { 18 } else { 16 },
            ),
        )
    } else if dark {
        (fade(p.theme.surface_glass_elevated, 230), fade(p.theme.separator, 200))
    } else {
        (fade(p.theme.surface_glass_elevated, 236), fade(p.theme.separator, 24))
    };
    let rad = CornerRadius::same((12.0 * s).clamp(4.0, 16.0) as u8);
    p.ui.painter().rect_filled(rect, rad, fill);
    p.ui.painter().rect_stroke(
        rect,
        rad,
        Stroke::new((0.7 * s).max(1.0), edge),
        egui::StrokeKind::Inside,
    );
}

fn paint_rule(p: &Paint<'_>, x: f32, y: f32, w: f32) {
    let s = p.scale;
    let dark = settings_dark(&p.theme);
    let edge = if dark {
        fade(p.theme.surface_hover, 20)
    } else {
        fade(p.theme.separator, 28)
    };
    p.ui.painter().line_segment(
        [Pos2::new(x + 14.0 * s, y), Pos2::new(x + w - 14.0 * s, y)],
        Stroke::new((0.6 * s).max(1.0), edge),
    );
}

fn paint_switch(p: &Paint<'_>, x: f32, y: f32, w: f32, h: f32, on: bool) {
    let s = p.scale;
    let tw = 46.0 * s;
    let th = 26.0 * s;
    let track = r(x + w - 14.0 * s - tw, y + (h - th) / 2.0, tw, th);
    let fill = if on {
        fade(p.theme.success, 235)
    } else {
        fade(p.theme.surface_hover, if settings_dark(&p.theme) { 40 } else { 16 })
    };
    p.ui
        .painter()
        .rect_filled(track, CornerRadius::same((th / 2.0) as u8), fill);
    let knob = th - 4.0 * s;
    let inset = 2.0 * s;
    let knx = if on {
        track.max.x - knob - inset
    } else {
        track.min.x + inset
    };
    let kny = track.min.y + 2.0 * s;
    p.ui
        .painter()
        .circle_filled(Pos2::new(knx + knob / 2.0, kny + knob / 2.0 + 0.7 * s), knob / 2.0, fade(hex("#000000", 255), 36));
    p.ui
        .painter()
        .circle_filled(Pos2::new(knx + knob / 2.0, kny + knob / 2.0), knob / 2.0, hex("#FFFFFF", 255));
}

fn paint_section(p: &Paint<'_>, x: f32, y: f32, w: f32, title: &str) -> f32 {
    let s = p.scale;
    p.text(
        r(x + 4.0 * s, y, w, SETTING_HEAD_H * s),
        title,
        p.theme.text_tertiary,
        11.0,
        false,
        false,
        false,
        false,
        true,
    );
    y + SETTING_HEAD_H * s
}

fn paint_setting_chips(p: &mut Paint<'_>, x: f32, y: f32, w: f32, items: &[(&str, &str, bool)]) -> f32 {
    let s = p.scale;
    let h = SETTING_CHIP_H * s;
    paint_card(p, r(x, y, w, h), true);
    let pad = 3.0 * s;
    let n = items.len().max(1) as f32;
    let cw = (w - pad * 2.0) / n;
    let dark = settings_dark(&p.theme);
    for (i, (target, title, on)) in items.iter().enumerate() {
        let bx = x + pad + i as f32 * cw;
        let rect = r(bx, y, cw, h);
        if *on {
            p.ui.painter().rect_filled(
                r(bx, y + pad, cw, h - pad * 2.0),
                CornerRadius::same((8.0 * s) as u8),
                fade(p.theme.tint, if dark { 40 } else { 28 }),
            );
        }
        p.text(
            rect,
            title,
            if *on { p.theme.text_primary } else { p.theme.text_secondary },
            12.0,
            false,
            true,
            false,
            false,
            true,
        );
        p.hit((*target).to_string(), rect);
    }
    y + h
}

fn paint_toggle(p: &mut Paint<'_>, x: f32, y: f32, w: f32, target: &str, title: &str, on: bool) -> f32 {
    let s = p.scale;
    let h = SETTING_ROW_H * s;
    p.text(
        r(x + 14.0 * s, y, w - 76.0 * s, h),
        title,
        p.theme.text_primary,
        13.0,
        false,
        false,
        true,
        false,
        false,
    );
    paint_switch(p, x, y, w, h, on);
    p.hit(target.to_string(), r(x, y, w, h));
    y + h
}

fn paint_action(p: &mut Paint<'_>, x: f32, y: f32, w: f32, target: &str, title: &str) -> f32 {
    let s = p.scale;
    let h = SETTING_ROW_H * s;
    let danger = target == "command:reset" || target == "command:quit";
    p.text(
        r(x + 14.0 * s, y, w - 36.0 * s, h),
        title,
        if danger { p.theme.destructive } else { p.theme.text_primary },
        13.0,
        false,
        false,
        true,
        false,
        false,
    );
    p.text(
        r(x, y, w - 12.0 * s, h),
        "›",
        p.theme.text_tertiary,
        16.0,
        true,
        false,
        false,
        false,
        false,
    );
    p.hit(target.to_string(), r(x, y, w, h));
    y + h
}

fn paint_select(p: &mut Paint<'_>, x: f32, y: f32, w: f32, target: &str, title: &str, on: bool) -> f32 {
    let s = p.scale;
    let h = SETTING_ROW_H * s;
    p.text(
        r(x + 14.0 * s, y, w - 36.0 * s, h),
        title,
        p.theme.text_primary,
        13.0,
        false,
        false,
        true,
        false,
        false,
    );
    if on {
        p.text(
            r(x, y, w - 14.0 * s, h),
            "✓",
            p.theme.tint,
            14.0,
            true,
            false,
            false,
            false,
            true,
        );
    }
    p.hit(target.to_string(), r(x, y, w, h));
    y + h
}

fn paint_toggles(p: &mut Paint<'_>, x: f32, y: f32, w: f32, items: &[(&str, &str, bool)]) -> f32 {
    let s = p.scale;
    paint_card(p, r(x, y, w, SETTING_ROW_H * items.len() as f32 * s), false);
    let mut yy = y;
    for (i, (target, title, on)) in items.iter().enumerate() {
        if i > 0 {
            paint_rule(p, x, yy, w);
        }
        yy = paint_toggle(p, x, yy, w, target, title, *on);
    }
    yy
}

fn paint_extra_group(p: &mut Paint<'_>, x: f32, y: f32, w: f32, rows: &[(String, String, bool)]) -> f32 {
    let s = p.scale;
    paint_card(p, r(x, y, w, SETTING_ROW_H * rows.len() as f32 * s), false);
    let mut yy = y;
    for (i, (target, title, on)) in rows.iter().enumerate() {
        if i > 0 {
            paint_rule(p, x, yy, w);
        }
        if target == "note" {
            p.text(
                r(x + 14.0 * s, yy, w - 28.0 * s, SETTING_ROW_H * s),
                title,
                p.theme.text_secondary,
                13.0,
                false,
                false,
                true,
                false,
                false,
            );
            yy += SETTING_ROW_H * s;
        } else if target.starts_with("room:") {
            yy = paint_select(p, x, yy, w, target, title, *on);
        } else {
            yy = paint_action(p, x, yy, w, target, title);
        }
    }
    yy
}

fn paint_glass_panel(p: &Paint<'_>, panel: Rect, opaque: bool) {
    let s = p.scale;
    let dark = settings_dark(&p.theme);
    let rad = CornerRadius::same((16.0 * s).clamp(8.0, 22.0) as u8);
    if !opaque {
        let shadow = hex(if dark { "#000000" } else { "#24344A" }, 255);
        for i in (1..=8).rev() {
            let grow = i as f32 * 1.15 * s;
            // 파이썬 shade.adjust(-0.25g, 0.1g, 0.25g, 0.65g)
            let shade = Rect::from_min_max(
                Pos2::new(panel.min.x - grow * 0.25, panel.min.y + grow * 0.1),
                Pos2::new(panel.max.x + grow * 0.25, panel.max.y + grow * 0.65),
            );
            p.ui.painter().rect_filled(
                shade,
                CornerRadius::same((16.0 * s + grow * 0.15) as u8),
                fade(shadow, 8 + i * 4),
            );
        }
    }
    let glass_a = if opaque { 255 } else { p.theme.surface_alpha };
    p.ui.painter().rect_filled(panel, rad, fade(p.theme.surface_glass, glass_a));
    paint_wash(p, panel, rad.nw as f32, 88.0 * s, fade(p.theme.tint, if dark { 20 } else { 14 }));
    p.ui.painter().rect_stroke(
        panel,
        rad,
        Stroke::new(s.max(1.0), fade(p.theme.tint, if dark { 36 } else { 28 })),
        egui::StrokeKind::Inside,
    );
}

/// 파이썬 `_paint_glass` 의 위쪽 틴트: 위에서 `depth` 까지 `top` → 투명. 둥근 모서리 안쪽만 칠한다.
fn paint_wash(p: &Paint<'_>, panel: Rect, rad: f32, depth: f32, top: Color32) {
    let mut mesh = egui::Mesh::default();
    let rows = (rad.ceil() as usize).max(1);
    let ys = (0..=rows).map(|k| k as f32 * rad / rows as f32).chain(std::iter::once(depth));
    for (k, dy) in ys.enumerate() {
        let inset = if dy < rad { rad - (rad * rad - (rad - dy).powi(2)).sqrt() } else { 0.0 };
        let c = top.gamma_multiply(1.0 - (dy / depth).clamp(0.0, 1.0));
        mesh.colored_vertex(Pos2::new(panel.min.x + inset, panel.min.y + dy), c);
        mesh.colored_vertex(Pos2::new(panel.max.x - inset, panel.min.y + dy), c);
        if k > 0 {
            let i = k as u32 * 2;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    p.ui.painter().add(egui::Shape::mesh(mesh));
}

fn paint_settings_viewport(ctx: &egui::Context, app: &mut OverlayApp) {
    let theme = app.resolved_theme(ctx.style().visuals.dark_mode);
    let extra = settings_extra_rows(app);
    let s = app.scale;
    let w = (SETTINGS_W + SETTINGS_CHROME * 2.0) * s;
    let h = (SETTINGS_CHROME * 2.0 + SETTINGS_PAD * 2.0 + settings_body_h(app) + FOOT_H) * s;
    let id = egui::ViewportId::from_hash_of("tokenmeter-settings");
    let title = crate::i18n::tr(&app.lang, "TokenMeter 설정");
    // macOS: 새 설정 창은 첫 프레임만 숨겨 두고 모든 Space 속성을 붙인 뒤 보인다.
    let first = cfg!(target_os = "macos") && app.settings_frames == 0;
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_inner_size([w, h])
        .with_visible(!first);
    let mut clicked = String::new();
    ctx.show_viewport_immediate(id, builder, |ctx, _| {
        if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
            clicked = "dismiss".into();
        }
        let mut hits = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let mut p = Paint {
                    ui,
                    hits: &mut hits,
                    theme: theme.clone(),
                    scale: s,
                    lang: &app.lang,
                };
                let full = p.ui.max_rect();
                let chrome = SETTINGS_CHROME * s;
                paint_glass_panel(
                    &p,
                    r(full.min.x + chrome, full.min.y + chrome, full.width() - chrome * 2.0, full.height() - chrome * 2.0),
                    app.reduce_transparency,
                );
                let inset = (SETTINGS_CHROME + SETTINGS_PAD) * s;
                let x = full.min.x + inset;
                let mut yy = full.min.y + inset;
                let iw = full.width() - inset * 2.0;
                yy = paint_section(&p, x, yy, iw, "언어");
                yy = paint_setting_chips(&mut p, x, yy, iw, &[
                    ("lang:ko", "한국어", app.lang == "ko"),
                    ("lang:en", "English", app.lang == "en"),
                ]);
                yy += SETTINGS_SEC_GAP * s;
                yy = paint_section(&p, x, yy, iw, "모양");
                yy = paint_setting_chips(&mut p, x, yy, iw, &[
                    ("theme:system", "시스템", app.theme_mode == "system"),
                    ("theme:dark", "다크", app.theme_mode == "dark"),
                    ("theme:light", "라이트", app.theme_mode == "light"),
                ]);
                yy += SETTINGS_GROUP_GAP * s;
                yy = paint_section(&p, x, yy, iw, "최소화");
                yy = paint_setting_chips(&mut p, x, yy, iw, &[
                    ("skin:bar", "기본 미터", app.s_skin == "bar"),
                    ("skin:dial", "픽셀 다이얼", app.s_skin == "dial"),
                    ("skin:loot", "픽셀 코인", app.s_skin == "loot"),
                ]);
                yy += SETTINGS_GROUP_GAP * s;
                yy = paint_section(&p, x, yy, iw, "미니 투명도");
                yy = paint_setting_chips(&mut p, x, yy, iw, &[
                    ("mini_opacity:light", "약하게", app.mini_opacity == "light"),
                    ("mini_opacity:mid", "보통", app.mini_opacity == "mid"),
                    ("mini_opacity:strong", "강하게", app.mini_opacity == "strong"),
                ]);
                yy += SETTINGS_GROUP_GAP * s;
                yy = paint_toggles(&mut p, x, yy, iw, &[
                    ("command:transparency", "투명도 줄이기", app.reduce_transparency),
                    ("command:motion", "모션 줄이기", app.reduce_motion),
                    ("command:top", "항상 위", app.on_top),
                    ("command:mini", "미니 모드", app.mini),
                    ("command:mini_hover", "마우스 올리면 선명하게", app.mini_hover),
                ]);
                yy += SETTINGS_SEC_GAP * s;
                yy = paint_section(&p, x, yy, iw, "크기");
                let pct = format!("{:.0}%", app.scale * 100.0);
                yy = paint_setting_chips(&mut p, x, yy, iw, &[
                    ("command:scale_down", "−", false),
                    ("command:scale_reset", pct.as_str(), (app.scale - 1.0).abs() < 0.001),
                    ("command:scale_up", "+", false),
                ]);
                yy += SETTINGS_SEC_GAP * s;
                if !extra.is_empty() {
                    yy = paint_section(&p, x, yy, iw, "리그");
                    yy = paint_extra_group(&mut p, x, yy, iw, &extra);
                    yy += SETTINGS_SEC_GAP * s;
                }
                yy = paint_section(&p, x, yy, iw, "데이터");
                let data = [
                    ("command:price".into(), "환산 단가 입력…".into(), false),
                    ("command:reset".into(), "통계 초기화".into(), false),
                    ("close".into(), "오버레이 숨기기 · 측정 계속".into(), false),
                    ("command:quit".into(), "TokenMeter 종료 · 측정 중지".into(), false),
                ];
                yy = paint_extra_group(&mut p, x, yy, iw, &data);
                let _ = yy;
                p.text(
                    r(x, full.max.y - (SETTINGS_CHROME + FOOT_H) * s, iw, FOOT_H * s),
                    "esc 닫기",
                    p.theme.text_tertiary,
                    11.0,
                    false,
                    true,
                    false,
                    false,
                    false,
                );
            });
        if let Some(pos) = ctx.pointer_interact_pos() {
            if ctx.input(|i| i.pointer.primary_pressed()) {
                if let Some(name) = hit_test(&hits, pos) {
                    clicked = name;
                }
            }
        }
    });
    #[cfg(target_os = "macos")]
    if first {
        crate::macos::set_all_spaces(app.all_spaces);
    }
    app.settings_frames = app.settings_frames.saturating_add(1);
    if clicked == "dismiss" {
        app.settings_open = false;
    } else if !clicked.is_empty() {
        let snap = app.shared.lock().map(|g| g.clone()).unwrap_or_default();
        let windows = quota::load()
            .get("windows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        app.activate(&clicked, &snap, &windows);
    }
}

pub fn run_window(shared: SharedMeter, start_hidden: bool) -> i32 {
    match run_overlay_hidden(shared, start_hidden) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("[TokenMeter] 오버레이 실패: {err}");
            0
        }
    }
}

pub fn run_overlay(shared: SharedMeter) -> eframe::Result<()> {
    run_overlay_hidden(shared, false)
}

/// 창이 한 번이라도 떴는지. 데몬은 뜨기 전의 실패(화면 없음)만 창 없이 버틴다.
pub static OVERLAY_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn run_overlay_hidden(shared: SharedMeter, hidden: bool) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([340.0, 430.0])
            .with_min_inner_size([200.0, HEADER_H])
            .with_always_on_top()
            .with_decorations(false)
            .with_transparent(true)
            .with_visible(!hidden)
            .with_title("TokenMeter"),
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(crate::macos::accessory_event_loop)),
        ..Default::default()
    };
    eframe::run_native(
        "TokenMeter",
        options,
        Box::new(|cc| {
            OVERLAY_STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
            install_cjk_fonts(&cc.egui_ctx);
            let mut visuals = cc.egui_ctx.style().visuals.clone();
            visuals.panel_fill = Color32::TRANSPARENT;
            visuals.window_fill = Color32::TRANSPARENT;
            cc.egui_ctx.set_visuals(visuals);
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            let mut app = OverlayApp::from_prefs(shared, hidden);
            #[cfg(target_os = "macos")]
            {
                crate::macos::set_all_spaces(app.all_spaces);
                app.status = crate::macos::StatusItem::create();
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_commands_are_edge_triggered() {
        let mut state = ViewportState::new(false);
        assert!(state.commands(false, 340.0, 430.0, true).is_empty());

        let hidden = state.commands(true, 340.0, 430.0, true);
        assert!(matches!(
            hidden.as_slice(),
            [egui::ViewportCommand::Visible(false)]
        ));
        assert!(state.commands(true, 340.0, 430.0, true).is_empty());

        let resized = state.commands(true, 340.0, 560.0, true);
        assert!(matches!(
            resized.as_slice(),
            [egui::ViewportCommand::InnerSize(size)] if *size == Vec2::new(340.0, 560.0)
        ));
        assert!(state.commands(true, 340.0, 560.0, true).is_empty());
    }

    #[test]
    fn hidden_start_hides_the_first_frame() {
        let (_g, _tmp) = crate::test_home("hidden-start");
        let mut app = OverlayApp::from_prefs(Arc::new(Mutex::new(MeterSnapshot::default())), true);
        let (w, h, top) = (app.viewport.width, app.viewport.height, app.on_top);
        let cmds = app.viewport.commands(true, w, h, top);
        assert!(cmds.iter().any(|c| matches!(c, ViewportCommand::Visible(false))), "{cmds:?}");
    }

    #[test]
    fn saved_position_survives_other_monitors() {
        let main = Rect::from_min_size(Pos2::ZERO, Vec2::new(1728.0, 1080.0));
        let right = Rect::from_min_size(Pos2::new(1728.0, 0.0), Vec2::new(2560.0, 1415.0));
        assert_eq!(restore_pos([2000.0, 300.0], &[main, right], 340.0), Pos2::new(2000.0, 300.0));
        assert_eq!(restore_pos([2000.0, 300.0], &[main], 340.0), Pos2::new(40.0, 80.0), "뺀 모니터");
        assert_eq!(restore_pos([900.0, 700.0], &[main], 340.0), Pos2::new(900.0, 700.0), "화면 아래쪽");
        assert_eq!(restore_pos([1600.0, 20.0], &[main], 340.0), Pos2::new(1600.0, 20.0), "S 폭이어도 x=0으로 붙지 않는다");
        assert_eq!(restore_pos([5.0, 5.0], &[Rect::EVERYTHING], 340.0), Pos2::new(5.0, 5.0), "모니터 정보 없음");
    }

    #[test]
    fn pulse_phase_is_smooth_and_time_based() {
        assert_eq!(pulse_phase(false, 0.25), 0.0);
        assert!((pulse_phase(true, 0.25) - 1.0).abs() < 0.001);
        assert!((pulse_phase(true, 0.999) - pulse_phase(true, 0.001)).abs() < 0.01);
    }

    #[test]
    fn segment_color_is_green_then_amber_then_red() {
        let t = theme_named("dark");
        assert_eq!(seg_color(&t, 0.0), t.success);
        assert_eq!(seg_color(&t, 0.54), t.success);
        assert_eq!(seg_color(&t, 0.55), t.warning);
        assert_eq!(seg_color(&t, 0.79), t.warning);
        assert_eq!(seg_color(&t, 0.8), t.destructive);
    }

    #[test]
    fn search_score_prefix_beats_subsequence() {
        assert!(search_score("세션", &["세션", "현재 에이전트"]) > search_score("세션", &["보관 세션", "종료"]));
        assert_eq!(search_score("", &["anything"]), 1);
        assert_eq!(search_score("zzz", &["세션"]), 0);
    }

    #[test]
    fn comma_rate_matches_python_thousands() {
        assert_eq!(comma_rate(0.0), "0.0");
        assert_eq!(comma_rate(478.4), "478.4");
        assert_eq!(comma_rate(1234.5), "1,234.5");
    }

    #[test]
    fn pixel_face_is_the_mini_look_only() {
        let mut app = OverlayApp::from_prefs(Arc::new(Mutex::new(MeterSnapshot::default())), false);
        app.expanded = false;
        app.rows_on = false;
        app.mini = false;
        app.palette_open = false;
        app.scale = 1.0;
        app.s_skin = "bar".into();
        let s_bar = app.window_size(0, 0, 0);
        app.s_skin = "dial".into();
        assert_eq!(app.window_size(0, 0, 0), s_bar, "S 는 스킨과 무관하게 기본 계기판");
        assert_eq!(app.widget_skin(), None);
        app.mini = true;
        assert_eq!(app.window_size(0, 0, 0), crate::pixel_faces::MINI_SIZE);
        assert_eq!(app.widget_skin(), Some("dial"));
        app.s_skin = s_skin_name("horse").into();
        assert_eq!(app.widget_skin(), Some("loot"), "택시를 고른 사람은 픽셀 코인으로");
    }

    #[test]
    fn window_size_grows_for_l_and_graph() {
        let mut app = OverlayApp::from_prefs(Arc::new(Mutex::new(MeterSnapshot::default())), false);
        app.expanded = false;
        app.rows_on = true;
        app.mini = false;
        app.palette_open = false;
        app.scale = 1.0;
        let (mw, mh) = app.window_size(6, 0, 0);
        app.expanded = true;
        app.panel = "days".into();
        let (lw, lh) = app.window_size(7, 0, 0);
        assert!(lw > mw);
        assert!(lh > mh);
    }

    #[test]
    fn pretendard_is_bundled() {
        assert!(include_bytes!("../fonts/Pretendard-Regular.otf").len() > 100_000);
        assert!(include_bytes!("../fonts/Pretendard-SemiBold.otf").len() > 100_000);
    }

    #[test]
    fn translucent_colors_blend_like_qt() {
        // 흰색 알파 10 은 Qt 처럼 10 이어야 한다. 선형 공간에서 곱하면 56 이 돼 칸이 회색으로 뜬다.
        assert_eq!(fade(Color32::WHITE, 10), Color32::from_rgba_premultiplied(10, 10, 10, 10));
        assert_eq!(hex("#34C759", 255), Color32::from_rgb(0x34, 0xC7, 0x59));
        assert_eq!(fade(Color32::WHITE, 0), Color32::TRANSPARENT);
    }

    #[test]
    fn mini_alpha_fades_only_the_idle_mini() {
        assert_eq!(mini_alpha(false, false, false, false, "strong"), 1.0, "S/M/L");
        assert_eq!(mini_alpha(true, false, false, false, "light"), 0.85);
        assert_eq!(mini_alpha(true, false, false, false, "mid"), 0.70);
        assert_eq!(mini_alpha(true, false, false, false, "strong"), 0.55);
        assert_eq!(mini_alpha(true, false, false, false, "weird"), 0.70);
        assert_eq!(mini_alpha(true, true, false, false, "strong"), 1.0, "투명도 줄이기가 먼저");
        assert_eq!(mini_alpha(true, false, true, false, "strong"), 1.0, "설정·팔레트");
        assert_eq!(mini_alpha(true, false, false, true, "strong"), 1.0, "커서");
    }

    #[test]
    fn clear_color_is_eframe_default_unless_mini_or_hidden() {
        let default = Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32();
        assert_eq!(clear_rgba(false, false), default);
        assert_eq!(clear_rgba(true, false), [0.0; 4]);
        assert_eq!(clear_rgba(false, true), [0.0; 4]);
    }

    #[test]
    fn visual_prefs_round_trip_with_defaults() {
        let (_g, _tmp) = crate::test_home("visual-prefs");
        std::fs::create_dir_all(data_dir()).unwrap();
        let shared = || Arc::new(Mutex::new(MeterSnapshot::default()));
        let app = OverlayApp::from_prefs(shared(), false);
        assert_eq!((app.mini_opacity.as_str(), app.mini_hover), ("mid", true));
        assert!(app.all_spaces);
        assert_eq!((app.menubar.as_str(), app.menubar_value.as_str()), ("always", "rate"));
        std::fs::write(prefs_path(), r#"{"mini_opacity":"strong","mini_hover":false}"#).unwrap();
        let app = OverlayApp::from_prefs(shared(), false);
        assert_eq!((app.mini_opacity.as_str(), app.mini_hover), ("strong", false));
        app.save_prefs();
        let back = load_prefs();
        assert_eq!((back["mini_opacity"].as_str(), back["mini_hover"].as_bool()), (Some("strong"), Some(false)));
        std::fs::write(prefs_path(), r#"{"mini_opacity":"weird"}"#).unwrap();
        assert_eq!(OverlayApp::from_prefs(shared(), false).mini_opacity, "mid");
        std::fs::write(prefs_path(), r#"{"all_spaces":false}"#).unwrap();
        assert!(!OverlayApp::from_prefs(shared(), false).all_spaces);
        std::fs::write(prefs_path(), r#"{"menubar":"folded","menubar_value":"cost"}"#).unwrap();
        let mut app = OverlayApp::from_prefs(shared(), false);
        assert_eq!((app.menubar.as_str(), app.menubar_value.as_str()), ("folded", "cost"));
        // 메뉴에서만 바뀌는 세 키가 save_prefs 에서 빠지면 실패한다.
        app.all_spaces = false;
        app.save_prefs();
        let back = load_prefs();
        assert_eq!(
            (back["all_spaces"].as_bool(), back["menubar"].as_str(), back["menubar_value"].as_str()),
            (Some(false), Some("folded"), Some("cost"))
        );
    }
}
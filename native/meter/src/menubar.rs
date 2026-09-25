//! 메뉴바 미터의 그림과 문구. AppKit 없이 계산만 해서 모든 플랫폼에서 테스트한다.

use crate::overlay::{fade, mini_rate_caption, money_caption, seg_color, theme_named};

pub const CELLS: usize = 10;
/// @2x 픽셀 크기. 점 크기는 39×18pt 다.
pub const W: usize = 78;
pub const H: usize = 36;
/// 칸 3pt · 간격 1pt · 막대 높이 8pt (@2x).
const CELL: usize = 6;
const GAP: usize = 2;
const BAR_H: usize = 16;
/// 꺼진 칸 알파. 창의 26 은 메뉴바에서 너무 흐리다.
const UNLIT: u8 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readout {
    Rate,
    Cost,
}

pub struct BarImage {
    pub w: usize,
    pub h: usize,
    /// sRGB premultiplied RGBA, 위쪽 줄부터.
    pub rgba: Vec<u8>,
}

/// 켜진 칸 수. `gauge` 는 이미 `gauge_target` 을 거친 0~1 값이다.
pub fn lit(gauge: f64) -> usize {
    (gauge.clamp(0.0, 1.0) * CELLS as f64).round() as usize
}

/// 본 계기판과 같은 구간 색(칸 i 의 위치는 i / 9). 밝은 메뉴바는 라이트 팔레트.
pub fn bar_image(lit: usize, dark: bool) -> BarImage {
    let theme = theme_named(if dark { "dark" } else { "light" });
    let mut rgba = vec![0u8; W * H * 4];
    let top = (H - BAR_H) / 2;
    for i in 0..CELLS {
        let base = seg_color(&theme, i as f32 / (CELLS - 1) as f32);
        let color = if i < lit { base } else { fade(base, UNLIT) };
        let x0 = i * (CELL + GAP);
        for y in top..top + BAR_H {
            for x in x0..x0 + CELL {
                let o = (y * W + x) * 4;
                rgba[o..o + 4].copy_from_slice(&color.to_array());
            }
        }
    }
    BarImage { w: W, h: H, rgba }
}

/// 숫자 문구. 속도는 미니와 같은 모양에서 `/s` 를 뺀다. 비용은 늘 오늘 값이다.
pub fn caption(readout: Readout, rate: f64, today: f64) -> String {
    match readout {
        Readout::Rate => mini_rate_caption(rate).trim_end_matches("/s").to_string(),
        Readout::Cost => money_caption(false, today),
    }
}

/// 등폭 글꼴에서 폭이 흔들리지 않게 오른쪽 정렬한다. 속도는 최대 4자, 비용은 $1,000 아래 7자.
pub fn padded(readout: Readout, text: &str) -> String {
    match readout {
        Readout::Rate => format!("{text:>4}"),
        Readout::Cost => format!("{text:>7}"),
    }
}

pub fn tooltip(lang: &str, rate: f64, today: f64) -> String {
    format!(
        "TokenMeter · {} tok/s · {} {}",
        caption(Readout::Rate, rate, today),
        crate::i18n::tr(lang, "오늘"),
        caption(Readout::Cost, rate, today)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &BarImage, x: usize, y: usize) -> [u8; 4] {
        let o = (y * img.w + x) * 4;
        img.rgba[o..o + 4].try_into().unwrap()
    }

    fn cell(img: &BarImage, i: usize) -> [u8; 4] {
        px(img, i * (CELL + GAP) + 1, H / 2)
    }

    #[test]
    fn gauge_lights_whole_cells() {
        for (gauge, want) in [(0.0, 0), (0.04, 0), (0.06, 1), (0.5, 5), (1.0, 10), (1.7, 10)] {
            let img = bar_image(lit(gauge), true);
            assert_eq!((img.w, img.h, img.rgba.len()), (78, 36, 78 * 36 * 4));
            let on = (0..CELLS).filter(|&i| cell(&img, i)[3] == 255).count();
            assert_eq!(on, want, "gauge {gauge}");
            assert!((0..CELLS).all(|i| matches!(cell(&img, i)[3], 255 | UNLIT)));
        }
    }

    #[test]
    fn cells_follow_the_meter_zones_and_gaps_stay_clear() {
        let t = theme_named("dark");
        let img = bar_image(CELLS, true);
        let (green, amber, red) = (seg_color(&t, 0.0), seg_color(&t, 0.6), seg_color(&t, 0.9));
        for i in 0..CELLS {
            let want = if i <= 4 { green } else if i <= 7 { amber } else { red };
            assert_eq!(cell(&img, i), want.to_array(), "칸 {i}");
        }
        assert_eq!(px(&img, CELL, H / 2), [0, 0, 0, 0], "칸 사이 간격");
        assert_eq!(px(&img, 1, 0), [0, 0, 0, 0], "막대 위쪽 여백");
        assert_ne!(bar_image(CELLS, false).rgba, img.rgba, "밝은 메뉴바는 라이트 팔레트");
    }

    #[test]
    fn captions_match_the_mini_and_money_formats() {
        let rate = |v| caption(Readout::Rate, v, 0.0);
        assert_eq!([rate(0.0), rate(12.3), rate(100.0), rate(4500.0), rate(1e18)], ["0.0", "12.3", "100", "4.5k", "MAX"]);
        let cost = |v| caption(Readout::Cost, 0.0, v);
        assert_eq!([cost(0.0), cost(3.21), cost(1234.5)], ["$0.00", "$3.21", "$1,234.50"]);
        assert_eq!(padded(Readout::Rate, "0.0"), " 0.0");
        assert_eq!(padded(Readout::Rate, "MAX"), " MAX");
        assert_eq!(padded(Readout::Cost, "$3.21"), "  $3.21");
        assert_eq!(padded(Readout::Cost, "$1,234.50"), "$1,234.50", "넘치면 자르지 않는다");
    }

    #[test]
    fn tooltip_names_speed_and_today() {
        assert_eq!(tooltip("ko", 1234.0, 3.21), "TokenMeter · 1.2k tok/s · 오늘 $3.21");
        assert_eq!(tooltip("en", 1234.0, 3.21), "TokenMeter · 1.2k tok/s · Today $3.21");
    }
}

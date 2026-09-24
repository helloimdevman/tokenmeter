//! 미니 출력용 40×28 픽셀 다이얼/코인. v0.7.2 `pixel_faces.py` 와 같은 셀을 쓴다.
//! 반올림은 파이썬 `round()` 처럼 짝수 쪽(`round_ties_even`)이어야 바늘 칸이 같다.

use std::collections::HashMap;
use std::sync::OnceLock;

pub const GRID: (i32, i32) = (40, 28);
pub const MINI_SIZE: (f32, f32) = (168.0, 64.0);
const MONEY_THRESHOLDS: [f64; 3] = [100.0, 500.0, 1500.0];

const TICKS: [(i32, i32); 17] = [
    (13, 20),
    (11, 18),
    (10, 15),
    (10, 12),
    (11, 9),
    (12, 7),
    (14, 5),
    (17, 3),
    (20, 3),
    (23, 3),
    (26, 5),
    (28, 7),
    (29, 9),
    (30, 12),
    (30, 15),
    (29, 18),
    (27, 20),
];
const TIPS: [(i32, i32); 17] = [
    (14, 19),
    (13, 17),
    (12, 15),
    (12, 12),
    (13, 10),
    (14, 8),
    (16, 6),
    (18, 5),
    (20, 5),
    (22, 5),
    (24, 6),
    (26, 8),
    (27, 10),
    (28, 12),
    (28, 15),
    (27, 17),
    (26, 19),
];

const COIN: [&str; 27] = [
    "..........hhhhhhh..........",
    ".......hhhhssssshhhh.......",
    "......hhsssaaaaassshh......",
    "....hhhsaaaaaaaaaaashhh....",
    "...hhssaaaaaaaaaaaaasshs...",
    "...hssaaaaaaaaaaaaaaashs...",
    "..hhsaaaaaaaaaaaaaaaaahss..",
    ".hhsaaaaaaaaaaaaaaaaaaahss.",
    ".hsaaaaaaaaaaaaaaaaaaaaahs.",
    ".hsaaaaaaaaaaaaaaaaaaaaahs.",
    "hhsaaaaaaaaaaaaaaaaaaaaahss",
    "hsaaaaaaaaaaaaaaaaaaaaaaahs",
    "hsaaaaaaaaaaaaaaaaaaaaaaahs",
    "hsaaaaaaaaaaaaaaaaaaaaaaahs",
    "hsaaaaaaaaaaaaaaaaaaaaaaahs",
    "hsaaaaaaaaaaaaaaaaaaaaaaahs",
    "hhsaaaaaaaaaaaaaaaaaaaaahss",
    ".hsaaaaaaaaaaaaaaaaaaaaahs.",
    ".hsaaaaaaaaaaaaaaaaaaaaahs.",
    ".hhsaaaaaaaaaaaaaaaaaaahss.",
    "..hhsaaaaaaaaaaaaaaaaahss..",
    "...hssaaaaaaaaaaaaaaahhs...",
    "...hhhhaaaaaaaaaaaaahhss...",
    "....ssshaaaaaaaaaaahsss....",
    "......sshhhaaaaahhhss......",
    ".......sssshhhhhssss.......",
    "..........sssssss..........",
];
const BILL: [&str; 28] = [
    "........................................",
    "........................................",
    "........................................",
    "........................................",
    "........................................",
    ".....nnnnnnnnnnnnnnnnnnnnnnnnnnnnnn.....",
    "....nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn....",
    "....nngppppppppppppppppppppppppppgnn....",
    "....nngpppggggggpppppppppgggggggggnn....",
    "....nngpgpggggggppppnppppgggggggggnn....",
    "....nngpppggggggpppnnnnppgggggggggnn....",
    "....nnggggggggggppnnpppppgggggggggnn....",
    "....nnggggggggggppnnpppppgggggggggnn....",
    "....nnggggggggggpppnnnpppgggggggggnn....",
    "....nnggggggggggpppppnnppgggggggggnn....",
    "....nnggggggggggpppppnnppgggggggggnn....",
    "....nnggggggggggppnnnnpppgggggpppgnn....",
    "....nnggggggggggppppnppppgggggpgpgnn....",
    "....nnggggggggggpppppppppgggggpppgnn....",
    "....nnggggggggggggggggggggggggggggnn....",
    "....nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn....",
    ".....nnnnnnnnnnnnnnnnnnnnnnnnnnnnnn.....",
    "........................................",
    "........................................",
    "........................................",
    "........................................",
    "........................................",
    "........................................",
];
const BUNDLE: [&str; 28] = [
    "........................................",
    "........................................",
    "......nnnnnnnnnnnnnnhhhhsnnnnnnnnnnn....",
    ".....nnnnnnnnnnnnnnnhaahsnnnnnnnnnnnn...",
    ".....nngpppppppppppphaahspppppppppgnn...",
    ".....nngpppggggggppphaahspgggggggggnn...",
    ".....nngpgpggggggppphaahspgggggggggnn...",
    "....nnngpppggggggppphaahspgggggggggnn...",
    "....nnnggggggggggppnhaahspgggggggggnn...",
    "....nnnggggggggggppnhaahspgggggggggnn...",
    "....nnnggggggggggppphaahspgggggggggnn...",
    "...nnnnggggggggggppphaahspgggggggggnn...",
    "...nnnnggggggggggppphaahspgggggpppgnn...",
    "...nnnnggggggggggppnhaahspgggggpgpgnn...",
    "...nnnnggggggggggppphaahspgggggpppgnn...",
    "...nnnnggggggggggggghaahsggggggggggnn...",
    "...nnnnnnnnnnnnnnnnnhaahsnnnnnnnnnnnn...",
    "...nnnnnnnnnnnnnnnnnhaahsnnnnnnnnnnn....",
    "...nnnggggggggggpppphaahsgggggpppgnn....",
    "...nnngggggggggggggghaahsgggggggggnn....",
    "...nnnnnnnnnnnnnnnnnhaahsnnnnnnnnnnn....",
    "...nnnnnnnnnnnnnnnnnhaahsnnnnnnnnnn.....",
    "...nnggggggggggppppnhaahsggggpppgnn.....",
    "...nnggggggggggggggghaahsggggggggnn.....",
    "...nnnnnnnnnnnnnnnnnhaahsnnnnnnnnnn.....",
    "....nnnnnnnnnnnnnnnnhhhhsnnnnnnnnn......",
    "........................................",
    "........................................",
];
const BAG: [&str; 28] = [
    "........................................",
    "..............haaaaaaaaaaas.............",
    "..............hasaaaaaasaas.............",
    "...............hsaaaaaasas..............",
    "................saaaaaass...............",
    ".................haaaaas................",
    ".................haaaaas................",
    "...............hddddddddds..............",
    "................dddddddddsss............",
    "................haaaaaaas..ss...........",
    "..............haaaaaaaaaaasss...........",
    ".............haaaaaaaaaaaaas............",
    "............haaaaaaaaaaaaaaas...........",
    "...........haaaaaaaadaaaaaaass..........",
    "..........haaaaaaaaddddaaaaaass.........",
    "..........haaaaaaaddaaaaaaaaass.........",
    "..........haaaaaaaddaaaaaaaaass.........",
    "..........haaaaaaaadddaaaaaaass.........",
    "..........haaaaaaaaaaddaaaaaass.........",
    "..........haaaaaaaaaaddaaaaaass.........",
    "..........haaaaaaaddddaaaaaaass.........",
    "..........haaaaaaaaadaaaaaaaass.........",
    "...........haaaaaaaaaaaaaaaass..........",
    "...........haaaaaaaaaaaaaaaass..........",
    "............haasssssssssssass...........",
    "..............haaaaaaaaaass.............",
    "........................................",
    "........................................",
];

fn ring() -> &'static [(i32, i32)] {
    static RING: OnceLock<Vec<(i32, i32)>> = OnceLock::new();
    RING.get_or_init(|| {
        let mut out = Vec::new();
        for y in 0..27 {
            for x in 7..34 {
                let d2 = (x - 20) * (x - 20) + (y - 13) * (y - 13);
                if (144..=182).contains(&d2) {
                    out.push((x, y));
                }
            }
        }
        out
    })
}

pub fn palette_hex(skin: &str, key: char) -> Option<&'static str> {
    match (skin, key) {
        ("dial", 'a') => Some("#ECECEC"),
        ("dial", 'd') => Some("#454545"),
        ("dial", 's') => Some("#808080"),
        ("dial", 'h') => Some("#C8C8C8"),
        ("dial", 'w') => Some("#FFFFFF"),
        ("loot", 'a') => Some("#FFC53D"),
        ("loot", 'd') => Some("#4A3C25"),
        ("loot", 's') => Some("#AE7B20"),
        ("loot", 'h') => Some("#FFE7A0"),
        ("loot", 'w') => Some("#F3F6FB"),
        ("loot", 'g') => Some("#92CFA0"),
        ("loot", 'n') => Some("#35694F"),
        ("loot", 'p') => Some("#DDF0BC"),
        _ => None,
    }
}

pub fn money_stage(rate: f64, previous: i32) -> i32 {
    let mut stage = previous.clamp(0, 3);
    while stage < 3 && rate >= MONEY_THRESHOLDS[stage as usize] {
        stage += 1;
    }
    while stage > 0 && rate < MONEY_THRESHOLDS[(stage as usize) - 1] * 0.85 {
        stage -= 1;
    }
    stage
}

pub fn cells(
    skin: &str,
    gauge: f64,
    phase: f64,
    active: bool,
    stage: i32,
) -> HashMap<(i32, i32), char> {
    let mut pixels = HashMap::new();
    match skin {
        "dial" => {
            let gauge = gauge.clamp(0.0, 1.0);
            for &(x, y) in ring() {
                pixels.insert((x, y), 's');
            }
            for (i, &(x, y)) in TICKS.iter().enumerate() {
                pixels.insert((x, y), if i as f64 <= gauge * 16.0 { 'a' } else { 'd' });
            }
            let tip = TIPS[(gauge * 16.0).round_ties_even() as usize];
            let dx = tip.0 - 20;
            let dy = tip.1 - 13;
            let n = dx.abs().max(dy.abs()).max(1);
            for i in 0..=n {
                let x = 20 + (dx as f64 * i as f64 / n as f64).round_ties_even() as i32;
                let y = 13 + (dy as f64 * i as f64 / n as f64).round_ties_even() as i32;
                pixels.insert((x, y), 'w');
            }
            for yy in 12..15 {
                for xx in 19..22 {
                    pixels.insert((xx, yy), 'a');
                }
            }
            pixels.insert((20, 13), 's');
        }
        "loot" if stage <= 0 => {
            let frame = if active { ((phase * 5.0) as i32).rem_euclid(4) } else { 0 };
            for (y, row) in COIN.iter().enumerate() {
                for (x, ch) in row.chars().enumerate() {
                    if ch == '.' {
                        continue;
                    }
                    let mut color = ch;
                    if color == 'h' && (x as i32 + y as i32 + frame * 7).rem_euclid(28) < 6 {
                        color = 'w';
                    }
                    pixels.insert((x as i32 + 7, y as i32), color);
                }
            }
        }
        "loot" => {
            let art = [BILL.as_slice(), BUNDLE.as_slice(), BAG.as_slice()]
                [(stage.clamp(1, 3) as usize) - 1];
            for (y, row) in art.iter().enumerate() {
                for (x, ch) in row.chars().enumerate() {
                    if ch != '.' {
                        pixels.insert((x as i32, y as i32), ch);
                    }
                }
            }
        }
        _ => {}
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_stage_holds_near_threshold() {
        assert_eq!(money_stage(0.0, 0), 0);
        assert_eq!(money_stage(100.0, 0), 1);
        assert_eq!(money_stage(90.0, 1), 1);
        assert_eq!(money_stage(84.0, 1), 0);
    }

    #[test]
    fn dial_cells_include_ring_and_hub() {
        let pixels = cells("dial", 0.5, 0.0, false, 0);
        assert!(pixels.len() > 40);
        assert_eq!(pixels.get(&(20, 13)), Some(&'s'));
    }
}

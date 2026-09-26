//! 오버레이 문구. lang=en 이면 한글 키를 영어로 바꾼다.

/// 사용자 문구를 한국어로 낼까. ponytail: 오버레이에 저장된 `lang`을 보지 않는다. 한국어가 기본이고
/// TOKENMETER_LANG=en* 일 때만 영어. Task 10.1이 lang()으로 바꾼다
/// (저장값 → TOKENMETER_LANG → LC_* → macOS 선호 언어 → en).
pub fn ko() -> bool {
    !std::env::var("TOKENMETER_LANG").map(|v| v.to_ascii_lowercase().starts_with("en")).unwrap_or(false)
}

/// 새 사용자 문구는 `l10n!("English", "한국어")` 짝으로 쓴다. 인자는 `format!`과 같다.
#[macro_export]
macro_rules! l10n {
    ($en:literal, $ko:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::ko() { format!($ko $(, $($arg)*)?) } else { format!($en $(, $($arg)*)?) }
    };
}

pub fn normalize(name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("en") { "en" } else { "ko" }
}

pub fn tr(lang: &str, text: &str) -> String {
    if lang != "en" || text.is_empty() {
        return text.to_string();
    }
    match text {
        "설정" => "Settings".into(),
        "TokenMeter 설정" => "TokenMeter Settings".into(),
        "TokenMeter 사용량 오버레이" => "TokenMeter usage overlay".into(),
        "esc 닫기" => "esc to close".into(),
        "설정 닫기" => "Close settings".into(),
        "설정 항목" => "Setting".into(),
        "언어" => "Language".into(),
        "모양" => "Appearance".into(),
        "모양과 접근성" => "Appearance".into(),
        "크기" => "Size".into(),
        "팀" => "Team".into(),
        "리그" => "League".into(),
        "데이터" => "Data".into(),
        "한국어" => "한국어".into(),
        "English" => "English".into(),
        "시스템" => "System".into(),
        "시스템 테마" => "System".into(),
        "다크" => "Dark".into(),
        "라이트" => "Light".into(),
        "투명도 줄이기" => "Reduce transparency".into(),
        "모션 줄이기" => "Reduce motion".into(),
        "항상 위" => "Always on top".into(),
        "항상 위 켜기" => "Always on top".into(),
        "항상 위 끄기" => "Unpin from top".into(),
        "미니 모드" => "Mini mode".into(),
        "미니 투명도" => "Mini transparency".into(),
        "약하게" => "Subtle".into(),
        "보통" => "Medium".into(),
        "강하게" => "Strong".into(),
        "마우스 올리면 선명하게" => "Sharpen on hover".into(),
        "미니 모드 (한 줄만)" => "Mini mode".into(),
        "미니 모드 해제" => "Exit mini mode".into(),
        "최소화" => "Compact look".into(),
        "기본 미터" => "Classic".into(),
        "픽셀 다이얼" => "Pixel dial".into(),
        "픽셀 코인" => "Pixel coin".into(),
        "요금" => "FARE".into(),
        "기본" => "Reset".into(),
        "기본 크기 (100%)" => "Reset size (100%)".into(),
        "크게  +10%" => "Larger  +10%".into(),
        "작게  −10%" => "Smaller  −10%".into(),
        "참가한 방이 없습니다" => "No rooms joined".into(),
        "초대 링크 복사" => "Copy invite link".into(),
        "이 방 나가기" => "Leave this room".into(),
        "이 방 닫기" => "Close this room".into(),
        "환산 단가 입력…" => "Set conversion prices…".into(),
        "환산 단가 입력" => "Set conversion prices".into(),
        "통계 초기화" => "Reset stats".into(),
        "오버레이 숨기기 · 측정 계속" => "Hide overlay · keep measuring".into(),
        "TokenMeter 종료 · 측정 중지" => "Quit TokenMeter · stop measuring".into(),
        "TokenMeter 열기" => "Open TokenMeter".into(),
        "TokenMeter 접기" => "Fold TokenMeter".into(),
        "숫자: 속도" => "Number: Speed".into(),
        "숫자: 오늘 비용" => "Number: Today's cost".into(),
        "메뉴바 미터: 항상 표시" => "Menu bar meter: Always".into(),
        "메뉴바 미터: 창을 접었을 때만" => "Menu bar meter: When folded".into(),
        "전체화면·모든 데스크톱에 표시" => "Show over full screen and all desktops".into(),
        "오늘/누적 · S/M/L · 설정" => "Today/Total · S/M/L · Settings".into(),
        "오늘/누적 · " => "Today/Total · ".into(),
        "S/M/L · ⋯ 메뉴" => "S/M/L · ⋯ menu".into(),
        "오늘" => "Today".into(),
        "누적" => "Total".into(),
        "세션" => "Sessions".into(),
        "프로젝트" => "Projects".into(),
        "한도" => "Quota".into(),
        "속도" => "Speed".into(),
        "일별" => "Days".into(),
        "실시간" => "Live".into(),
        "보관" => "Archive".into(),
        "전체" => "All".into(),
        "상태" => "State".into(),
        "모델" => "Model".into(),
        "메인" => "Main".into(),
        "누적 토큰" => "Tokens".into(),
        "컨텍스트" => "Context".into(),
        "시각" => "Time".into(),
        "서비스" => "Service".into(),
        "기간" => "Window".into(),
        "사용" => "Used".into(),
        "페이스" => "Pace".into(),
        "리셋" => "Reset".into(),
        "프로바이더" => "Provider".into(),
        "메인 모델" => "Main model".into(),
        "최근 세션" => "Latest".into(),
        "확인" => "Check".into(),
        "작업" => "Work".into(),
        "대기" => "Wait".into(),
        "종료" => "Done".into(),
        "권한" => "Permission".into(),
        "질문" => "Question".into(),
        "중지" => "Stop".into(),
        "알림" => "Notice".into(),
        "입력" => "In".into(),
        "출력" => "Out".into(),
        "캐시" => "Cache".into(),
        "절감" => "Saved".into(),
        "전체 출력" => "All output".into(),
        "출력 흐름" => "Output flow".into(),
        "영수증" => "Receipt".into(),
        "7일" => "7d".into(),
        "30일" => "30d".into(),
        "1시간" => "1h".into(),
        "4시간" => "4h".into(),
        "1일" => "1d".into(),
        "홈 폴더" => "Home".into(),
        "폴더 미상" => "Unknown folder".into(),
        "측정 전" => "pending".into(),
        "미상" => "n/a".into(),
        "높음" => "high".into(),
        "창?" => "win?".into(),
        "진행 중" => "live".into(),
        "현재" => "Now".into(),
        "보기" => "View".into(),
        "상세" => "More".into(),
        "필터" => "Filter".into(),
        "명령" => "Cmd".into(),
        "라이브" => "Live".into(),
        "호스트" => "host".into(),
        "실시간 세션" => "Live sessions".into(),
        "보관 세션" => "Archived sessions".into(),
        "모든 세션" => "All sessions".into(),
        "미터만 보기" => "Meter only".into(),
        "기본 보기" => "Default view".into(),
        "상세 보기" => "Detailed view".into(),
        "누적 보기" => "Show total".into(),
        "오늘 보기" => "Show today".into(),
        "빠른 설정" => "Settings".into(),
        "세션 또는 명령 검색" => "Search sessions or commands".into(),
        "세션 또는 명령 검색 · Command K" => "Search sessions or commands · Command K".into(),
        "오늘과 누적 전환" => "Toggle today and total".into(),
        "전체 시간 범위로 돌아가기" => "Back to full range".into(),
        "세션 영수증 복사" => "Copy session receipt".into(),
        "검색 결과" => "Search result".into(),
        "작은 미터" => "Compact meter".into(),
        "기본 미터와 패널" => "Meter and panels".into(),
        "넓은 상세 보기" => "Wide detail view".into(),
        "한도 패널 보기" => "Open quota panel".into(),
        "목록 항목" => "List item".into(),
        "메인 속도 없음" => "No main speed".into(),
        "모델 미상" => "Unknown model".into(),
        "세션 상세" => "Session detail".into(),
        "현재 대표 구독" => "Current plan chip".into(),
        "대표 구독으로 설정" => "Set as plan chip".into(),
        "사용량 미상" => "Usage unknown".into(),
        "페이스 여유 없음" => "No pace slack".into(),
        "리셋 시각 미상" => "Reset time unknown".into(),
        "↑↓ 이동 · ↵ 열기 · esc 닫기" => "↑↓ move · ↵ open · esc close".into(),
        "일치하는 세션이나 명령이 없습니다" => "No matching sessions or commands".into(),
        "메인 모델 출력 처리량 · tok/s" => "Main-model output · tok/s".into(),
        "시간별 API 환산 비용 · USD" => "Hourly API-equivalent cost · USD".into(),
        "아직 쌓인 시간이 없습니다 — 한 시간이 지나면 그려집니다" => "Nothing to chart yet — bars appear after an hour".into(),
        "작업이 이어지면 속도가 쌓입니다" => "Speed builds as output continues".into(),
        "환산" => "est.".into(),
        "API 환산" => "API est.".into(),
        "비용" => "Cost".into(),
        "상태 읽기 실패 · tokenmeter doctor" => "Failed to read status · tokenmeter doctor".into(),
        "첫 세션 대기 중 · 에이전트를 재시작하세요" => "Waiting for first session · restart an agent".into(),
        "측정이 멈춤 · tokenmeter doctor" => "Meter stalled · tokenmeter doctor".into(),
        "초대 링크를 복사했습니다" => "Invite link copied".into(),
        "방을 여는 중…" => "Opening a room…".into(),
        "방을 열지 못했습니다" => "Could not open a room".into(),
        "영수증을 복사했습니다" => "Receipt copied".into(),
        "동기화 중…" => "Syncing…".into(),
        "동기화 완료" => "Synced".into(),
        "통계를 초기화했습니다" => "Stats reset".into(),
        "단가를 저장하지 못했습니다" => "Could not save prices".into(),
        "0 이상의 숫자 4개를 쉼표로 구분하세요." => "Enter four non-negative numbers, comma-separated.".into(),
        "누적 통계와 로컬 히스토리를 모두 초기화할까요? 이 작업은 되돌릴 수 없습니다." => "Reset all totals and local history? This cannot be undone.".into(),
        "USD / 100만 토큰\n입력, 캐시 읽기, 캐시 쓰기, 출력" => "USD / 1M tokens\ninput, cache read, cache write, output".into(),
        "실시간 세션 없음 — 실행 중인 에이전트가 여기에 표시됩니다" => "No live sessions — running agents show up here".into(),
        "GOGOGO! 프롬프트 하나면 미터가 살아납니다" => "GOGOGO! One prompt and the meter wakes up".into(),
        "아직 조용하다. 코딩 에이전트를 한 번 굴려 보세요" => "Quiet for now. Run a coding agent once".into(),
        "보관 세션 없음 — 종료된 에이전트가 여기에 표시됩니다" => "No archived sessions — finished agents land here".into(),
        "끝난 대화는 여기로 내려옵니다. 지금 한 판 더?" => "Finished chats drop here. Another round?".into(),
        "GOGOGO! 새 세션이 끝나면 보관이 채워집니다" => "GOGOGO! Archives fill when a session ends".into(),
        "기록된 세션 없음 — 세션은 에이전트 대화 하나입니다" => "No sessions yet — one session is one agent chat".into(),
        "GOGOGO! 에이전트를 재시작하고 프롬프트를 보내 보세요" => "GOGOGO! Restart an agent and send a prompt".into(),
        "미터가 기다립니다. 첫 토큰이 오는 순간 여기가 켜집니다" => "The meter is waiting. It lights up on the first token".into(),
        "에이전트를 실행한 폴더가 아직 없습니다" => "No folders have run an agent yet".into(),
        "GOGOGO! 프로젝트 폴더에서 에이전트를 한 번 돌리면 쌓입니다" => "GOGOGO! Run an agent from a project folder".into(),
        "폴더는 실행 위치입니다. 지금 그 디렉터리에서 시작해 보세요" => "A folder is the run location. Start from that directory".into(),
        "자격 없음 · Claude/Codex/Grok 로그인" => "No credentials · sign in to Claude/Codex/Grok".into(),
        "한도는 로그인한 플랜에서 읽습니다. GOGOGO!" => "Quota comes from the signed-in plan. GOGOGO!".into(),
        "로그인된 CLI가 있으면 잔여 창이 여기 뜹니다" => "A signed-in CLI will show remaining windows here".into(),
        "아직 작업 속도 기록이 없습니다" => "No speed history yet".into(),
        "GOGOGO! 출력이 흐르면 모델별 속도가 쌓입니다" => "GOGOGO! Output fills per-model speed".into(),
        "메인 모델이 토큰을 뱉는 순간 여기가 움직입니다" => "This moves when the main model emits tokens".into(),
        "히스토리 없음 — 하루가 지나면 쌓입니다" => "No history yet — it appears after a day".into(),
        "GOGOGO! 오늘을 쓰면 내일 여기 막대가 생깁니다" => "GOGOGO! Use today and bars show up tomorrow".into(),
        "한 시간이 지나면 그래프가 그려집니다" => "The chart draws after an hour".into(),
        "팀을 보려면 로그인이 필요합니다" => "Sign in to see the team".into(),
        "리그를 보려면 로그인이 필요합니다" => "Sign in to see the league".into(),
        "GOGOGO! 터미널에서 tokenmeter league login" => "GOGOGO! Run tokenmeter league login".into(),
        "로그인 한 줄이면 초대 링크를 만들 수 있습니다" => "One login and you can make an invite link".into(),
        "아직 참가한 방이 없습니다" => "You have not joined a room yet".into(),
        "GOGOGO! 초대 링크를 만들거나 받은 링크를 열어 보세요" => "GOGOGO! Copy an invite or open one you received".into(),
        "설정에서 초대 링크를 복사하면 방이 열립니다" => "Copy an invite in Settings to open a room".into(),
        "아직 팀 데이터가 없습니다" => "No team data yet".into(),
        "아직 리그 데이터가 없습니다" => "No league data yet".into(),
        "GOGOGO! 링크를 공유하면 게이지에 눈금이 생깁니다" => "GOGOGO! Share a link and ticks appear on the gauge".into(),
        "친구가 들어오는 순간 여기가 레이스가 됩니다" => "The moment a friend joins, this becomes a race".into(),
        "아직 측정된 폴더 없음 — 프로젝트는 에이전트를 실행한 폴더입니다" => "No measured folders — a project is the folder that ran the agent".into(),
        "현재 에이전트 상태와 컨텍스트" => "Live agent state and context".into(),
        "측정 토큰 · 최근 세션 순" => "Measured tokens · latest session".into(),
        "모델별 출력 처리량" => "Output throughput by model".into(),
        "오늘 · 7일 · 30일" => "Today · 7d · 30d".into(),
        "사용량 히스토리" => "Usage history".into(),
        "플랜 한도" => "Plan quota".into(),
        "Claude · Codex · Grok" => "Claude · Codex · Grok".into(),
        "공유 상태와 사용량" => "Shared status and usage".into(),
        "Token League · 초대와 사용량" => "Token League · invites and usage".into(),
        "현재 실행 중인 에이전트" => "Agents running now".into(),
        "종료된 에이전트" => "Finished agents".into(),
        "종료된 기록 포함" => "Includes finished history".into(),
        "콘텐츠 목록 접기" => "Collapse the list".into(),
        "미터와 핵심 목록" => "Meter plus the core list".into(),
        "넓은 히스토리와 더 많은 결과" => "Wider history and more rows".into(),
        "사용량 범위 전환" => "Toggle usage range".into(),
        "모양 · 동작 · 창 크기" => "Look, behavior, window size".into(),
        "비용은 로그 토큰과 공개 API 단가로 계산한 예상값입니다." => "Cost is an estimate from logged tokens and public API prices.".into(),
        "갱신 시각 미상" => "Updated time unknown".into(),
        "방금 갱신" => "Just updated".into(),
        "방금" => "now".into(),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_swaps_settings_title() {
        assert_eq!(tr("ko", "설정"), "설정");
        assert_eq!(tr("en", "설정"), "Settings");
        assert_eq!(tr("en", "TokenMeter 접기"), "Fold TokenMeter");
        assert_eq!(normalize("EN"), "en");
    }

    #[test]
    fn l10n_picks_by_language() {
        let (_g, _t) = crate::test_home("l10n");
        std::env::set_var("TOKENMETER_LANG", "en");
        assert_eq!(crate::l10n!("{n} services", "서비스 {n}개", n = 3), "3 services");
        assert_eq!(crate::l10n!("Off", "꺼짐"), "Off");
        std::env::remove_var("TOKENMETER_LANG");
        assert_eq!(crate::l10n!("Off", "꺼짐"), "꺼짐", "0.1.x처럼 한국어가 기본(10.1이 바꿈)");
        assert_eq!(crate::l10n!("{n} services", "서비스 {n}개", n = 3), "서비스 3개");
        let n = 2;
        assert_eq!(crate::l10n!("{n} a", "{n} 가"), "2 가", "인자 없어도 format!처럼 변수를 잡는다");
        assert_eq!(crate::l10n!("{{x}}", "{{x}}",), "{x}");
    }
}


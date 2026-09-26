//! Token League 실시간 연결. 데몬 안의 스레드 하나가 tokio current-thread 런타임으로 iroh 엔드포인트를 돌린다.
//! 포커스된 방의 멤버마다 내가 건 연결로 내 tok/s를 보내고, 상대가 건 연결로 상대 값을 받는다.
//! 한 쌍에 연결이 두 개라 중복을 정리할 필요가 없다. 받은 줄은 연결 상대의 EndpointId(TLS로 확인된 키)로
//! 멤버를 찾으므로 남의 행을 위조할 수 없고, 시각은 받는 쪽이 찍어 시계가 어긋나도 된다.

use iroh::endpoint::{presets, Connection, Incoming};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokenmeter_protocol::{LiveLine, LIVE_ALPN, MAX_LINE};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::watch;

/// 모르는 상대가 연결하면 방 목록이 새로 올 때까지 이만큼 기다린 뒤 끊는다.
/// 데몬의 새로 고침 간격(60초) + 확인 주기(5초) + HTTP 여유. 새로 들어온 멤버를 확인 전에 끊지 않게.
const UNKNOWN_WAIT: Duration = if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(80) };
/// tok/s가 그대로여도 이 간격으로 다시 보낸다(오버레이는 15초 넘은 행을 숨긴다).
const RESEND: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    pub uid: String,
    pub addr: EndpointAddr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub uid: String,
    pub tps: f64,
    pub at: f64,
}

type Peers = HashMap<EndpointId, Peer>;
type Rows = Arc<Mutex<HashMap<EndpointId, Row>>>;

/// 떨어뜨리면(drop) 엔드포인트가 닫히고 스레드가 끝난다.
pub struct Live {
    id: EndpointId,
    addr: EndpointAddr,
    peers: watch::Sender<Peers>,
    tps: watch::Sender<f64>,
    rows: Rows,
    unknown: Arc<AtomicBool>,
}

impl Live {
    /// 엔드포인트를 띄운다. `relay`가 없으면 직접 주소로만 붙는다(테스트용).
    pub fn start(key: SecretKey, relay: Option<RelayUrl>) -> Result<Live, String> {
        let (ready, is_ready) = std::sync::mpsc::channel();
        let (peers, peers_rx) = watch::channel(Peers::new());
        let (tps, tps_rx) = watch::channel(0.0);
        let rows = Rows::default();
        let unknown = Arc::new(AtomicBool::new(false));
        let (rows_in, unknown_in) = (rows.clone(), unknown.clone());
        std::thread::Builder::new()
            .name("league-live".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => return drop(ready.send(Err(e.to_string()))),
                };
                rt.block_on(async move {
                    let mode = relay.map_or(RelayMode::Disabled, |url| RelayMode::Custom(RelayMap::from(url)));
                    let mut builder = Endpoint::builder(presets::Minimal)
                        .secret_key(key)
                        .alpns(vec![LIVE_ALPN.to_vec()])
                        .relay_mode(mode);
                    // 테스트는 루프백에만 묶는다. 방화벽 허용 창이 뜨지 않고, 주소에 127.0.0.1:port가 실린다.
                    if cfg!(test) {
                        builder = match builder.clear_ip_transports().bind_addr("127.0.0.1:0") {
                            Ok(b) => b,
                            Err(e) => return drop(ready.send(Err(e.to_string()))),
                        };
                    }
                    let bound = builder.bind().await;
                    match bound {
                        Ok(ep) => {
                            let _ = ready.send(Ok((ep.id(), ep.addr())));
                            run(ep, peers_rx, tps_rx, rows_in, unknown_in).await;
                        }
                        Err(e) => drop(ready.send(Err(e.to_string()))),
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        let (id, addr) = is_ready.recv().map_err(|e| e.to_string())??;
        Ok(Live { id, addr, peers, tps, rows, unknown })
    }

    pub fn id(&self) -> EndpointId {
        self.id
    }

    /// 바인드 직후의 내 주소(직접 주소 포함). 릴레이 없이 붙는 테스트에서 쓴다.
    pub fn addr(&self) -> EndpointAddr {
        self.addr.clone()
    }

    pub fn set_tps(&self, tps: f64) {
        self.tps.send_replace(tps);
    }

    /// 연결할 멤버. 나 자신은 빼고, 빠진 멤버의 연결과 행은 정리한다.
    pub fn set_peers(&self, peers: Vec<Peer>) {
        let next: Peers = peers.into_iter().filter(|p| p.addr.id != self.id).map(|p| (p.addr.id, p)).collect();
        self.rows.lock().unwrap().retain(|id, _| next.contains_key(id));
        self.peers.send_if_modified(|cur| {
            let changed = *cur != next;
            *cur = next;
            changed
        });
    }

    /// 받은 행. 15초가 넘은 행은 부르는 쪽이 거른다.
    pub fn rows(&self) -> Vec<Row> {
        self.rows.lock().unwrap().values().cloned().collect()
    }

    /// 멤버 목록에 없는 상대가 연결했었는지. 한 번 읽으면 지운다.
    pub fn take_unknown(&self) -> bool {
        self.unknown.swap(false, Ordering::Relaxed)
    }
}

async fn run(ep: Endpoint, mut peers: watch::Receiver<Peers>, tps: watch::Receiver<f64>, rows: Rows, unknown: Arc<AtomicBool>) {
    let mut out: HashMap<EndpointId, tokio::task::AbortHandle> = HashMap::new();
    loop {
        tokio::select! {
            incoming = ep.accept() => {
                let Some(incoming) = incoming else { break };
                tokio::spawn(receive(incoming, peers.clone(), rows.clone(), unknown.clone()));
            }
            changed = peers.changed() => {
                if changed.is_err() {
                    break;
                }
                let now = peers.borrow_and_update().clone();
                out.retain(|id, task| {
                    let keep = now.contains_key(id);
                    if !keep {
                        task.abort();
                    }
                    keep
                });
                for (id, peer) in now {
                    out.entry(id).or_insert_with(|| tokio::spawn(send(ep.clone(), peer.addr, tps.clone())).abort_handle());
                }
            }
        }
    }
    for task in out.values() {
        task.abort();
    }
    ep.close().await;
}

/// 내가 건 연결로 내 tok/s를 보낸다. 끊기면 1초에서 60초까지 늘려 가며 다시 붙는다.
async fn send(ep: Endpoint, addr: EndpointAddr, tps: watch::Receiver<f64>) {
    let mut wait = 1;
    loop {
        if let Ok(conn) = ep.connect(addr.clone(), LIVE_ALPN).await {
            wait = 1;
            stream(&conn, &tps).await;
        }
        tokio::time::sleep(Duration::from_secs(wait)).await;
        wait = (wait * 2).min(60);
    }
}

/// tok/s가 1 이상 바뀌면 최대 초당 한 번, 그대로면 5초마다 한 줄. 10초 뒤 경로(직접·릴레이)를 한 번 적는다.
async fn stream(conn: &Connection, tps: &watch::Receiver<f64>) {
    let Ok(mut out) = conn.open_uni().await else { return };
    let started = Instant::now();
    let (mut last, mut sent_at, mut logged) = (f64::NAN, started, false);
    loop {
        let now = *tps.borrow();
        if last.is_nan() || (now - last).abs() >= 1.0 || sent_at.elapsed() >= RESEND {
            let mut line = serde_json::to_vec(&LiveLine { tps: Some(now) }).unwrap_or_default();
            line.push(b'\n');
            if out.write_all(&line).await.is_err() {
                return;
            }
            (last, sent_at) = (now, Instant::now());
        }
        if !logged && started.elapsed() >= Duration::from_secs(10) {
            logged = true;
            let direct = conn.paths().iter().any(|p| p.is_selected() && p.is_ip());
            eprintln!("[league] {} {}", conn.remote_id().fmt_short(), if direct { "direct" } else { "relay" });
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// 상대가 건 연결에서 줄을 읽는다. 방 목록을 새로 받고도 멤버가 아니면 끊고, 1 KB 넘는 줄에서도 끊는다.
async fn receive(incoming: Incoming, mut peers: watch::Receiver<Peers>, rows: Rows, unknown: Arc<AtomicBool>) {
    let Ok(conn) = incoming.await else { return };
    let id = conn.remote_id();
    if !peers.borrow().contains_key(&id) {
        unknown.store(true, Ordering::Relaxed);
        let known = tokio::time::timeout(UNKNOWN_WAIT, peers.wait_for(|p| p.contains_key(&id))).await.is_ok_and(|r| r.is_ok());
        if !known {
            conn.close(1u32.into(), b"not a member");
            return;
        }
    }
    let Ok(stream) = conn.accept_uni().await else { return };
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE as u64).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) if line.last() != Some(&b'\n') => {
                conn.close(2u32.into(), b"line too long");
                return;
            }
            Ok(_) => {}
        }
        let Some(uid) = peers.borrow().get(&id).map(|p| p.uid.clone()) else {
            conn.close(1u32.into(), b"not a member");
            return;
        };
        let tps = serde_json::from_slice::<LiveLine>(&line).ok().and_then(|l| l.tps);
        if let Some(tps) = tps.filter(|t| t.is_finite() && *t >= 0.0) {
            rows.lock().unwrap().insert(id, Row { uid, tps, at: crate::watch::now_secs() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !ok() {
            assert!(Instant::now() < end, "시간 초과: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn peer(uid: &str, live: &Live) -> Peer {
        Peer { uid: uid.into(), addr: live.addr() }
    }

    #[test]
    fn members_exchange_rates_without_a_relay() {
        let a = Live::start(SecretKey::generate(), None).unwrap();
        let b = Live::start(SecretKey::generate(), None).unwrap();
        a.set_peers(vec![peer("1", &a), peer("2", &b)]);
        b.set_peers(vec![peer("1", &a)]);
        a.set_tps(12.34);
        b.set_tps(3.0);
        wait_until("b가 a의 값을 받음", || b.rows().iter().any(|r| r.uid == "1" && r.tps == 12.34));
        wait_until("a가 b의 값을 받음", || a.rows().iter().any(|r| r.uid == "2" && r.tps == 3.0));
        assert!(a.rows().iter().all(|r| r.uid != "1"), "나 자신에게는 붙지 않는다");
        b.set_peers(Vec::new());
        assert!(b.rows().is_empty(), "빠진 멤버의 행은 지운다");
    }

    #[test]
    fn strangers_are_dropped_after_a_room_refresh() {
        let a = Live::start(SecretKey::generate(), None).unwrap();
        let c = Live::start(SecretKey::generate(), None).unwrap();
        c.set_peers(vec![peer("1", &a)]);
        c.set_tps(9.0);
        wait_until("a가 모르는 상대를 알아챔", || a.take_unknown());
        std::thread::sleep(UNKNOWN_WAIT * 4);
        assert!(a.rows().is_empty(), "멤버가 아닌 상대의 값은 받지 않는다");
    }

    #[test]
    fn a_line_over_1kb_closes_the_connection() {
        let a = Live::start(SecretKey::generate(), None).unwrap();
        let key = SecretKey::generate();
        a.set_peers(vec![Peer { uid: "9".into(), addr: EndpointAddr::new(key.public()) }]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let ep = Endpoint::builder(presets::Minimal).secret_key(key).clear_ip_transports().bind_addr("127.0.0.1:0").unwrap().bind().await.unwrap();
            let conn = ep.connect(a.addr(), LIVE_ALPN).await.unwrap();
            let mut out = conn.open_uni().await.unwrap();
            let mut long = vec![b'x'; MAX_LINE + 10];
            long.push(b'\n');
            out.write_all(&long).await.unwrap();
            let closed = tokio::time::timeout(Duration::from_secs(10), conn.closed()).await;
            assert!(closed.is_ok(), "1 KB 넘는 줄에서 끊는다");
        });
        assert!(a.rows().is_empty());
    }
}

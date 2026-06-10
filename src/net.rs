use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::mpsc as smpsc;
use tokio::sync::mpsc as tmpsc;

pub const ALPN: &[u8] = b"de-micro/1";
const MAX_FRAME: u32 = 1 << 20;

// ---------- wire types ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum C2S {
    Hello { name: String, want_t: bool },
    Input(PlayerInput),
    Shot(ShotMsg),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub weapon: u8,
    pub e_held: bool,
    pub alive_seq: u32, // echoes last spawn seq applied, so host ignores stale inputs
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShotMsg {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub pistol: bool,
    pub hit: Option<HitClaim>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum HitClaim {
    Player { id: u8, dmg: i32 },
    Bot { idx: u8, dmg: i32 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum S2C {
    Welcome { id: u8, team_t: bool, ff: bool },
    Spawn { pos: [f32; 3], yaw: f32, seq: u32 },
    Snap(Snapshot),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlayerNet {
    pub id: u8,
    pub name: String,
    pub team_t: bool,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub hp: i32,
    pub carrier: bool,
    pub progress: f32, // own plant/defuse progress 0..1, only meaningful for that player
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BotNet {
    pub name: String,
    pub team_t: bool,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub carrier: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Event {
    Kill { killer: String, killer_ct: bool, victim: String },
    Tracer { from: [f32; 3], to: [f32; 3] },
    Planted,
    Defused { by: String },
    Explosion { pos: [f32; 3] },
    RoundEnd { ct_won: bool, reason: String },
    RoundMsg { round: i32 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Snapshot {
    pub players: Vec<PlayerNet>,
    pub bots: Vec<BotNet>,
    pub bomb: Option<([f32; 3], f32, bool)>, // pos, time left, defused
    pub dropped: Option<[f32; 3]>,
    pub phase: u8, // 0 freeze, 1 live, 2 post
    pub round_time: f32,
    pub score_ct: i32,
    pub score_t: i32,
    pub round: i32,
    pub events: Vec<Event>,
}

// ---------- framing ----------

fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = postcard::to_allocvec(msg).expect("encode");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Option<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await.ok()?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    recv.read_exact(&mut buf).await.ok()?;
    Some(buf)
}

pub fn ticket_from_addr(addr: &EndpointAddr) -> String {
    let bytes = postcard::to_allocvec(addr).expect("addr encode");
    data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
}

pub fn addr_from_ticket(ticket: &str) -> Option<EndpointAddr> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(ticket.trim().to_uppercase().as_bytes())
        .ok()?;
    postcard::from_bytes(&bytes).ok()
}

// ---------- host ----------

pub enum HostEvent {
    Ticket(String),
    Joined { id: u8, name: String, want_t: bool },
    Msg { id: u8, msg: C2S },
    Left { id: u8 },
}

pub struct HostNet {
    pub events: smpsc::Receiver<HostEvent>,
    out: tmpsc::UnboundedSender<(Option<u8>, S2C)>,
}

impl HostNet {
    pub fn send_to(&self, id: u8, msg: S2C) {
        let _ = self.out.send((Some(id), msg));
    }
    pub fn broadcast(&self, msg: S2C) {
        let _ = self.out.send((None, msg));
    }
}

pub fn start_host() -> HostNet {
    let (ev_tx, ev_rx) = smpsc::channel::<HostEvent>();
    let (out_tx, mut out_rx) = tmpsc::unbounded_channel::<(Option<u8>, S2C)>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let ep = match Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
            {
                Ok(ep) => ep,
                Err(e) => {
                    eprintln!("host: failed to bind endpoint: {e}");
                    return;
                }
            };
            ep.online().await;
            let ticket = ticket_from_addr(&ep.addr());
            let _ = ev_tx.send(HostEvent::Ticket(ticket));

            let conns: std::sync::Arc<tokio::sync::Mutex<HashMap<u8, tmpsc::UnboundedSender<S2C>>>> =
                Default::default();

            // router: outgoing game messages -> per-connection writers
            {
                let conns = conns.clone();
                tokio::spawn(async move {
                    while let Some((target, msg)) = out_rx.recv().await {
                        let map = conns.lock().await;
                        match target {
                            Some(id) => {
                                if let Some(tx) = map.get(&id) {
                                    let _ = tx.send(msg);
                                }
                            }
                            None => {
                                for tx in map.values() {
                                    let _ = tx.send(msg.clone());
                                }
                            }
                        }
                    }
                });
            }

            let mut next_id: u8 = 1;
            loop {
                let incoming = match ep.accept().await {
                    Some(i) => i,
                    None => break,
                };
                let conn = match incoming.await {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let id = next_id;
                next_id = next_id.wrapping_add(1).max(1);
                let ev_tx = ev_tx.clone();
                let conns = conns.clone();
                tokio::spawn(async move {
                    let (mut send, mut recv) = match conn.accept_bi().await {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    // first frame must be Hello
                    let hello = match read_frame(&mut recv).await {
                        Some(buf) => match postcard::from_bytes::<C2S>(&buf) {
                            Ok(C2S::Hello { name, want_t }) => (name, want_t),
                            _ => return,
                        },
                        None => return,
                    };
                    let (wtx, mut wrx) = tmpsc::unbounded_channel::<S2C>();
                    conns.lock().await.insert(id, wtx);
                    let _ = ev_tx.send(HostEvent::Joined {
                        id,
                        name: hello.0,
                        want_t: hello.1,
                    });

                    let writer = tokio::spawn(async move {
                        while let Some(msg) = wrx.recv().await {
                            if send.write_all(&encode(&msg)).await.is_err() {
                                break;
                            }
                        }
                    });

                    while let Some(buf) = read_frame(&mut recv).await {
                        if let Ok(msg) = postcard::from_bytes::<C2S>(&buf) {
                            if ev_tx.send(HostEvent::Msg { id, msg }).is_err() {
                                break;
                            }
                        }
                    }
                    writer.abort();
                    conns.lock().await.remove(&id);
                    let _ = ev_tx.send(HostEvent::Left { id });
                });
            }
        });
    });

    HostNet {
        events: ev_rx,
        out: out_tx,
    }
}

// ---------- client ----------

pub enum ClientEvent {
    Connected,
    Msg(S2C),
    Disconnected(String),
}

pub struct ClientNet {
    pub events: smpsc::Receiver<ClientEvent>,
    out: tmpsc::UnboundedSender<C2S>,
}

impl ClientNet {
    pub fn send(&self, msg: C2S) {
        let _ = self.out.send(msg);
    }
}

pub fn start_client(ticket: String, name: String, want_t: bool) -> ClientNet {
    let (ev_tx, ev_rx) = smpsc::channel::<ClientEvent>();
    let (out_tx, mut out_rx) = tmpsc::unbounded_channel::<C2S>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let addr = match addr_from_ticket(&ticket) {
                Some(a) => a,
                None => {
                    let _ = ev_tx.send(ClientEvent::Disconnected("bad ticket".into()));
                    return;
                }
            };
            let ep = match Endpoint::builder(presets::N0).bind().await {
                Ok(ep) => ep,
                Err(e) => {
                    let _ = ev_tx.send(ClientEvent::Disconnected(format!("bind: {e}")));
                    return;
                }
            };
            let conn = match ep.connect(addr, ALPN).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = ev_tx.send(ClientEvent::Disconnected(format!("connect: {e}")));
                    return;
                }
            };
            let (mut send, mut recv) = match conn.open_bi().await {
                Ok(s) => s,
                Err(e) => {
                    let _ = ev_tx.send(ClientEvent::Disconnected(format!("stream: {e}")));
                    return;
                }
            };
            if send
                .write_all(&encode(&C2S::Hello { name, want_t }))
                .await
                .is_err()
            {
                let _ = ev_tx.send(ClientEvent::Disconnected("handshake failed".into()));
                return;
            }
            let _ = ev_tx.send(ClientEvent::Connected);

            let writer = tokio::spawn(async move {
                while let Some(msg) = out_rx.recv().await {
                    if send.write_all(&encode(&msg)).await.is_err() {
                        break;
                    }
                }
            });

            while let Some(buf) = read_frame(&mut recv).await {
                if let Ok(msg) = postcard::from_bytes::<S2C>(&buf) {
                    if ev_tx.send(ClientEvent::Msg(msg)).is_err() {
                        break;
                    }
                }
            }
            writer.abort();
            let _ = ev_tx.send(ClientEvent::Disconnected("connection lost".into()));
        });
    });

    ClientNet {
        events: ev_rx,
        out: out_tx,
    }
}

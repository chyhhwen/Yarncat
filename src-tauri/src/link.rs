/* ─── 串門子的區網連線：找人（UDP 廣播）、配對（兩邊互相勾選）、連線（TCP），再把訊息轉給前端 ───
   串門子本身（誰出門、客人貓、對帳）全部在前端（index.html 的「串門子」），跟網頁版共用。
   這裡不看訊息內容：一行一則 JSON，收到就交給前端，欄位檢查交給前端的 onLink。
   不經過伺服器；只有「連線」打開時才會開 port。 */

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::Serialize;
use serde_json::{json, Value};

/// UDP 廣播、TCP 連線都用這個 port
pub const PORT: u16 = 52525;

/// 廣播、握手的格式版本（Rust 之間用的；前端訊息的版本是 index.html 的 LINK_V）。不一樣就當作沒看到
const V: u64 = 1;
const APP: &str = "yarncat";

/// 每隔多久廣播一次「我在這裡」
const BEACON_EVERY: Duration = Duration::from_secs(2);

/// 多久沒收到廣播，就從「找到的電腦」拿掉
const FOUND_TTL: Duration = Duration::from_secs(7);

/// 同一台電腦從別的網卡也收得到時（有線、無線都接著），原本的位址這麼久沒消息才換過去
const ADDR_SWITCH: Duration = Duration::from_secs(5);

/// 收到廣播馬上回一則，但同一台每秒最多回一次（每張網卡的廣播都會收到一份）
const REPLY_EVERY: Duration = Duration::from_secs(1);

/// 一行最多幾個位元組，超過就斷線（正常的訊息不到 300 個位元組）
const MAX_LINE: usize = 4096;

/// 兩邊都勾好之後由代號小的那邊撥；過了這麼久還沒連上，代號大的那邊也撥。
/// 防火牆只擋住其中一台時，由另一台撥出去還連得上
const DIAL_FALLBACK: Duration = Duration::from_secs(5);

/// 撥不通，隔多久再撥
const DIAL_RETRY: Duration = Duration::from_secs(3);

/// 撥號、握手最多等多久
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// 每隔多久送一個空行，讓對方知道線還通著（前端的畫面停住、不送訊息時也一樣）
const KEEPALIVE_EVERY: Duration = Duration::from_secs(3);

/// 多久沒收到任何東西（包括空行）就當作斷線：拔網路線、對方電腦睡著了
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// 收廣播、接電話的迴圈多久跑一次
const TICK: Duration = Duration::from_millis(100);

/// 交給外面（main.rs）的事
pub enum Event {
    /// 隔壁送來一則訊息（已經確定是 JSON 物件）
    Msg(Value),
    /// 連上了、斷了
    Conn,
    /// 找到的電腦、勾選、錯誤訊息有變：狀態視窗要重畫
    Changed,
}

/// 狀態視窗「連線」分頁要顯示的東西
#[derive(Clone, Serialize)]
pub struct Status {
    pub on: bool,
    /// 這台電腦的名稱、IP（給另一台手動輸入用）
    pub name: String,
    pub ips: Vec<String>,
    /// 找到的電腦；勾選的那台不在線上也會列出來
    pub peers: Vec<PeerView>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct PeerView {
    pub id: String,
    pub name: String,
    /// 不在線上時是空字串
    pub ip: String,
    pub chosen: bool,
    pub wants_me: bool,
    pub connected: bool,
}

pub struct Link {
    me: String,
    name: String,
    /// (自己聽的 port, 送廣播的目的 port)：正式版兩個都是 PORT；測試時兩個節點在同一台電腦上，要錯開
    ports: (u16, u16),
    sink: Box<dyn Fn(Event) + Send + Sync>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    on: bool,
    /// 第幾次打開；關掉再打開會換一號，舊的執行緒看到號碼不對就收工
    run: u64,
    /// 勾選的那台：(代號, 電腦名稱)。名稱留著，對方不在線上時清單也列得出來
    peer: Option<(String, String)>,
    /// 手動輸入的 IP：不同網段時廣播傳不過去，改成直接送過去
    ips: Vec<Ipv4Addr>,
    found: HashMap<String, Found>,
    conn: Option<Conn>,
    /// 給連線編號：舊連線收尾時才分得出自己還是不是現在這條
    serial: u64,
    dialing: bool,
    last_dial: Option<Instant>,
    /// 從什麼時候開始兩邊都勾了對方（還沒連上）
    mutual_since: Option<Instant>,
    error: Option<String>,
}

struct Found {
    name: String,
    /// 對方的 TCP 位址：廣播的來源 IP + 廣播裡寫的 port
    addr: SocketAddr,
    /// 最後一次從 addr 收到廣播
    addr_seen: Instant,
    wants_me: bool,
    seen: Instant,
    replied: Option<Instant>,
}

struct Conn {
    peer: String,
    /// 誰撥的。兩邊同時撥通時，兩邊都留下撥號方代號比較小的那條，才會留到同一條
    dialer: String,
    serial: u64,
    stream: TcpStream,
}

impl Link {
    pub fn new(
        me: String,
        name: String,
        ports: (u16, u16),
        sink: impl Fn(Event) + Send + Sync + 'static,
    ) -> Arc<Link> {
        Arc::new(Link { me, name, ports, sink: Box::new(sink), inner: Mutex::default() })
    }

    /// 某個執行緒拿著鎖時當掉，鎖會被標成壞掉；裡面的資料還是能用，照用
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, e: Event) {
        (self.sink)(e);
    }

    /// 打開：開 port、開始廣播、接受連線。關掉：關 port、掛斷，找到的電腦也清掉
    pub fn set_on(self: &Arc<Self>, on: bool) {
        let (run, dropped) = {
            let mut s = self.lock();
            if s.on == on {
                return;
            }
            s.on = on;
            s.run += 1;
            s.found.clear();
            s.error = None;
            s.mutual_since = None;
            (s.run, hang_up(&mut s))
        };
        if on {
            let link = self.clone();
            thread::spawn(move || link.run_loop(run));
        }
        if dropped {
            self.emit(Event::Conn);
        }
        self.emit(Event::Changed);
    }

    /// 勾選（或取消勾選）要跟哪一台連。換人就先掛掉現在這條
    pub fn set_peer(&self, peer: Option<(String, String)>) {
        let dropped = {
            let mut s = self.lock();
            if s.peer.as_ref().map(|p| &p.0) == peer.as_ref().map(|p| &p.0) {
                s.peer = peer;
                return;
            }
            s.peer = peer;
            s.mutual_since = None;
            let other = s.conn.as_ref().is_some_and(|c| Some(&c.peer) != s.peer.as_ref().map(|p| &p.0));
            other && hang_up(&mut s)
        };
        if dropped {
            self.emit(Event::Conn);
        }
        self.emit(Event::Changed);
    }

    pub fn set_ips(&self, ips: Vec<Ipv4Addr>) {
        self.lock().ips = ips;
    }

    /// 找到的電腦叫什麼名字（勾選時要記下來）
    pub fn found_name(&self, id: &str) -> Option<String> {
        self.lock().found.get(id).map(|f| f.name.clone())
    }

    /// 現在連著誰
    pub fn connected(&self) -> Option<String> {
        self.lock().conn.as_ref().map(|c| c.peer.clone())
    }

    /// 送一則訊息給隔壁。沒連上就丟掉：前端只在連上時才送
    pub fn send(&self, msg: &Value) -> Result<(), String> {
        if !msg.is_object() {
            return Err("訊息要是 JSON 物件".into());
        }
        // serde_json 會把字串裡的換行寫成 \n，一則訊息一定只佔一行
        let mut line = msg.to_string();
        if line.len() > MAX_LINE {
            return Err(format!("訊息太長：{} 個位元組", line.len()));
        }
        line.push('\n');
        self.write_line(&line);
        Ok(())
    }

    /// 寫一行給隔壁。寫不出去就掛斷，讀的那邊會收尾、通知前端
    fn write_line(&self, line: &str) {
        let s = self.lock();
        if let Some(c) = &s.conn {
            if (&c.stream).write_all(line.as_bytes()).is_err() {
                let _ = c.stream.shutdown(Shutdown::Both);
            }
        }
    }

    pub fn status(&self) -> Status {
        let s = self.lock();
        let chosen = s.peer.as_ref().map(|p| p.0.as_str());
        let connected = s.conn.as_ref().map(|c| c.peer.as_str());
        let mut peers: Vec<PeerView> = s
            .found
            .iter()
            .map(|(id, f)| PeerView {
                id: id.clone(),
                name: f.name.clone(),
                ip: f.addr.ip().to_string(),
                chosen: chosen == Some(id.as_str()),
                wants_me: f.wants_me,
                connected: connected == Some(id.as_str()),
            })
            .collect();
        if let Some((id, name)) = &s.peer {
            if !s.found.contains_key(id) {
                peers.push(PeerView {
                    id: id.clone(),
                    name: name.clone(),
                    ip: String::new(),
                    chosen: true,
                    wants_me: false,
                    connected: connected == Some(id.as_str()),
                });
            }
        }
        // 勾選的排最前面，其他照名字排，清單才不會每次重畫都換順序
        peers.sort_by(|a, b| b.chosen.cmp(&a.chosen).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id)));
        Status {
            on: s.on,
            name: self.name.clone(),
            ips: local_v4().iter().map(|(ip, _)| ip.to_string()).collect(),
            peers,
            error: s.error.clone(),
        }
    }

    /// 這一號還該不該跑
    fn alive(&self, run: u64) -> bool {
        let s = self.lock();
        s.on && s.run == run
    }

    /// 打開連線後一直跑：收廣播、接電話、定時廣播、該撥就撥。關掉（換號）就收工，port 跟著關掉
    fn run_loop(self: Arc<Self>, run: u64) {
        let listen = self.ports.0;
        let (udp, tcp) = match self.bind(run) {
            Ok(s) => s,
            Err(e) => {
                let msg = if e.kind() == ErrorKind::AddrInUse {
                    format!("port {listen} 被別的程式占用了（是不是開了兩個毛球貓？）")
                } else {
                    format!("開不了 port {listen}：{e}")
                };
                let failed = {
                    let mut s = self.lock();
                    let mine = s.on && s.run == run;
                    if mine {
                        s.error = Some(msg);
                    }
                    mine
                };
                if failed {
                    self.emit(Event::Changed);
                }
                return;
            }
        };
        let mut last_beacon: Option<Instant> = None;
        let mut last_keepalive = Instant::now();
        let mut buf = [0u8; 2048];
        while self.alive(run) {
            loop {
                match udp.recv_from(&mut buf) {
                    Ok((n, from)) => self.on_beacon(&udp, &buf[..n], from),
                    // Windows：之前送去的地方沒人在聽，會在這裡收到一次 ConnectionReset，跳過就好
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                    Err(_) => break,
                }
            }
            while let Ok((stream, _)) = tcp.accept() {
                let link = self.clone();
                thread::spawn(move || link.run_conn(stream, run, None));
            }
            if last_beacon.is_none_or(|t| t.elapsed() >= BEACON_EVERY) {
                last_beacon = Some(Instant::now());
                self.beacon_all(&udp);
                self.prune();
            }
            if last_keepalive.elapsed() >= KEEPALIVE_EVERY {
                last_keepalive = Instant::now();
                self.write_line("\n");
            }
            self.maybe_dial(run);
            thread::sleep(TICK);
        }
    }

    /// UDP、TCP 開同一個 port。剛關掉又馬上打開時，上一輪可能還沒放掉 port，等一下再試
    fn bind(&self, run: u64) -> std::io::Result<(UdpSocket, TcpListener)> {
        let listen = self.ports.0;
        let mut tries = 0;
        loop {
            let sockets = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, listen)).and_then(|udp| {
                udp.set_broadcast(true)?;
                udp.set_nonblocking(true)?;
                let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, listen))?;
                tcp.set_nonblocking(true)?;
                Ok((udp, tcp))
            });
            match sockets {
                Err(e) if e.kind() == ErrorKind::AddrInUse && tries < 10 && self.alive(run) => {
                    tries += 1;
                    thread::sleep(Duration::from_millis(200));
                }
                r => return r,
            }
        }
    }

    /// 廣播的內容：我是誰、叫什麼、勾了誰、TCP 開在哪個 port。re 表示這是回覆，收到的人不用再回
    fn beacon(&self, want: &str, re: bool) -> Vec<u8> {
        json!({ "app": APP, "v": V, "id": self.me, "name": self.name, "want": want, "port": self.ports.0, "re": re })
            .to_string()
            .into_bytes()
    }

    /// 每 2 秒送一次「我在這裡」：送到每張網卡的廣播位址（VMware、VPN 的虛擬網卡也算），
    /// 另外直接送給手動輸入的 IP 和已經找到的電腦（廣播傳不過去的網段、防火牆只放行回覆時也收得到）
    fn beacon_all(&self, udp: &UdpSocket) {
        let (want, mut targets) = {
            let s = self.lock();
            let want = s.peer.as_ref().map(|p| p.0.clone()).unwrap_or_default();
            let mut targets: HashSet<IpAddr> = s.ips.iter().map(|&ip| IpAddr::V4(ip)).collect();
            targets.extend(s.found.values().map(|f| f.addr.ip()));
            (want, targets)
        };
        targets.insert(IpAddr::V4(Ipv4Addr::BROADCAST));
        targets.extend(local_v4().into_iter().filter_map(|(_, b)| b.map(IpAddr::V4)));
        let msg = self.beacon(&want, false);
        for ip in targets {
            let _ = udp.send_to(&msg, (ip, self.ports.1));
        }
    }

    /// 收到一則廣播：記下這台電腦；不是回覆的話，馬上直接回一則給它。
    /// 馬上回是為了防火牆：Windows 只擋「沒問就來」的封包，剛送出廣播的那幾秒，對方的回覆放得進來
    fn on_beacon(&self, udp: &UdpSocket, data: &[u8], from: SocketAddr) {
        let Some(b) = parse_beacon(data) else {
            return;
        };
        if b.id == self.me || !from.is_ipv4() {
            return; // 自己的廣播繞回來了
        }
        let addr = SocketAddr::new(from.ip(), b.port);
        let wants_me = b.want == self.me;
        let now = Instant::now();
        let (changed, reply) = {
            let mut guard = self.lock();
            let s = &mut *guard;
            if !s.on {
                return;
            }
            let mut changed = false;
            let f = s.found.entry(b.id.clone()).or_insert_with(|| {
                changed = true;
                Found { name: b.name.clone(), addr, addr_seen: now, wants_me, seen: now, replied: None }
            });
            if f.addr == addr {
                f.addr_seen = now;
            } else if f.addr_seen.elapsed() >= ADDR_SWITCH {
                f.addr = addr;
                f.addr_seen = now;
                changed = true;
            }
            changed |= f.name != b.name || f.wants_me != wants_me;
            f.name.clone_from(&b.name);
            f.wants_me = wants_me;
            f.seen = now;
            let reply = !b.re && f.replied.is_none_or(|t| t.elapsed() >= REPLY_EVERY);
            if reply {
                f.replied = Some(now);
            }
            // 勾選的那台改了電腦名稱：清單上也跟著改
            if let Some(p) = s.peer.as_mut().filter(|p| p.0 == b.id) {
                p.1.clone_from(&b.name);
            }
            (changed, reply.then(|| s.peer.as_ref().map(|p| p.0.clone()).unwrap_or_default()))
        };
        if let Some(want) = reply {
            let _ = udp.send_to(&self.beacon(&want, true), from);
        }
        if changed {
            self.emit(Event::Changed);
        }
    }

    /// 太久沒消息的電腦從清單拿掉
    fn prune(&self) {
        let changed = {
            let mut s = self.lock();
            let before = s.found.len();
            s.found.retain(|_, f| f.seen.elapsed() < FOUND_TTL);
            s.found.len() != before
        };
        if changed {
            self.emit(Event::Changed);
        }
    }

    /// 兩邊互相勾選了、還沒連上：輪到我就撥過去
    fn maybe_dial(self: &Arc<Self>, run: u64) {
        let (id, addr) = {
            let mut s = self.lock();
            let target = match (&s.peer, &s.conn) {
                (Some((id, _)), None) => s.found.get(id).filter(|f| f.wants_me).map(|f| (id.clone(), f.addr)),
                _ => None,
            };
            let Some((id, addr)) = target else {
                s.mutual_since = None;
                return;
            };
            let since = *s.mutual_since.get_or_insert_with(Instant::now);
            let my_turn = self.me < id || since.elapsed() >= DIAL_FALLBACK;
            let due = s.last_dial.is_none_or(|t| t.elapsed() >= DIAL_RETRY);
            if s.dialing || !my_turn || !due {
                return;
            }
            s.dialing = true;
            s.last_dial = Some(Instant::now());
            (id, addr)
        };
        let link = self.clone();
        thread::spawn(move || {
            let stream = TcpStream::connect_timeout(&addr, DIAL_TIMEOUT);
            link.lock().dialing = false;
            if let Ok(stream) = stream {
                link.run_conn(stream, run, Some(id));
            }
        });
    }

    /// 一條連線從握手到掛斷。dialed 是我撥的對象；別人撥進來的是 None。
    /// 握手：兩邊各送一行 { app, v, id, want }，要「我勾了你、你也勾了我」才留下來
    fn run_conn(self: &Arc<Self>, stream: TcpStream, run: u64, dialed: Option<String>) {
        // Windows 上接進來的連線會沿用 listener 的非阻塞設定，要改回來
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_read_timeout(Some(DIAL_TIMEOUT));
        let serial = stream.try_clone().ok().and_then(|r| {
            let want = self.lock().peer.as_ref().map(|p| p.0.clone())?;
            let hello = json!({ "app": APP, "v": V, "id": self.me, "want": want }).to_string() + "\n";
            (&stream).write_all(hello.as_bytes()).ok()?;
            let mut reader = BufReader::new(r);
            let id = read_line(&mut reader).and_then(|l| parse_hello(&l, &self.me))?;
            if dialed.as_ref().is_some_and(|d| *d != id) {
                return None; // 撥給 A 卻是 B 接的（IP 換人用了）
            }
            let dialer = if dialed.is_some() { self.me.clone() } else { id.clone() };
            let serial = self.register(run, &id, dialer, &stream)?;
            Some((serial, reader))
        });
        let Some((serial, mut reader)) = serial else {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        };
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        // 斷線、逾時、一行太長、不是 UTF-8 都會讀到 None，就掛斷
        while let Some(line) = read_line(&mut reader) {
            if line.is_empty() {
                continue; // 空行是「線還通著」
            }
            if let Ok(msg @ Value::Object(_)) = serde_json::from_str::<Value>(&line) {
                self.emit(Event::Msg(msg));
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
        let dropped = {
            let mut s = self.lock();
            let mine = s.conn.as_ref().is_some_and(|c| c.serial == serial);
            if mine {
                s.conn = None;
            }
            mine
        };
        if dropped {
            self.emit(Event::Conn);
            self.emit(Event::Changed);
        }
    }

    /// 握手成功：記成現在這條連線，回傳編號；不該留的回傳 None
    fn register(&self, run: u64, id: &str, dialer: String, stream: &TcpStream) -> Option<u64> {
        let writer = stream.try_clone().ok()?;
        let (serial, fresh) = {
            let mut s = self.lock();
            if !s.on || s.run != run || s.peer.as_ref().map(|p| p.0.as_str()) != Some(id) {
                return None;
            }
            let mut fresh = true;
            if let Some(old) = &s.conn {
                // 同一台的第二條線：兩邊同時撥通，或舊的那條其實已經斷了還沒發現。
                // 留撥號方代號小的那條；一樣的話舊的大概斷了，換新的
                if old.peer == id && old.dialer < dialer {
                    return None;
                }
                fresh = old.peer != id; // 同一台換一條線而已，前端看來一直連著
                let _ = old.stream.shutdown(Shutdown::Both);
            }
            s.serial += 1;
            let serial = s.serial;
            s.conn = Some(Conn { peer: id.to_string(), dialer, serial, stream: writer });
            s.mutual_since = None;
            (serial, fresh)
        };
        if fresh {
            self.emit(Event::Conn);
            self.emit(Event::Changed);
        }
        Some(serial)
    }
}

/// 掛掉現在這條連線，回傳有沒有掛（有的話要通知前端）
fn hang_up(s: &mut Inner) -> bool {
    match s.conn.take() {
        Some(c) => {
            let _ = c.stream.shutdown(Shutdown::Both);
            true
        }
        None => false,
    }
}

struct Beacon {
    id: String,
    name: String,
    want: String,
    port: u16,
    re: bool,
}

/// 代號：小寫英文字母和數字，4～16 個字
pub fn valid_id(s: &str) -> bool {
    (4..=16).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// 電腦名稱是別人送來的：拿掉控制字元，最多 40 個字
fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string()
}

fn parse_beacon(data: &[u8]) -> Option<Beacon> {
    let v: Value = serde_json::from_slice(data).ok()?;
    if v["app"] != APP || v["v"] != V {
        return None;
    }
    Some(Beacon {
        id: v["id"].as_str().filter(|s| valid_id(s))?.to_string(),
        name: v["name"].as_str().map(clean_name).unwrap_or_default(),
        want: v["want"].as_str().unwrap_or_default().to_string(),
        port: v["port"].as_u64().and_then(|p| u16::try_from(p).ok()).filter(|&p| p > 0)?,
        re: v["re"].as_bool().unwrap_or(false),
    })
}

/// 握手那一行：格式對、對方也勾了我，才回傳對方的代號
fn parse_hello(line: &str, me: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["app"] != APP || v["v"] != V || v["want"] != me {
        return None;
    }
    v["id"].as_str().filter(|s| valid_id(s)).map(str::to_string)
}

/// 讀一行（不含換行）。斷線、逾時、超過 MAX_LINE、不是 UTF-8 都是 None
fn read_line(r: &mut impl BufRead) -> Option<String> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf).ok()?;
    if n == 0 || buf.last() != Some(&b'\n') {
        return None;
    }
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf).ok()
}

/// 這台電腦每張網卡的 IPv4 位址和廣播位址（不含 127.0.0.1）
fn local_v4() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !a.ip.is_loopback() => {
                let computed = Ipv4Addr::from(u32::from(a.ip) | !u32::from(a.netmask));
                let broadcast = (a.prefixlen < 31).then(|| a.broadcast.unwrap_or(computed));
                Some((a.ip, broadcast))
            }
            _ => None,
        })
        .collect()
}

/// 這台電腦的代號：第一次打開毛球貓時產生，存在設定檔裡。
/// 不用另外裝亂數套件：RandomState 每次開程式都會拿系統給的亂數當種子
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    let mut n = h.finish();
    (0..10)
        .map(|_| {
            let d = (n % 36) as u32;
            n /= 36;
            char::from_digit(d, 36).unwrap_or('0')
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn ids() {
        let a = new_id();
        assert!(valid_id(&a), "{a}");
        assert_ne!(a, new_id());
        assert!(!valid_id("abc"));
        assert!(!valid_id("ABCD1234"));
        assert!(!valid_id("abcd-1234"));
    }

    #[test]
    fn lines() {
        let long = "x".repeat(MAX_LINE + 1);
        let data = format!("hi\r\n\n{{\"t\":1}}\n{long}\n");
        let mut r = BufReader::new(data.as_bytes());
        assert_eq!(read_line(&mut r).as_deref(), Some("hi"));
        assert_eq!(read_line(&mut r).as_deref(), Some(""));
        assert_eq!(read_line(&mut r).as_deref(), Some("{\"t\":1}"));
        assert_eq!(read_line(&mut r), None); // 一行太長
        let mut r = BufReader::new("沒有換行就斷了".as_bytes());
        assert_eq!(read_line(&mut r), None);
    }

    #[test]
    fn beacons() {
        let ok = br#"{"app":"yarncat","v":1,"id":"abcd1234","name":"A\u0007B","want":"","port":52525}"#;
        let b = parse_beacon(ok).expect("格式對");
        assert_eq!((b.id.as_str(), b.name.as_str(), b.port, b.re), ("abcd1234", "AB", 52525, false));
        assert!(parse_beacon(br#"{"app":"yarncat","v":2,"id":"abcd1234","port":1}"#).is_none());
        assert!(parse_beacon(br#"{"app":"yarncat","v":1,"id":"../x","port":1}"#).is_none());
        assert!(parse_beacon(br#"{"app":"yarncat","v":1,"id":"abcd1234","port":70000}"#).is_none());
        assert!(parse_beacon(b"not json").is_none());
        assert_eq!(parse_hello(r#"{"app":"yarncat","v":1,"id":"abcd1234","want":"me000000"}"#, "me000000").as_deref(), Some("abcd1234"));
        assert_eq!(parse_hello(r#"{"app":"yarncat","v":1,"id":"abcd1234","want":"other000"}"#, "me000000"), None);
    }

    /// 測試用的節點：事件丟進 channel
    fn node(me: &str, ports: (u16, u16)) -> (Arc<Link>, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let link = Link::new(me.into(), format!("電腦{me}"), ports, move |e| {
            let _ = tx.lock().unwrap().send(e);
        });
        link.set_ips(vec![Ipv4Addr::LOCALHOST]);
        (link, rx)
    }

    fn wait(what: &str, mut ok: impl FnMut() -> bool) {
        let start = Instant::now();
        while !ok() {
            assert!(start.elapsed() < Duration::from_secs(20), "等太久：{what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn next_msg(rx: &mpsc::Receiver<Event>) -> Value {
        let start = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Event::Msg(v)) => return v,
                Ok(_) => {}
                Err(e) => panic!("沒收到訊息：{e}"),
            }
            assert!(start.elapsed() < Duration::from_secs(10), "沒收到訊息");
        }
    }

    /// 同一台電腦上的兩個節點：找到對方、互相勾選、連上、互傳訊息、一邊關掉另一邊會知道
    #[test]
    fn two_nodes() {
        let (a, ra) = node("aaaa0001", (52611, 52612));
        let (b, rb) = node("bbbb0002", (52612, 52611));
        a.set_on(true);
        b.set_on(true);
        wait("A 找到 B", || a.found_name("bbbb0002").is_some());
        wait("B 找到 A", || b.found_name("aaaa0001").is_some());
        assert_eq!(a.found_name("bbbb0002").as_deref(), Some("電腦bbbb0002"));

        // 只有一邊勾：不會連上
        a.set_peer(Some(("bbbb0002".into(), String::new())));
        thread::sleep(Duration::from_secs(3));
        assert_eq!(a.connected(), None);
        assert!(b.status().peers.iter().any(|p| p.id == "aaaa0001" && p.wants_me && !p.chosen));

        b.set_peer(Some(("aaaa0001".into(), String::new())));
        wait("A 連上", || a.connected().as_deref() == Some("bbbb0002"));
        wait("B 連上", || b.connected().as_deref() == Some("aaaa0001"));
        assert_eq!(a.status().peers[0].name, "電腦bbbb0002"); // 勾選時沒給名字，收到廣播後補上

        a.send(&json!({ "t": "hi", "n": "一\n二" })).unwrap();
        assert_eq!(next_msg(&rb), json!({ "t": "hi", "n": "一\n二" }));
        b.send(&json!({ "t": "visit" })).unwrap();
        assert_eq!(next_msg(&ra), json!({ "t": "visit" }));
        assert!(a.send(&json!("不是物件")).is_err());

        // B 取消勾選：兩邊都斷
        b.set_peer(None);
        wait("A 發現斷了", || a.connected().is_none());
        assert_eq!(b.connected(), None);

        // 再勾回來會重新連上；B 關掉連線，A 會發現
        b.set_peer(Some(("aaaa0001".into(), String::new())));
        wait("重新連上", || a.connected().is_some() && b.connected().is_some());
        b.set_on(false);
        wait("A 發現 B 關掉了", || a.connected().is_none());
        assert!(b.status().peers.iter().all(|p| !p.connected));
        a.set_on(false);
    }

    /// 一行太長就掛斷；沒勾選的人撥進來不理
    #[test]
    fn rejects() {
        let (a, _ra) = node("aaaa0003", (52613, 52614));
        a.set_peer(Some(("bbbb0004".into(), String::new())));
        a.set_on(true);
        wait("A 開好 port", || TcpStream::connect(("127.0.0.1", 52613)).is_ok());

        // 冒充別人：A 勾的是 bbbb0004，cccc0005 撥進來會被掛斷
        let mut s = TcpStream::connect(("127.0.0.1", 52613)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"{\"app\":\"yarncat\",\"v\":1,\"id\":\"cccc0005\",\"want\":\"aaaa0003\"}\n").unwrap();
        let mut rest = String::new();
        let _ = BufReader::new(&s).read_to_string(&mut rest);
        assert_eq!(a.connected(), None);

        // 本人撥進來會連上；送一行太長的就被掛斷
        let mut s = TcpStream::connect(("127.0.0.1", 52613)).unwrap();
        s.write_all(b"{\"app\":\"yarncat\",\"v\":1,\"id\":\"bbbb0004\",\"want\":\"aaaa0003\"}\n").unwrap();
        wait("A 接受 B", || a.connected().is_some());
        let _ = s.write_all(&vec![b'x'; MAX_LINE + 10]);
        wait("A 掛斷", || a.connected().is_none());
        a.set_on(false);
    }
}

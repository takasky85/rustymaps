//! Phone sync: receives the day's KML tracks from the Android tracker over the home WiFi.
//!
//! * **Finding the PC**: the phone broadcasts `RMAPS1 WHO` (UDP) and this app answers with its
//!   address, so nothing has to be typed.
//! * **Pairing**: you press "Pair a phone" here, which shows a short code, and type it on the phone.
//!   Both sides prove they know the code and derive the same secret key from it; the key itself is
//!   never sent. A wrong code four times closes the pairing window.
//! * **Sending**: after that, every file the phone sends is signed with that key (HMAC-SHA-256), so
//!   only the paired phone can add tracks. The data is *not* encrypted: it is meant for your own
//!   home network.
//!
//! Std only (see `sha256.rs`). Everything runs on small worker threads; the app polls `snapshot`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::sha256::{from_hex, hex, hmac, same, sha256, stretch};

pub const TCP_PORT: u16 = 47680;
pub const UDP_PORT: u16 = 47680;
const MAX_BODY: usize = 32 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 8;
const PAIR_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_WRONG_CODES: u8 = 4;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Clone)]
pub struct Phone {
    pub id: String,
    pub name: String,
    pub key: [u8; 32],
}

struct Pairing {
    code: String,
    until: Instant,
    wrong: u8,
}

struct Shared {
    desk_id: String,
    desk_name: String,
    phones: Vec<Phone>,
    pairing: Option<Pairing>,
    /// Files saved since the app last looked.
    saved: Vec<String>,
    log: Vec<String>,
    /// Newest time stamp accepted per (phone, file), so an old copy cannot overwrite a newer one.
    last_time: HashMap<(String, String), u64>,
    tcp_port: u16,
    udp_port: u16,
    error: String,
}

/// What the app shows.
pub struct Snapshot {
    pub running: bool,
    /// The pairing code in the form `ABCD-EFGH`, while pairing is open.
    pub code: Option<String>,
    pub code_secs_left: u64,
    pub phones: Vec<(String, String)>,
    pub log: Vec<String>,
    pub error: String,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub desk_name: String,
}

pub struct PhoneSync {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    cfg: PathBuf,
}

// ------------------------------------------------------------------ small helpers

fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut buf = vec![0u8; n];
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    // Windows: the standard library's hash keys come from the system's random number generator.
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut i = 0u64;
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(nanos);
        out.extend_from_slice(&h.finish().to_le_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

pub fn normalize_code(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn new_code() -> String {
    random_bytes(8).iter().map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char).collect()
}

fn show_code(code: &str) -> String {
    if code.len() == 8 {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_string()
    }
}

/// Keeps a name short and free of anything that could break a line or a file name.
fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() && *c != '|').take(40).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "Phone".to_string()
    } else {
        t
    }
}

fn slug(s: &str) -> String {
    let t: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    if t.is_empty() {
        "phone".to_string()
    } else {
        t
    }
}

/// A KML file name the phone may send: letters, digits, `.`, `_`, `-`, ending in `.kml`.
pub fn valid_file_name(n: &str) -> bool {
    n.len() >= 5 && n.len() <= 80 && n.ends_with(".kml") && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') && !n.starts_with('.') && !n.contains("..")
}

fn valid_id(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn desktop_name() -> String {
    clean_name(&std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Desktop".to_string()))
}

/// This computer's address on the local network (for typing into the phone if finding it fails).
pub fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // nothing is sent; this only picks the outgoing interface
    s.local_addr().ok().map(|a| a.ip())
}

// ------------------------------------------------------------------ saved settings

fn load_cfg(path: &Path) -> (String, Vec<Phone>) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut id = String::new();
    let mut phones = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("desk=") {
            id = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("phone=") {
            let mut it = v.splitn(3, '|');
            if let (Some(pid), Some(k), Some(name)) = (it.next(), it.next(), it.next()) {
                if let Some(b) = from_hex(k).filter(|b| b.len() == 32 && valid_id(pid)) {
                    let mut key = [0u8; 32];
                    key.copy_from_slice(&b);
                    phones.push(Phone { id: pid.to_string(), name: clean_name(name), key });
                }
            }
        }
    }
    if !valid_id(&id) {
        id = hex(&random_bytes(8));
    }
    (id, phones)
}

fn save_cfg(path: &Path, s: &Shared) {
    let mut t = format!("# RustyMaps phone sync: this computer's id and the phones paired with it (keep private)\ndesk={}\n", s.desk_id);
    for p in &s.phones {
        t.push_str(&format!("phone={}|{}|{}\n", p.id, hex(&p.key), p.name));
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, t);
}

fn note(s: &mut Shared, line: String) {
    if s.log.last() == Some(&line) {
        return;
    }
    s.log.push(line);
    if s.log.len() > 12 {
        s.log.remove(0);
    }
}

// ------------------------------------------------------------------ HTTP

struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn reply(stream: &mut TcpStream, code: u16, body: &str) {
    let text = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let msg = format!("HTTP/1.1 {code} {text}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(msg.as_bytes());
    let _ = stream.flush();
}

fn read_request(stream: &mut TcpStream) -> Result<Req, (u16, &'static str)> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > 16 * 1024 {
            return Err((400, "header too long"));
        }
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => return Err((400, "incomplete")),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err((413, "too big"));
    }
    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => return Err((400, "incomplete body")),
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(len);
    Ok(Req { method, path, headers, body })
}

fn fields(body: &[u8]) -> HashMap<String, String> {
    String::from_utf8_lossy(body)
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Ctx {
    shared: Arc<Mutex<Shared>>,
    dir: PathBuf,
    cfg: PathBuf,
    notify: Arc<dyn Fn() + Send + Sync>,
}

fn pair(ctx: &Ctx, req: &Req) -> (u16, String) {
    let f = fields(&req.body);
    let (Some(phone), Some(nonce), Some(proof)) = (f.get("phone"), f.get("nonce"), f.get("proof")) else { return (400, "missing fields".into()) };
    if !valid_id(phone) || nonce.len() < 16 || nonce.len() > 64 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return (400, "bad fields".into());
    }
    let name = clean_name(f.get("name").map(|s| s.as_str()).unwrap_or(""));
    let code = {
        let mut s = ctx.shared.lock().unwrap();
        match &s.pairing {
            Some(p) if p.until > Instant::now() => p.code.clone(),
            _ => {
                s.pairing = None;
                return (403, "pairing is not open on the computer".into());
            }
        }
    };
    let pk = stretch(&code);
    let want = hex(&hmac(&pk, format!("pair|{phone}|{nonce}").as_bytes()));
    if !same(want.as_bytes(), proof.as_bytes()) {
        let mut s = ctx.shared.lock().unwrap();
        if let Some(p) = &mut s.pairing {
            p.wrong += 1;
            if p.wrong >= MAX_WRONG_CODES {
                s.pairing = None;
                note(&mut s, "Pairing closed after too many wrong codes".into());
            }
        }
        drop(s);
        (ctx.notify)();
        return (403, "wrong code".into());
    }
    let desk_nonce = hex(&random_bytes(16));
    let desk_proof = hex(&hmac(&pk, format!("desk|{nonce}|{desk_nonce}").as_bytes()));
    let key = hmac(&pk, format!("key|{nonce}|{desk_nonce}").as_bytes());
    let (desk_id, desk_name) = {
        let mut s = ctx.shared.lock().unwrap();
        s.phones.retain(|p| p.id != *phone);
        s.phones.push(Phone { id: phone.clone(), name: name.clone(), key });
        s.pairing = None;
        note(&mut s, format!("Paired with {name}"));
        save_cfg(&ctx.cfg, &s);
        (s.desk_id.clone(), s.desk_name.clone())
    };
    (ctx.notify)();
    (200, format!("desk={desk_id}\nname={desk_name}\nnonce={desk_nonce}\nproof={desk_proof}\n"))
}

/// The paired phone a request is from, if its signature is right for `message`.
fn authenticate(ctx: &Ctx, req: &Req, message: impl Fn(u64) -> String) -> Result<Phone, (u16, String)> {
    let h = |k: &str| req.headers.get(k).cloned().unwrap_or_default();
    let (id, sig) = (h("x-phone"), h("x-sig"));
    let Ok(time) = h("x-time").parse::<u64>() else { return Err((400, "no time".into())) };
    let phone = ctx.shared.lock().unwrap().phones.iter().find(|p| p.id == id).cloned();
    let Some(phone) = phone else { return Err((401, "not paired".into())) };
    let want = hex(&hmac(&phone.key, message(time).as_bytes()));
    if same(want.as_bytes(), sig.as_bytes()) {
        Ok(phone)
    } else {
        Err((401, "bad signature".into()))
    }
}

fn put_track(ctx: &Ctx, req: &Req, name: &str) -> (u16, String) {
    if !valid_file_name(name) {
        return (400, "bad file name".into());
    }
    let digest = hex(&sha256(&req.body));
    let phone = match authenticate(ctx, req, |t| format!("put|{name}|{t}|{digest}")) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let time: u64 = req.headers.get("x-time").and_then(|v| v.parse().ok()).unwrap_or(0);
    if !String::from_utf8_lossy(&req.body[..req.body.len().min(4096)]).contains("<kml") {
        return (400, "not a KML file".into());
    }
    {
        let s = ctx.shared.lock().unwrap();
        if s.last_time.get(&(phone.id.clone(), name.to_string())).map_or(false, |&t| time < t) {
            return (409, "an older copy".into());
        }
    }
    let file = format!("{}_{}", slug(&phone.name), name);
    let dest = ctx.dir.join(&file);
    let tmp = ctx.dir.join(format!("{file}.part"));
    let _ = std::fs::create_dir_all(&ctx.dir);
    if let Err(e) = std::fs::write(&tmp, &req.body).and_then(|_| std::fs::rename(&tmp, &dest)) {
        let _ = std::fs::remove_file(&tmp);
        return (500, format!("could not save: {e}"));
    }
    {
        let mut s = ctx.shared.lock().unwrap();
        s.last_time.insert((phone.id.clone(), name.to_string()), time);
        if !s.saved.contains(&file) {
            s.saved.push(file.clone());
        }
        note(&mut s, format!("Received {file} ({} KB)", req.body.len() / 1024));
    }
    (ctx.notify)();
    (200, "ok".into())
}

fn handle(ctx: &Ctx, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err((c, m)) => return reply(&mut stream, c, m),
    };
    let (code, body) = match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/pair") => pair(ctx, &req),
        ("GET", "/ping") => match authenticate(ctx, &req, |t| format!("ping|{t}")) {
            Ok(_) => (200, format!("ok {}", ctx.shared.lock().unwrap().desk_name)),
            Err(e) => e,
        },
        // lets the phone check an address directly when the broadcast search is blocked
        ("GET", "/hello") => {
            let mut s = ctx.shared.lock().unwrap();
            if let Ok(a) = stream.peer_addr() {
                note(&mut s, format!("Phone at {} checked in", a.ip()));
            }
            let r = (200, format!("RMAPS1 HERE {} {} {}", s.tcp_port, s.desk_id, s.desk_name));
            drop(s);
            (ctx.notify)();
            r
        }
        ("PUT", p) if p.starts_with("/track/") => put_track(ctx, &req, &p["/track/".len()..]),
        _ => (404, "not found".into()),
    };
    reply(&mut stream, code, &body);
}

// ------------------------------------------------------------------ the service

impl PhoneSync {
    /// Starts listening. `tcp_port` / `udp_port` of 0 pick any free port (tests).
    pub fn start(dir: PathBuf, cfg: PathBuf, tcp_port: u16, udp_port: u16, notify: Arc<dyn Fn() + Send + Sync>) -> PhoneSync {
        let (desk_id, phones) = load_cfg(&cfg);
        let shared = Arc::new(Mutex::new(Shared {
            desk_id,
            desk_name: desktop_name(),
            phones,
            pairing: None,
            saved: Vec::new(),
            log: Vec::new(),
            last_time: HashMap::new(),
            tcp_port: 0,
            udp_port: 0,
            error: String::new(),
        }));
        save_cfg(&cfg, &shared.lock().unwrap());
        let stop = Arc::new(AtomicBool::new(false));

        // TCP: the first free port from `tcp_port` up.
        let tcp = if tcp_port == 0 {
            TcpListener::bind(("0.0.0.0", 0)).ok()
        } else {
            (0..10u16).find_map(|k| TcpListener::bind(("0.0.0.0", tcp_port + k)).ok())
        };
        match tcp {
            Some(l) => {
                let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
                shared.lock().unwrap().tcp_port = port;
                let _ = l.set_nonblocking(true);
                let ctx = Arc::new(Ctx { shared: shared.clone(), dir, cfg: cfg.clone(), notify: notify.clone() });
                let stop2 = stop.clone();
                let active = Arc::new(AtomicUsize::new(0));
                let _ = std::thread::Builder::new().name("sync-http".into()).spawn(move || {
                    while !stop2.load(Ordering::Relaxed) {
                        match l.accept() {
                            Ok((s, _)) => {
                                let _ = s.set_nonblocking(false);
                                if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                                    continue;
                                }
                                active.fetch_add(1, Ordering::Relaxed);
                                let (ctx, active) = (ctx.clone(), active.clone());
                                let _ = std::thread::Builder::new().name("sync-conn".into()).spawn(move || {
                                    handle(&ctx, s);
                                    active.fetch_sub(1, Ordering::Relaxed);
                                });
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(80)),
                        }
                    }
                });
            }
            None => shared.lock().unwrap().error = "Could not open a network port (is another copy running?)".to_string(),
        }

        // UDP: answers "who is RustyMaps?"
        match UdpSocket::bind(("0.0.0.0", udp_port)) {
            Ok(u) => {
                let port = u.local_addr().map(|a| a.port()).unwrap_or(0);
                shared.lock().unwrap().udp_port = port;
                let _ = u.set_read_timeout(Some(Duration::from_millis(300)));
                let (sh, stop2, nf) = (shared.clone(), stop.clone(), notify.clone());
                let _ = std::thread::Builder::new().name("sync-udp".into()).spawn(move || {
                    let mut buf = [0u8; 256];
                    while !stop2.load(Ordering::Relaxed) {
                        if let Ok((n, from)) = u.recv_from(&mut buf) {
                            if buf[..n].starts_with(b"RMAPS1 WHO") {
                                let mut s = sh.lock().unwrap();
                                let msg = format!("RMAPS1 HERE {} {} {}", s.tcp_port, s.desk_id, s.desk_name);
                                note(&mut s, format!("Search from {} answered", from.ip()));
                                drop(s);
                                let _ = u.send_to(msg.as_bytes(), from);
                                nf();
                            }
                        }
                    }
                });
            }
            Err(_) => {
                let mut s = shared.lock().unwrap();
                if s.error.is_empty() {
                    s.error = "Could not listen for the phone's search (the phone can still connect by address)".to_string();
                }
            }
        }
        PhoneSync { shared, stop, cfg }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Opens pairing for five minutes and returns the code (`ABCD-EFGH`).
    pub fn begin_pairing(&self) -> String {
        let code = new_code();
        let mut s = self.shared.lock().unwrap();
        s.pairing = Some(Pairing { code: code.clone(), until: Instant::now() + PAIR_WINDOW, wrong: 0 });
        show_code(&code)
    }

    pub fn cancel_pairing(&self) {
        self.shared.lock().unwrap().pairing = None;
    }

    pub fn forget(&self, id: &str) {
        let mut s = self.shared.lock().unwrap();
        s.phones.retain(|p| p.id != id);
        save_cfg(&self.cfg, &s);
    }

    /// Files received since the last call (names inside the tracks folder).
    pub fn take_saved(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().unwrap().saved)
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.shared.lock().unwrap();
        if matches!(&s.pairing, Some(p) if p.until <= Instant::now()) {
            s.pairing = None;
        }
        Snapshot {
            running: s.tcp_port != 0,
            code: s.pairing.as_ref().map(|p| show_code(&p.code)),
            code_secs_left: s.pairing.as_ref().map_or(0, |p| p.until.saturating_duration_since(Instant::now()).as_secs()),
            phones: s.phones.iter().map(|p| (p.id.clone(), p.name.clone())).collect(),
            log: s.log.clone(),
            error: s.error.clone(),
            tcp_port: s.tcp_port,
            udp_port: s.udp_port,
            desk_name: s.desk_name.clone(),
        }
    }
}

impl Drop for PhoneSync {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------------ the phone's side, for tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn request(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        let mut h = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            h.push_str(&format!("{k}: {v}\r\n"));
        }
        h.push_str("\r\n");
        s.write_all(h.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).to_string();
        let code: u16 = text.split(' ').nth(1).unwrap().parse().unwrap();
        (code, text.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
    }

    fn pair_as_phone(addr: SocketAddr, id: &str, code: &str) -> Result<[u8; 32], (u16, String)> {
        let pk = stretch(&normalize_code(code));
        let nonce = hex(&[9u8; 16]);
        let proof = hex(&hmac(&pk, format!("pair|{id}|{nonce}").as_bytes()));
        let body = format!("phone={id}\nname=Test Phone\nnonce={nonce}\nproof={proof}\n");
        let (c, b) = request(addr, "POST", "/pair", &[], body.as_bytes());
        if c != 200 {
            return Err((c, b));
        }
        let f = fields(b.as_bytes());
        let dn = f["nonce"].clone();
        // the computer proves it knew the code too
        assert_eq!(f["proof"], hex(&hmac(&pk, format!("desk|{nonce}|{dn}").as_bytes())));
        Ok(hmac(&pk, format!("key|{nonce}|{dn}").as_bytes()))
    }

    fn put(addr: SocketAddr, id: &str, key: &[u8; 32], name: &str, time: u64, body: &[u8]) -> (u16, String) {
        let sig = hex(&hmac(key, format!("put|{name}|{time}|{}", hex(&sha256(body))).as_bytes()));
        request(addr, "PUT", &format!("/track/{name}"), &[("X-Phone", id.to_string()), ("X-Time", time.to_string()), ("X-Sig", sig)], body)
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rmaps-sync-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const KML: &str = "<kml><Placemark><name>t</name><LineString><coordinates>1,2 1.1,2.1</coordinates></LineString></Placemark></kml>";

    #[test]
    fn pair_send_replace_and_reject() {
        let d = temp("a");
        let s = PhoneSync::start(d.join("tracks"), d.join("sync.txt"), 0, 0, Arc::new(|| {}));
        let snap = s.snapshot();
        assert!(snap.running && snap.udp_port != 0);
        let addr: SocketAddr = ([127, 0, 0, 1], snap.tcp_port).into();
        let id = "0123456789abcdef";

        // nothing works before pairing is opened
        assert_eq!(pair_as_phone(addr, id, "AAAA-AAAA").unwrap_err().0, 403);
        let code = s.begin_pairing();
        assert_eq!(code.len(), 9);
        // a wrong code is refused (typed in lower case with the dash left out it still counts)
        assert_eq!(pair_as_phone(addr, id, "AAAAAAAA").unwrap_err().0, 403);
        let key = pair_as_phone(addr, id, &code.to_lowercase().replace('-', "")).unwrap();
        // the window is closed after one success
        assert!(s.snapshot().code.is_none());
        assert_eq!(s.snapshot().phones.len(), 1);

        assert_eq!(put(addr, id, &key, "2026-11-28.kml", 100, KML.as_bytes()).0, 200);
        let file = d.join("tracks").join("TestPhone_2026-11-28.kml");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), KML);
        assert_eq!(s.take_saved(), vec!["TestPhone_2026-11-28.kml".to_string()]);
        // the same file again replaces it (no copies); an older one is refused
        let newer = KML.replace("t</name>", "later</name>");
        assert_eq!(put(addr, id, &key, "2026-11-28.kml", 200, newer.as_bytes()).0, 200);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);
        assert_eq!(put(addr, id, &key, "2026-11-28.kml", 150, KML.as_bytes()).0, 409);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);
        assert_eq!(std::fs::read_dir(d.join("tracks")).unwrap().count(), 1);

        // wrong key, unknown phone, bad names and non-KML are all refused
        assert_eq!(put(addr, id, &[1u8; 32], "x.kml", 300, KML.as_bytes()).0, 401);
        assert_eq!(put(addr, "ffffffffffffffff", &key, "x.kml", 300, KML.as_bytes()).0, 401);
        assert_eq!(put(addr, id, &key, "..kml", 300, KML.as_bytes()).0, 400);
        assert_eq!(put(addr, id, &key, "a%2fb.kml", 300, KML.as_bytes()).0, 400);
        assert_eq!(put(addr, id, &key, "notkml.kml", 300, b"hello").0, 400);
        // a signature for other content does not carry over to a tampered body
        let sig = hex(&hmac(&key, format!("put|y.kml|300|{}", hex(&sha256(KML.as_bytes()))).as_bytes()));
        let hdrs = [("X-Phone", id.to_string()), ("X-Time", "300".to_string()), ("X-Sig", sig)];
        assert_eq!(request(addr, "PUT", "/track/y.kml", &hdrs, newer.as_bytes()).0, 401);

        // the phone can check an address directly, without the broadcast search
        let (c, hello) = request(addr, "GET", "/hello", &[], b"");
        assert!(c == 200 && hello.starts_with(&format!("RMAPS1 HERE {} ", snap.tcp_port)), "{hello}");

        // ping needs the key too
        let t = 5u64;
        let ok = hex(&hmac(&key, format!("ping|{t}").as_bytes()));
        let h = |sig: String| [("X-Phone", id.to_string()), ("X-Time", t.to_string()), ("X-Sig", sig)];
        assert_eq!(request(addr, "GET", "/ping", &h(ok), b"").0, 200);
        assert_eq!(request(addr, "GET", "/ping", &h("00".into()), b"").0, 401);

        // forgetting a phone cuts it off, and a restart remembers who is paired
        drop(s);
        let s2 = PhoneSync::start(d.join("tracks"), d.join("sync.txt"), 0, 0, Arc::new(|| {}));
        assert_eq!(s2.snapshot().phones.len(), 1);
        let addr2: SocketAddr = ([127, 0, 0, 1], s2.snapshot().tcp_port).into();
        assert_eq!(put(addr2, id, &key, "z.kml", 400, KML.as_bytes()).0, 200);
        s2.forget(id);
        assert_eq!(put(addr2, id, &key, "z.kml", 500, KML.as_bytes()).0, 401);
        s2.stop();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn too_many_wrong_codes_close_pairing() {
        let d = temp("b");
        let s = PhoneSync::start(d.join("tracks"), d.join("sync.txt"), 0, 0, Arc::new(|| {}));
        let addr: SocketAddr = ([127, 0, 0, 1], s.snapshot().tcp_port).into();
        let code = s.begin_pairing();
        for _ in 0..MAX_WRONG_CODES {
            assert_eq!(pair_as_phone(addr, "0123456789abcdef", "ZZZZZZZZ").unwrap_err().0, 403);
        }
        assert!(s.snapshot().code.is_none(), "the window closes");
        // even the right code no longer works
        assert_eq!(pair_as_phone(addr, "0123456789abcdef", &code).unwrap_err().0, 403);
        s.stop();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn answers_the_phones_search() {
        let d = temp("c");
        let s = PhoneSync::start(d.join("tracks"), d.join("sync.txt"), 0, 0, Arc::new(|| {}));
        let snap = s.snapshot();
        let u = UdpSocket::bind("127.0.0.1:0").unwrap();
        u.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        u.send_to(b"RMAPS1 WHO", ("127.0.0.1", snap.udp_port)).unwrap();
        let mut buf = [0u8; 256];
        let (n, _) = u.recv_from(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(text.starts_with(&format!("RMAPS1 HERE {} ", snap.tcp_port)), "{text}");
        assert!(text.ends_with(&snap.desk_name));
        s.stop();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn codes_and_names_are_tidy() {
        assert_eq!(normalize_code("abcd-ef gh"), "ABCDEFGH");
        assert_eq!(new_code().len(), 8);
        assert!(new_code().chars().all(|c| CODE_ALPHABET.contains(&(c as u8))));
        assert_ne!(new_code(), new_code());
        assert!(valid_file_name("2026-11-28.kml") && valid_file_name("a_b.kml"));
        assert!(!valid_file_name("../a.kml") && !valid_file_name("a/b.kml") && !valid_file_name("a.txt") && !valid_file_name(".kml") && !valid_file_name("a..kml"));
        assert_eq!(clean_name("Pixel|8\n"), "Pixel8");
        assert_eq!(slug("John's Pixel 8!"), "JohnsPixel8");
    }
}

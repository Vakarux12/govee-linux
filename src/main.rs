// SPDX-License-Identifier: GPL-3.0-only
//! Govee CLI: LAN control over UDP (public Govee LAN API) plus cloud login via the
//! phone-app account endpoint (no envelope, long-lived token), see docs/PROTOCOL.md §8/§11.
use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const MCAST: &str = "239.255.255.250";

#[derive(Parser)]
#[command(version, about = "Govee lights from the terminal (LAN API)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Discover devices with LAN Control enabled (ip, sku, id)
    Scan,
    /// Power on
    On { ip: String },
    /// Power off
    Off { ip: String },
    /// Brightness 1-100
    Bri { ip: String, value: u8 },
    /// Colour r g b (0-255)
    Color { ip: String, r: u8, g: u8, b: u8 },
    /// Colour temperature in kelvin (2000-9000)
    Temp { ip: String, kelvin: u16 },
    /// Device status (onOff, brightness, color, colorTemInKelvin)
    Status { ip: String },
    /// Paint segments directly (razer mode): hex colours, one per segment, e.g. ff0000 00ff00.
    /// No colours = leave razer mode.
    Segs { ip: String, colors: Vec<String> },
    /// Animated effect over the segments (razer mode) until Ctrl-C.
    /// Names: rainbow, wave, breathe, chase, fire. `--colors` = palette as rrggbb.
    Effect {
        ip: String,
        segments: u8,
        name: String,
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        #[arg(long, num_args = 1..)]
        colors: Vec<String>,
    },
    /// Screen -> lights. Targets are `ip[:area[:segments]]`. Areas: all, left, right, top, bottom
    /// (border of --edge thickness), topleft/topright/bottomleft/bottomright (corner squares),
    /// center, or a percent rect `WxH+X+Y` (e.g. `50x20+25+0` = top middle fifth).
    /// With a segment count the area is split along its long axis and streamed per segment
    /// (LAN `razer` mode); without it one mean colour goes out via `colorwc`.
    /// `area` may also be a comma list with one side per segment (`ip:left,left,top,right`);
    /// each side is then split into as many bands as segments it was given.
    Dreamview {
        #[arg(required = true)]
        targets: Vec<String>,
        /// Saturation boost (1 = raw screen average)
        #[arg(long, default_value_t = 1.6)]
        sat: f32,
        /// Max updates per second sent to each device
        #[arg(long, default_value_t = 15)]
        fps: u32,
        /// Reverse segment order (strip mounted the other way round)
        #[arg(long)]
        reverse: bool,
        /// Thickness of the side/corner areas as a fraction of the screen (0.5 = half)
        #[arg(long, default_value_t = 0.5)]
        edge: f32,
        /// Brightness 1-100; also live-adjustable by writing a number per line to stdin
        #[arg(long, default_value_t = 100)]
        bri: u32,
    },
    /// Cloud login. With an email: password prompt (phone-app endpoint, token lasts ~2 months).
    /// Without: prints a QR code to scan with the Govee phone app. Token -> ~/.config/govee/cloud.json.
    Login { email: Option<String> },
    /// Cloud device list: `device<TAB>sku<TAB>name<TAB>topic<TAB>imgUrl` (needs `login`)
    Devices,
    /// List cloud scenes for a device: `id<TAB>name<TAB>category<TAB>iconUrl` (device id + sku as printed by scan/devices)
    Scenes { device: String, sku: String },
    /// Apply a scene. `ip` = LAN address (scene bytes sent over the LAN API); `-` = via Govee cloud instead.
    Scene { ip: String, device: String, sku: String, id: String },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan => scan(),
        Cmd::On { ip } => send(&ip, "turn", json!({"value": 1})),
        Cmd::Off { ip } => send(&ip, "turn", json!({"value": 0})),
        Cmd::Bri { ip, value } => send(&ip, "brightness", json!({"value": value.clamp(1, 100)})),
        Cmd::Color { ip, r, g, b } => send(&ip, "colorwc", json!({"color": {"r": r, "g": g, "b": b}, "colorTemInKelvin": 0})),
        Cmd::Temp { ip, kelvin } => send(&ip, "colorwc", json!({"color": {"r": 0, "g": 0, "b": 0}, "colorTemInKelvin": kelvin})),
        Cmd::Status { ip } => {
            let sock = reply_socket()?;
            send_on(&sock, &ip, "devStatus", json!({}))?;
            let msg = recv(&sock, Duration::from_secs(2))?.ok_or(anyhow!("no reply from {ip}"))?;
            println!("{}", serde_json::to_string_pretty(&msg["msg"]["data"])?);
            Ok(())
        }
        Cmd::Segs { ip, colors } => {
            let sock = UdpSocket::bind("0.0.0.0:0")?;
            if colors.is_empty() {
                return send_razer(&sock, &ip, &razer_packet(&[0xB1, 0x00]));
            }
            let colors = colors.iter().map(|c| parse_hex(c)).collect::<Result<Vec<_>>>()?;
            send_razer(&sock, &ip, &razer_packet(&[0xB1, 0x01]))?;
            send_razer(&sock, &ip, &razer_colors(&colors))
        }
        Cmd::Effect { ip, segments, name, speed, colors } => {
            let colors = colors.iter().map(|c| parse_hex(c)).collect::<Result<Vec<_>>>()?;
            effect(&ip, segments.max(1) as usize, &name, speed, &colors)
        }
        Cmd::Dreamview { targets, sat, fps, reverse, edge, bri } => dreamview(&targets, sat, fps, reverse, edge.clamp(0.01, 1.0), bri),
        Cmd::Login { email } => match email { Some(e) => login(&e), None => login_qr() },
        Cmd::Scenes { device, sku } => {
            for sc in scenes(&device, &sku)? {
                println!("{}\t{}\t{}\t{}", sc.id, sc.name, sc.category, sc.icon);
            }
            Ok(())
        }
        Cmd::Scene { ip, device, sku, id } => {
            let sc = scenes(&device, &sku)?.into_iter().find(|s| s.id == id || s.name.eq_ignore_ascii_case(&id)).ok_or(anyhow!("no scene {id:?}"))?;
            if ip == "-" {
                cloud("POST", "/bff-app/v1/pc/iot-control", Some(&json!({"device": device, "sku": sku, "sceneParamId": sc.param_id, "id": sc.scene_id, "type": 1})))?;
                return Ok(());
            }
            let sock = reply_socket()?;
            send_on(&sock, &ip, "ptReal", json!({"command": sc.lan_commands()}))?;
            match recv(&sock, Duration::from_secs(2))? {
                Some(r) => { if std::env::var_os("GOVEE_DEBUG").is_some() { eprintln!("{r}"); } Ok(()) }
                None => Ok(()),
            }
        }
        Cmd::Devices => {
            let v = cloud("GET", "/bff-app/v1/pc/user/devices", None)?;
            for d in v["devices"].as_array().into_iter().flatten() {
                let f = |k: &str| d[k].as_str().unwrap_or("").to_string();
                println!("{}\t{}\t{}\t{}\t{}", f("device"), f("sku"), f("name"), f("topic"), f("imgUrl"));
            }
            Ok(())
        }
    }
}

// cloud (govee please do not change anything :beg:)

const CLOUD: &str = "https://app2.govee.com";

fn cloud_file() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config/govee/cloud.json")
}

fn load_cloud() -> Result<Value> {
    let f = cloud_file();
    serde_json::from_slice(&std::fs::read(&f).with_context(|| format!("{}: run `govee login` first", f.display()))?).context("bad cloud.json")
}

fn save_cloud(store: &Value) -> Result<()> {
    let f = cloud_file();
    std::fs::create_dir_all(f.parent().unwrap())?;
    std::fs::write(&f, serde_json::to_vec_pretty(store)?)?;
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600))?)
}

///`client` + `client_type` are what the token was issued to, phone or pc
struct Ident { client: String, client_type: u8, token: String }

impl Ident {
    fn from_store(s: &Value) -> Ident {
        Ident { client: s["client"].as_str().unwrap_or("").into(), client_type: s["clientType"].as_u64().unwrap_or(1) as u8, token: s["token"].as_str().unwrap_or("").into() }
    }
    fn phone(client: String) -> Ident { Ident { client, client_type: 1, token: String::new() } }
    fn desktop() -> Ident { Ident { client: mac_address(), client_type: 7, token: String::new() } }
}

fn mac_address() -> String {
    std::fs::read_dir("/sys/class/net").ok().into_iter().flatten().flatten()
        .filter(|e| e.file_name() != "lo")
        .filter_map(|e| std::fs::read_to_string(e.path().join("address")).ok())
        .map(|m| m.trim().to_string()).find(|m| !m.is_empty() && m != "00:00:00:00:00:00")
        .unwrap_or_else(|| "02:00:00:00:00:01".into())
}


fn http(method: &str, path: &str, body: Option<&Value>, id: &Ident) -> Result<Value> {
    /// curl it is
    let mut c = std::process::Command::new("curl");
    c.args(["-sS", "--compressed", "-m", "90", "-X", method, &format!("{CLOUD}{path}")]);
    c.args(["-H", "Content-Type: application/json", "-H", &format!("Authorization: Bearer {}", id.token)]);
    let hdrs: Vec<String> = if id.client_type == 7 {
        // 500 ("service is busy") IS NOT THE SERVERS INTERNAL ERROR!
        // means something model/clientName/sysVersion/timezone/country is missing
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
        let tz = std::env::var("TZ").ok().or_else(|| std::fs::read_link("/etc/localtime").ok()
            .and_then(|p| p.to_str().and_then(|p| p.split("zoneinfo/").nth(1)).map(String::from))).unwrap_or_else(|| "UTC".into());
        let country = std::env::var("LANG").ok().and_then(|l| l.split(['_', '.']).nth(1).map(|c| c.to_uppercase())).filter(|c| c.len() == 2).unwrap_or_else(|| "US".into());
        vec![format!("clientid: {}", id.client), "clientType: 7".into(), "appVersion: 2.40.60".into(), "pcversion: 2.40.60".into(),
             format!("clientName: {host}"), "model: PC".into(), "sysVersion: Microslop Windows 11".into(), format!("timezone: {tz}"),
                                                                                // :3
             format!("country: {country}"), "Accept-Language: en".into(), "User-Agent: RestSharp/112.0.0.0".into()]
    } else {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
        vec![format!("clientId: {}", id.client), "clientType: 1".into(), "appVersion: 5.6.01".into(), "iotVersion: 0".into(), format!("timestamp: {ts}"),
             "User-Agent: GoveeHome/5.6.01 (com.ihoment.GoVeeSensor; build:2; iOS 16.5.0) Alamofire/5.6.4".into()]
    };
    for h in &hdrs {
        c.args(["-H", h]);
    }
    if let Some(b) = body {
        c.args(["-d", &b.to_string()]);
    }
    let out = c.output().context("run curl")?;
    if !out.status.success() {
        return Err(anyhow!("curl: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: Value = serde_json::from_slice(&out.stdout).with_context(|| format!("non-JSON reply: {}", String::from_utf8_lossy(&out.stdout)))?;
    if v["status"] != 200 {
        return Err(anyhow!("{path}: {} {}", v["status"], v["message"].as_str().unwrap_or("")));
    }
    Ok(v)
}

fn cloud(method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
    let mut s = load_cloud()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    // 15 min expiry
    if s["clientType"] == 7 && jwt_exp(s["token"].as_str().unwrap_or("")).unwrap_or(0) < now + 60 {
        refresh(&mut s)?;
    }
    Ok(http(method, path, body, &Ident::from_store(&s))?["data"].take())
}

/// rsa pub key from desktop app
const REFRESH_PUBKEY: &str = "MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAIBV0asQLU23iIxJQ/bwkuyhkZLPcByOsZOjQHUE2FX9A7BPr22BqYlRIHcJXWPXSf38Mb1VPU/9bBF95WZoyf0CAwEAAQ==";

fn openssl(args: &[&str], stdin: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut c = std::process::Command::new("openssl").args(args)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().context("run openssl")?;
    c.stdin.take().unwrap().write_all(stdin)?;
    let out = c.wait_with_output()?;
    if !out.status.success() {
        return Err(anyhow!("openssl {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// aes 256 ecb pkcs7 with the key's ascii bytes, 32-char hex strings as keys
fn aes_ecb(encrypt: bool, key: &str, data: &[u8]) -> Result<Vec<u8>> {
    let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    openssl(&["enc", "-aes-256-ecb", if encrypt { "-e" } else { "-d" }, "-K", &hex, "-nosalt"], data)
}

/// opencode fixed this
fn refresh(store: &mut Value) -> Result<()> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))?;
    let sk: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let pem = std::env::temp_dir().join("govee-refresh-pub.pem");
    std::fs::write(&pem, format!("-----BEGIN PUBLIC KEY-----\n{REFRESH_PUBKEY}\n-----END PUBLIC KEY-----\n"))?;
    let v = b64(&openssl(&["pkeyutl", "-encrypt", "-pubin", "-inkey", pem.to_str().unwrap()], sk.as_bytes())?);
    let d = b64(&aes_ecb(true, &sk, json!({"refreshToken": store["refreshToken"], "type": 0}).to_string().as_bytes())?);
    let r = http("POST", "/bff-app/v2/pc/refresh-token", Some(&json!({"d": d, "v": v})), &Ident::from_store(store))
        .context("token refresh failed, run `govee login` again")?;
    let t: Value = serde_json::from_slice(&aes_ecb(false, &sk, &b64decode(r["data"]["v"].as_str().unwrap_or("")))?).context("bad refresh reply")?;
    if t["token"].is_null() || t["refreshToken"].is_null() {
        return Err(anyhow!("refresh reply without tokens: {t}"));
    }
    store["token"] = t["token"].clone();
    store["refreshToken"] = t["refreshToken"].clone();
    store["tokenExpireTime"] = json!(jwt_exp(t["token"].as_str().unwrap()).unwrap_or(0) * 1000);
    save_cloud(store)
}

fn jwt_exp(token: &str) -> Option<u64> {
    serde_json::from_slice::<Value>(&b64decode(token.split('.').nth(1)?)).ok()?["exp"].as_u64()
}

struct Scene { id: String, name: String, category: String, icon: String, scene_id: Value, param_id: Value, code: u16, param: String }

fn ble_packet(payload: &[u8]) -> Vec<u8> {
    let mut p = payload.to_vec();
    p.resize(19, 0);
    let x = p.iter().fold(0, |a, b| a ^ b);
    p.push(x);
    p
}

impl Scene {
    /// opencode also did this
    /// LAN `ptReal` packets `a3 00 01 <lines> 02 ...` split into 19-byte lines (`a3 <i>` continuation, `a3 ff` last), then `33 05 04 <code>` selects the scene. Built-in scenes have no bytes and need only the code.
    fn lan_commands(&self) -> Vec<String> {
        let bytes = b64decode(&self.param);
        let mut out = Vec::new();
        if !bytes.is_empty() {
            let mut d = vec![0xa3, 0x00, 0x01, 0x00, 0x02];
            let (mut lines, mut last) = (0u8, 1);
            for b in bytes {
                if d.len() % 19 == 0 {
                    lines += 1;
                    d.push(0xa3);
                    last = d.len();
                    d.push(lines);
                }
                d.push(b);
            }
            d[last] = 0xff;
            d[3] = lines + 1;
            out.extend(d.chunks(19).map(|c| b64(&ble_packet(c))));
        }
        out.push(b64(&ble_packet(&[0x33, 0x05, 0x04, self.code as u8, (self.code >> 8) as u8])));
        out
    }
}

fn b64decode(s: &str) -> Vec<u8> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
        let Some(v) = T.iter().position(|t| *t == c).or(match c { b'-' => Some(62), b'_' => Some(63), _ => None }) else { continue };
        acc = (acc << 6) | v as u32; bits += 6;
        if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); acc &= (1 << bits) - 1; }
    }
    out
}

fn scenes(device: &str, sku: &str) -> Result<Vec<Scene>> {
    // a day
    let cache = cloud_file().with_file_name(format!("scenes-{sku}.json"));
    let fresh = std::fs::metadata(&cache).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(86400));
    let v: Value = match fresh.then(|| std::fs::read(&cache).ok()).flatten().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => v,
        None => {
            let v = cloud("GET", &format!("/appsku/v1/light-effect-libraries?sku={sku}&device={device}"), None)?;
            let _ = std::fs::write(&cache, v.to_string());
            v
        }
    };
    if std::env::var_os("GOVEE_DEBUG").is_some() { eprintln!("{}", serde_json::to_string_pretty(&v)?); }
    let mut out = Vec::new();
    for cat in v["categories"].as_array().into_iter().flatten() {
        for sc in cat["scenes"].as_array().into_iter().flatten() {
            let effects = sc["lightEffects"].as_array().cloned().unwrap_or_default();
            for e in &effects {
                let sname = sc["sceneName"].as_str().unwrap_or("");
                let ename = e["scenceName"].as_str().unwrap_or("");
                let name = if ename.is_empty() || effects.len() == 1 { sname.to_string() } else { format!("{sname} {ename}") };
                out.push(Scene {
                    id: e["scenceParamId"].to_string(), name, category: cat["categoryName"].as_str().unwrap_or("").into(),
                    icon: sc["iconUrls"][0].as_str().unwrap_or("").into(), scene_id: sc["sceneId"].clone(),
                    param_id: e["scenceParamId"].clone(), code: e["sceneCode"].as_u64().unwrap_or(0) as u16,
                    param: e["scenceParam"].as_str().unwrap_or("").into(),
                });
            }
        }
    }
    Ok(out)
}

fn read_password() -> Result<String> {
    if let Ok(p) = std::env::var("GOVEE_PASSWORD") {
        return Ok(p);
    }
    eprint!("password: ");
    let stty = |a| { let _ = std::process::Command::new("stty").arg(a).stderr(std::process::Stdio::null()).status(); };
    stty("-echo");
    let mut p = String::new();
    std::io::stdin().read_line(&mut p)?;
    stty("echo");
    eprintln!();
    Ok(p.trim_end_matches(['\r', '\n']).to_string())
}

fn login(email: &str) -> Result<()> {
    let password = read_password()?;
    // client id is saved in serverside so it needs to be saved
    let client = load_cloud().ok().filter(|s| s["clientType"] == 1).and_then(|s| s["client"].as_str().map(String::from)).unwrap_or_else(|| {
        let mut b = [0u8; 16];
        std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).ok();
        b.iter().map(|x| format!("{x:02x}")).collect()
    });
    let v = http("POST", "/account/rest/account/v1/login", Some(&json!({"email": email, "password": password, "client": client})), &Ident::phone(client.clone()))?;
    let c = &v["client"];
    save_cloud(&json!({"email": email, "client": client, "clientType": 1, "token": c["token"], "refreshToken": c["refreshToken"],
        "accountId": c["accountId"], "topic": c["topic"], "tokenExpireCycle": c["tokenExpireCycle"], "headPortrait": c["headPortrait"]}))?;
    println!("logged in as {} (account {}), token saved to {}", email, c["accountId"], cloud_file().display());
    Ok(())
}

fn login_qr() -> Result<()> {
    let id = Ident::desktop();
    let q = http("GET", "/bff-app/pad/v1/qrcode/generate", None, &id)?["data"].take();
    let (qr_id, addr) = (q["qrCodeId"].as_str().unwrap_or(""), q["qrCodeAddr"].as_str().unwrap_or(""));
    let poll = q["pollTime"].as_u64().unwrap_or(1).max(1);
    match std::process::Command::new("qrencode").args(["-t", "UTF8", addr]).output() {
        Ok(o) if o.status.success() => print!("{}", String::from_utf8_lossy(&o.stdout)),
        _ => eprintln!("(please install qrencode to use this feature!)"),
    }
    println!("{addr}\nScan with the Govee app (Ctrl-C to abort)...");
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(poll));
        let st = http("GET", &format!("/bff-app/pad/v1/qrcode/status?qrCodeId={qr_id}"), None, &id)?["data"].take();
        let status = st["qrCodeStatus"].as_str().unwrap_or("").to_string();
        if status == "UNUSED" {
            continue;
        }
        let lv = &st["loginVo"];
        if lv.is_null() {
            if std::env::var_os("GOVEE_DEBUG").is_some() { eprintln!("{status}: {st}"); }
            if status == "EXPIRED" || status == "INVALID" { return Err(anyhow!("QR code {status}, run login again")); }
            eprintln!("status: {status}");
            continue;
        }
        if std::env::var_os("GOVEE_DEBUG").is_some() { eprintln!("{}", serde_json::to_string_pretty(lv)?); }
        let token = lv["token"].as_str().or(lv["accessToken"].as_str()).ok_or_else(|| anyhow!("no token: {lv}"))?;
        let mut store = json!({"client": id.client, "clientType": 7, "token": token, "loginVo": lv});
        for k in ["refreshToken", "accountId", "email", "topic", "headPortrait", "tokenExpireTime"] {
            if !lv[k].is_null() { store[k] = lv[k].clone(); }
        }
        save_cloud(&store)?;
        println!("logged in (account {}), token saved in {}", lv["accountId"], cloud_file().display());
        return Ok(());
    }
    Err(anyhow!("timed out waiting for the scan"))
}


fn parse_hex(c: &str) -> Result<[u8; 3]> {
    let c = c.trim_start_matches('#');
    let v = u32::from_str_radix(c, 16).ok().filter(|_| c.len() == 6).ok_or(anyhow!("bad colour {c:?}, want rrggbb"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn packet(cmd: &str, data: Value) -> String {
    json!({"msg": {"cmd": cmd, "data": data}}).to_string()
}

fn reply_socket() -> Result<UdpSocket> {
    UdpSocket::bind("0.0.0.0:4002").context("bind udp 4002 (another govee client running?)")
}

fn send_on(sock: &UdpSocket, ip: &str, cmd: &str, data: Value) -> Result<()> {
    let port = if ip == MCAST { 4001 } else { 4003 };
    sock.send_to(packet(cmd, data).as_bytes(), (ip, port))?;
    Ok(())
}

fn send(ip: &str, cmd: &str, data: Value) -> Result<()> {
    send_on(&UdpSocket::bind("0.0.0.0:0")?, ip, cmd, data)
}

fn recv(sock: &UdpSocket, timeout: Duration) -> Result<Option<Value>> {
    sock.set_read_timeout(Some(timeout))?;
    let mut buf = [0u8; 4096];
    match sock.recv_from(&mut buf) {
        Ok((n, _)) => Ok(Some(serde_json::from_slice(&buf[..n])?)),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn scan() -> Result<()> {
    let sock = reply_socket()?;
    send_on(&sock, MCAST, "scan", json!({"account_topic": "reserve"}))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
        if let Some(msg) = recv(&sock, left)? {
            let d = &msg["msg"]["data"];
            let s = |k: &str| d[k].as_str().unwrap_or("?").to_string();
            println!("{}\t{}\t{}", s("ip"), s("sku"), s("device"));
        }
    }
    Ok(())
}

// DreamView tm

const FRAME: &str = "/dev/shm/govee-screen";

// GVSC u32 w u32 h u32 sw u32 sh u64 seq BGRx
fn read_frame() -> Option<(u32, u32, u64, Vec<u8>)> {
    let b = std::fs::read(FRAME).ok()?;
    if b.len() < 28 || &b[..4] != b"GVSC" {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
    let (w, h) = (u32_at(4), u32_at(8));
    let seq = u64::from_le_bytes(b[20..28].try_into().unwrap());
    (b.len() >= 28 + (w * h * 4) as usize).then(|| (w, h, seq, b[28..].to_vec()))
}

fn area_rect(area: &str, w: u32, h: u32, edge: f32) -> (u32, u32, u32, u32) {
    let (ew, eh) = ((w as f32 * edge) as u32, (h as f32 * edge) as u32);
    match area {
        "left" => (0, ew, 0, h),
        "right" => (w - ew, w, 0, h),
        "top" => (0, w, 0, eh),
        "bottom" => (0, w, h - eh, h),
        "topleft" => (0, ew, 0, eh),
        "topright" => (w - ew, w, 0, eh),
        "bottomleft" => (0, ew, h - eh, h),
        "bottomright" => (w - ew, w, h - eh, h),
        "center" => ((w - ew) / 2, (w + ew) / 2, (h - eh) / 2, (h + eh) / 2),
        g => geometry(g, w, h).unwrap_or((0, w, 0, h)),
    }
}

/// WxH+X+Y
/// 50x20+25+0
fn geometry(g: &str, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let v: Vec<u32> = g.split(['x', '+']).map(|n| n.parse().ok()).collect::<Option<_>>()?;
    let [gw, gh, gx, gy] = v[..] else { return None };
    let pct = |p: u32, full: u32| full * p.min(100) / 100;
    let (x0, y0) = (pct(gx, w).min(w - 1), pct(gy, h).min(h - 1));
    Some((x0, (x0 + pct(gw, w).max(1)).min(w), y0, (y0 + pct(gh, h).max(1)).min(h)))
}

/// color of a rect: chroma mean then saturation boost with the screen brightness kept.
fn mean_color(px: &[u8], w: u32, (x0, x1, y0, y1): (u32, u32, u32, u32), sat: f32) -> [u8; 3] {
    let (mut acc, mut n) = ([0f32; 3], 0f32);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            let (b, g, r) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32);
            let chroma = r.max(g).max(b) - r.min(g).min(b);
            let wt = chroma + 4.0; // ponytail: +4 floor keeps greys grey instead of black; tune if dull scenes look tinted
            acc[0] += r * wt;
            acc[1] += g * wt;
            acc[2] += b * wt;
            n += wt;
        }
    }
    let c = acc.map(|v| v / n.max(1.0));
    let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    if max - min < 1.0 {
        return c.map(|v| v.round() as u8);
    }
    let s = (max - min) / max;
    let k = (s * sat).min(1.0) / s; // scale chroma about V
    c.map(|v| (max - (max - v) * k).round().clamp(0.0, 255.0) as u8)
}

fn band_colors(px: &[u8], w: u32, h: u32, area: &str, n: u32, sat: f32, edge: f32) -> Vec<[u8; 3]> {
    let (x0, x1, y0, y1) = area_rect(area, w, h, edge);
    let horizontal = x1 - x0 >= y1 - y0;
    (0..n.max(1))
        .map(|i| {
            let (a, b) = if horizontal { (x0, x1) } else { (y0, y1) };
            let (s, e) = (a + (b - a) * i / n.max(1), a + (b - a) * (i + 1) / n.max(1));
            mean_color(px, w, if horizontal { (s, e, y0, y1) } else { (x0, x1, s, e) }, sat)
        })
        .collect()
}

fn map_colors(px: &[u8], w: u32, h: u32, sides: &[String], sat: f32, edge: f32) -> Vec<[u8; 3]> {
    let mut out = vec![[0u8; 3]; sides.len()];
    let mut done: Vec<&str> = vec![];
    for side in sides {
        if done.contains(&side.as_str()) {
            continue;
        }
        done.push(side);
        let idx: Vec<usize> = (0..sides.len()).filter(|&i| &sides[i] == side).collect();
        for (i, c) in idx.iter().zip(band_colors(px, w, h, side, idx.len() as u32, sat, edge)) {
            out[*i] = c;
        }
    }
    out
}

/// BB 00 <len> <cmd> <args...> <xor>, len = args bytes
/// enable = B1 01, off = B1 00, colours = B0 01 <n> r g b ...
/// base64 in {"cmd":"razer","data":{"pt":..}}
fn razer_packet(body: &[u8]) -> Vec<u8> {
    let mut p = vec![0xBB, 0x00, (body.len() - 1) as u8];
    p.extend_from_slice(body);
    let x = p.iter().fold(0u8, |a, b| a ^ b);
    p.push(x);
    p
}

fn razer_colors(colors: &[[u8; 3]]) -> Vec<u8> {
    let mut body = vec![0xB0, 0x01, colors.len() as u8];
    body.extend(colors.iter().flatten());
    razer_packet(&body)
}

fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |a, (i, &v)| a | (v as u32) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

fn send_razer(sock: &UdpSocket, ip: &str, pkt: &[u8]) -> Result<()> {
    send_on(sock, ip, "razer", json!({"pt": b64(pkt)}))
}

/// govee-screen.py
struct ScreenDaemon(std::process::Child);
impl Drop for ScreenDaemon {
    fn drop(&mut self) {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(self.0.id() as i32, 15) };
        let _ = self.0.wait();
    }
}

/// daemon is already running
fn screen_daemon_alive() -> bool {
    std::fs::metadata(FRAME).and_then(|m| m.modified()).map(|t| t.elapsed().unwrap_or_default() < Duration::from_secs(2)).unwrap_or(false)
}

fn spawn_screen_daemon() -> Result<ScreenDaemon> {
    let exe = std::env::current_exe()?;
    let candidates = [
        std::env::var("GOVEE_SCREEN_PY").ok().map(std::path::PathBuf::from),
        Some(std::path::PathBuf::from("/usr/share/govee-linux/govee-screen.py")),
        exe.ancestors().nth(4).map(|p| p.join("govee-screen.py")),
    ];
    let script = candidates.into_iter().flatten().find(|p| p.exists()).ok_or(anyhow!("govee-screen.py not found (set GOVEE_SCREEN_PY)"))?;
    std::process::Command::new("python3").arg(script).stdout(std::process::Stdio::null()).spawn().context("spawn govee-screen.py").map(ScreenDaemon)
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let (i, f) = (h.floor() as i32, h.fract());
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let c = match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    };
    c.map(|x| (x * 255.0).round() as u8)
}

fn gradient(cols: &[[u8; 3]], p: f32, cyclic: bool) -> [u8; 3] {
    let n = cols.len();
    if n == 1 {
        return cols[0];
    }
    let span = if cyclic { n } else { n - 1 } as f32;
    let x = p.rem_euclid(1.0) * span;
    let (i, f) = (x.floor() as usize, x.fract());
    let (a, b) = (cols[i % n], cols[(i + 1) % n]);
    [0, 1, 2].map(|k| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * f).round() as u8)
}

fn scale(c: [u8; 3], v: f32) -> [u8; 3] {
    c.map(|x| (x as f32 * v.clamp(0.0, 1.0)).round() as u8)
}

fn effect(ip: &str, n: usize, name: &str, speed: f32, palette: &[[u8; 3]]) -> Result<()> {
    let default: &[[u8; 3]] = match name {
        "fire" => &[[255, 30, 0], [255, 140, 0]],
        "breathe" | "chase" => &[[255, 255, 255]],
        _ => &[[255, 0, 0], [0, 255, 0], [0, 0, 255]],
    };
    let pal = if palette.is_empty() { default } else { palette };
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe { signal(2, on_sigint) };
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    send_razer(&sock, ip, &razer_packet(&[0xB1, 0x01]))?;
    let start = Instant::now();
    let mut rng: u32 = 0x9E37_79B9; // ponytail: xorshift, good enough for flicker
    let mut fire = vec![1.0f32; n];
    while !STOP.load(std::sync::atomic::Ordering::Relaxed) {
        let t = start.elapsed().as_secs_f32() * speed;
        let frame: Vec<[u8; 3]> = (0..n)
            .map(|i| {
                let x = i as f32 / n as f32;
                match name {
                    "rainbow" => hsv(x + t * 0.15, 1.0, 1.0),
                    "wave" => gradient(pal, x + t * 0.2, true),
                    "breathe" => {
                        let phase = t * 0.4;
                        scale(gradient(pal, (phase.floor() / pal.len() as f32).fract(), true), 0.05 + 0.95 * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()))
                    }
                    "chase" => {
                        let head = (t * 0.5).fract() * n as f32;
                        let d = ((i as f32 - head).rem_euclid(n as f32)) / n as f32; // 0 = head, trailing tail
                        scale(gradient(pal, x, false), (1.0 - d * 3.0).max(0.0).powi(2))
                    }
                    "fire" => {
                        rng ^= rng << 13; rng ^= rng >> 17; rng ^= rng << 5;
                        let r = (rng % 1000) as f32 / 1000.0;
                        fire[i] = fire[i] * 0.7 + (0.35 + 0.65 * r) * 0.3;
                        scale(gradient(pal, r, false), fire[i])
                    }
                    _ => return Err(anyhow!("unknown effect {name}")),
                }
                .pipe(Ok)
            })
            .collect::<Result<_>>()?;
        send_razer(&sock, ip, &razer_colors(&frame))?;
        std::thread::sleep(Duration::from_millis(50));
    }
    send_razer(&sock, ip, &razer_packet(&[0xB1, 0x00]))
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_sigint(_: i32) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

static BRI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

fn dreamview(targets: &[String], sat: f32, fps: u32, reverse: bool, edge: f32, bri: u32) -> Result<()> {
    BRI.store(bri.clamp(1, 100), std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(|| {
        for l in std::io::stdin().lines().map_while(Result::ok) {
            if let Ok(v) = l.trim().parse::<u32>() {
                BRI.store(v.clamp(1, 100), std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    let targets: Vec<(String, Vec<String>, bool)> = targets
        .iter()
        .map(|t| {
            let mut it = t.split(':');
            let ip = it.next().unwrap_or_default().to_string();
            let area = it.next().unwrap_or("all");
            let segs: u32 = it.next().map(|n| n.parse().context("segment count")).transpose()?.unwrap_or(0);
            let sides: Vec<String> = if area.contains(',') {
                area.split(',').map(str::to_string).collect()
            } else {
                vec![area.to_string(); segs.max(1) as usize]
            };
            Ok((ip, sides, area.contains(',') || segs > 0))
        })
        .collect::<Result<_>>()?;
    let _daemon = if screen_daemon_alive() { None } else { Some(spawn_screen_daemon()?) };
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe { signal(2, on_sigint) }; // SIGINT: leave razer mode before quitting
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    for (ip, sides, _) in targets.iter().filter(|t| t.2) {
        send_razer(&sock, ip, &razer_packet(&[0xB1, 0x01]))?;
        eprintln!("{ip}: razer mode on, {} segments", sides.len());
    }
    let (mut last_seq, mut last_bri) = (0, 0);
    let mut last: Vec<Vec<[u8; 3]>> = vec![vec![]; targets.len()];
    let tick = Duration::from_millis(1000 / fps.max(1) as u64);
    while !STOP.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(tick);
        let Some((w, h, seq, px)) = read_frame() else { continue };
        let bri = BRI.load(std::sync::atomic::Ordering::Relaxed);
        if seq == last_seq && bri == last_bri {
            continue; // static screen: still re-send when only brightness moved
        }
        (last_seq, last_bri) = (seq, bri);
        for (i, (ip, sides, razer)) in targets.iter().enumerate() {
            let mut c: Vec<[u8; 3]> = map_colors(&px, w, h, sides, sat, edge).into_iter().map(|c| scale(c, bri as f32 / 100.0)).collect();
            if reverse {
                c.reverse();
            }
            if c == last[i] {
                continue;
            }
            if !razer {
                send_on(&sock, ip, "colorwc", json!({"color": {"r": c[0][0], "g": c[0][1], "b": c[0][2]}, "colorTemInKelvin": 0}))?;
            } else {
                send_razer(&sock, ip, &razer_colors(&c))?;
            }
            last[i] = c;
        }
    }
    for (ip, _, _) in targets.iter().filter(|t| t.2) {
        send_razer(&sock, ip, &razer_packet(&[0xB1, 0x00]))?;
    }
    Ok(())
}


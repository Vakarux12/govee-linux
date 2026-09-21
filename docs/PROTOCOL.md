# Protocol research notes

> These notes describe how the unofficial `govee` CLI talks to Govee devices
> and cloud endpoints. They were reconstructed from traffic captures and
> runtime tracing of the vendor's Govee Desktop app for interoperability.
> This project is not affiliated with or endorsed by Govee.


Working notes for a RAM-friendly CLI/daemon replacement of Govee Desktop
2.40.60. Facts here are verified by inspection, traffic capture or runtime
tracing of the real app unless marked *(unverified)*. Keep this file updated
whenever something new is learned.

## 1. What the app is

- .NET 6 WPF + WebView2 (Edge Chromium) shell; managed code is protected by a
  JIT-time decrypting obfuscator, so decompiled bodies read
  `throw new Exception("Runtime exception")`. Names (types, methods, fields,
  resources) are intact; string literals are encrypted. Runtime tracing
  (EventPipe exceptions, Wine API traces, MITM) is how facts were obtained.
- Feature areas (assemblies/namespaces):
  - `Govee.Application`: `DeviceService`, `DreamviewService`,
    `MusicDreamviewService`, login/account, scenes, firmware, settings.
  - `Govee.Infrastructure.Shared`: `AwsIotHelper` (MQTT), `NetworkHelper`,
    `ServerLogHelper`, energy/beat detection (music mode), device entities.
  - `GoveeAPI/`: TouchSocket TCP/UDP + named pipes (LAN + local service).
  - `Govee.Service`: Windows-service style named-pipe device service.
  - Libraries: RestSharp (HTTP), M2Mqtt (MQTT), OpenCvSharp (frame
    analysis), SharpDX (DXGI capture), NAudio (music), QRCoder (login QR),
    Quartz (scheduled jobs), ManagedNativeWifi (SSID for pairing).

## 2. Identity and login

- Every BFF request carries headers (captured):
  `Authorization: Bearer <token>`, `clientid: <MAC of first IP-enabled
  adapter, e.g. AA:BB:CC:DD:EE:FF>`, `clientType: 7`, `appVersion: 2.40.60`,
  `pcversion: 2.40.60`, `clientName: <hostname>`, `model: <PC vendor>`,
  `sysVersion: Microsoft Windows 11`, `timezone`, `country`,
  `Accept-Language: en`, `User-Agent: RestSharp/112.0.0.0`.
- clientid source: WMI `Win32_NetworkAdapterConfiguration` (IPEnabled=true)
  `MacAddress`; also stored hashed in
  `%LOCALAPPDATA%\GoveeDesktop\config\device_config.ini` under `[networkaddress]`.
- QR login: `GET https://app2.govee.com/bff-app/pad/v1/qrcode/generate` ->
  `{"status":200,"data":{"qrCodeId","qrCodeAddr":"https://app-hd.govee.com/common/qr?qrCodeId=...","pollTime":1}}`
  then poll (endpoint in section 8 once captured). Scanning with the phone app
  authorises the qrCodeId.
- Email/password login: `gapp.govee.com` (endpoint in section 8 once captured).
- Token refresh: exists (RestSharp client), endpoint in section 8.

## 3. Device list and state

- REST: device list from the BFF after login (section 8).
- Live state/online status: **AWS IoT MQTT** over TLS 8883,
  endpoint `*.iot.us-east-1.amazonaws.com`, mutual
  TLS with a **per-account client certificate + PKCS#8 private key** delivered
  by the BFF (`GlobalParameters.Certificate`); part of the material is RSA
  PKCS#1 v1.5 encrypted and decrypted with a key embedded in the app. Root CA
  embedded resource `Govee.Infrastructure.Shared.Files.rootCA.crt` (Amazon).
  Library M2Mqtt; helper `AwsIotHelper` with `Publish`, `PublishAsync(topic,
  IotSend, timeout=3000)`, `PublishMultAsync`, `MqttClient_MqttMsgPublishReceived`,
  response cache `IotRspDic`. Topics/payload schema: section 8 *(to capture:
  needs an MQTT-level tap, e.g. run the CLI prototype with the same cert)*.
- On Linux: `paho-mqtt` + `tls_set(ca_certs, certfile, keyfile)`.

## 4. LAN control (already works from Linux)

- Govee LAN API (public spec): UDP multicast `239.255.255.250:4001` scan
  (`{"msg":{"cmd":"scan","data":{"account_topic":"reserve"}}}`), devices reply
  to port 4002 with `ip`, `device` (MAC-ish id), `sku`, versions; control by
  unicast UDP to device port 4003: `turn`, `brightness`, `colorwc`
  (`{"color":{"r","g","b"},"colorTemInKelvin"}`), `devStatus`. Needs "LAN
  Control" enabled per device in the mobile app. Only Wi-Fi devices; the
  Wi-Fi devices with "LAN Control" enabled in the mobile app are LAN-reachable (e.g. LED strip H61C3, Light Bars H6056).
- Per-segment streaming over LAN (verified on H61C3, 2026-09-15): the
  undocumented `razer` command, `{"msg":{"cmd":"razer","data":{"pt":"<b64>"}}}`
  to port 4003, packet `BB 00 <len> <cmd> <args> <xor-of-all-bytes>` where len
  counts the args (excl. cmd): enable `BB 00 01 B1 01 0A`, disable
  `BB 00 01 B1 00 0B`, frame `B0 01 <n> r g b ...` (n=15 for H61C3). No reply.
  `ptReal` (0x33 BLE framing) was never needed.

## 5. DreamView (screen -> lights)

- Capture: DXGI desktop duplication (`InitDxgiAsync`, `OutputDuplication`,
  `TryAcquireNextFrame`) or GDI+ (`Graphics.CopyFromScreen` of the full
  primary screen into a screen-sized bitmap, every frame). Setting names
  `ST_DrmCDxgi` / `ST_DrmCGdi` ("DXGI" / "GDI+").
- Analysis: bitmap -> OpenCV `Mat` (`BitmapToMat`), screen split into areas
  (`ScreenAreaType`, `GetValidScreenAreas(type,w,h)`, custom areas
  `GetValidScreenAreasForCustom`), per-area colour (`GetScreenColor(area,
  segment)`), mapped to device segments, sent as byte lists per device
  (`GetBytesForH6608orH6609` etc.). Modes: movie/game (`DA_MODE`), music
  (`MusicDreamviewService`, NAudio WASAPI loopback + `EnergyBeatDetector`).
- Linux equivalent: PipeWire/xdg-desktop-portal ScreenCast (see
  `govee-screen.py`), downscale to ~480px, average colours per area, send via
  LAN API (Wi-Fi devices) or BLE (light bars). 20 fps is plenty.

## 6. Local pieces worth knowing

- Named-pipe device service (`Govee.Service`, TouchSocket): the UI talks to a
  local service process for device IO; irrelevant for a CLI.
- Scene/effect images: `d1f2504ijhdyjw.cloudfront.net` (CDN), cached in
  `%LOCALAPPDATA%\GoveeDesktop\images\`.
- Razer Chroma integration (`RzChromaConnectAPI64.dll`), Windows Firewall
  rule management (`INetFwPolicy2`), auto-update via MSI: skip.

## 7. Wine port fixes (only relevant while running the Windows build)

See README.md: netprofm typelib (login), Cecil-patched .NET assemblies
(IoT cert import/decrypt/TLS client cert), PKCS12 iteration cap,
`govee-screen.py` + patched `CopyFromScreen` (DreamView).

## 8. Captured API contract

(Filled from `debug/flows-session.mitm`; dump with `python3 debug/dumpflows.py <file>`.)

Base URL `https://app2.govee.com`. All requests carry the section-2 headers.
Responses are `{"status":200,"message":"Success","data":{...}}`.

### Envelope encryption (some endpoints)
A few endpoints wrap the JSON in AES so it isn't plaintext on the wire:
- `refresh-token` request `{"d":..,"v":..}` / response `{"v":..,"token":..}`:
  `d`/`v` payload = base64(AES-256-ECB/PKCS7). Static key = the 32 ASCII bytes
  `D59AC2DEE1329A3FF9D3AC640D959F7E` (a build constant); a per-call session key
  (also 32 ASCII hex bytes) wraps the body. Decrypted `d` =
  `{"refreshToken":"<JWT>","type":0}`, decrypted response `v` =
  `{"refreshToken":"<JWT>","token":"<JWT>"}`. JWT is HS256; its `data.account`
  holds accountId/email/client(MAC)/uuid; access token `exp` = iat+900s
  (**15 min**), refresh token ~30 days. `data.account` is a JSON *string*.
- **Solved 2026-09-15**: `v` = base64(RSA-PKCS#1 v1.5(server RSA-512 pubkey,
  32-char hex session key)) (64 raw bytes); `d` = base64(AES-256-ECB(session
  key ASCII bytes, `{"refreshToken":..,"type":0}`)); response `data.v` is
  AES-ECB under the same session key. The pubkey (SPKI base64) is
  `EncryptionHelpers.publicKey`, set by the class constructor whose body only
  exists at runtime (the assembly decompiles as `throw "Runtime exception"`),
  so it was read with `debug/keytap` (a `DOTNET_STARTUP_HOOKS` DLL that
  reflects the field + calls RSAEncrypt/AESEncrypt and writes `C:\keytap.log`).
  Key is in `src/main.rs` (`REFRESH_PUBKEY`); the CLI refreshes desktop
  tokens automatically before any cloud call (openssl CLI does the crypto).
- On-disk token store `config/device_config.ini` `[sectionfirst]`:
  `value` = AES-256-ECB(static key, `{"EmailName","TokenDate","RefreshToken"}`),
  `value2` = AES-256-ECB(static key, access JWT). Static key is in plaintext
  UTF-16 in Govee.Infrastructure.Shared.dll (#US heap, next to
  `/bff-app/v1/pc/login`, `/bff-app/v1/pc/codes/verification`,
  `/bff-app/v1/tokens/refresh`, `/bff-app/v1/pc/logout`).
- Most endpoints (device list, feasts, scenes, iot-control...) are plain JSON.

### IoT key exchange (`POST /bff-app/v1/account-client/iot-key`)
Hybrid RSA+AES-GCM; this is how the MQTT client cert is delivered.
- Client generates an ephemeral RSA-2048 keypair and a 32-byte AES key.
  Request: `{"publicKey": <client RSA pub, base64 SPKI>, "encryptedAesKey":
  <client AES key, RSA-PKCS1 encrypted to Govee's fixed server pubkey>,
  "encryptedData": base64(nonce(12) || AESGCM(clientAesKey, "{}"))}`.
  Header `iotVersion: 0`.
- Response `{"encryptedAesKey": <server AES key, RSA-PKCS1 encrypted to the
  client's ephemeral pubkey>, "encryptedData": base64(nonce(12) ||
  AESGCM(serverAesKey, <plaintext>))}`.
- Plaintext:
  `{"certificatePem":"-----BEGIN CERTIFICATE-----...","privateKey":"-----BEGIN
  RSA PRIVATE KEY-----...","endpoint":"<id>-ats.iot.us-east-1.amazonaws.com",
  "port":8883,"p12":null,"p12Pass":null}`.
- GCM layout: ciphertext is `nonce(12) || ciphertext || tag(16)`; AAD empty.
- => a Linux client can call this endpoint itself to obtain its own cert/key,
  or reuse the ones the desktop app already fetched.

### Endpoints observed (all GET unless noted, all -> 200)
- `POST /bff-app/v2/pc/refresh-token` - refresh JWT (envelope, see above).
- `GET /bff-app/v1/pc/users` -> accountId, nickName, identity, `topic`
  ("GA/<hex>" account MQTT topic), headerUrl.
- `POST /bff-app/v1/account-client/iot-key` - MQTT cert/key (see above).
- `GET /bff-app/v1/pc/user/devices` -> `devices[]`: deviceId, `device`
  (8-byte colon id e.g. `AA:BB:CC:DD:EE:01`), sku, name, imgUrl, `topic`
  ("GD/<hex>" device topic), ic, ble/wifi hard+soft versions, `wifiMac`,
  spliceState, share, `ga` (account topic), gas. Plus `sort`.
- `GET /bff-app/v1/pc/devices/groups` -> `deviceGroups[]` (groupId, groupName,
  userDevicesVO = same device shape). Rooms.
- `GET /bff-app/pc/v1/support-skus` -> big `supportSkus[]` catalog: per SKU
  name, imgUrl, segmentNums, supported ble/wifi version ranges, feature flags
  (supportDiy/Scene/Feast/Music/Razer/Widget/Color/Snapshot/Speed),
  colorTemperature range, `supportConnectType` (bitmask: 1=BLE,2=Wi-Fi/IoT...),
  `colorModeIotType`, etc. Drives what the UI offers.
- `GET /bff-app/v1/pc/settings` -> key/value app settings
  (UseAPIChecked, ApplianceTemp, CpuLevel, ColorRenderingMode).
- `GET /bff-app/v1/pc/check-version?version=2.40.60` -> pc/razer update pkgs.
- `GET /bff-app/v1/newest/describe?pcVersion=` -> what's-new (empty here).
- `GET /bff-app/v1/pc/device-list/light-eff` -> per-device light effects.
- `GET /bff-app/v1/pc/devices/scenes?device=&sku=&goodsType=0` -> scene list.
- `GET /bff-app/v1/pc/devices/diy?device=&sku=&goodsType=0` and
  `.../diy/pages?...&groupId=-1&lastId=-1` -> DIY effects.
- `GET /bff-app/v1/pc/scene-relate-details?device=&sku=` -> active scene params
  (`scenceParamId`, speedIndex...).
- `GET /bff-app/v1/pc/snapshots?device=&sku=&snapshotId=-1` -> saved snapshots.
- `GET /bff-app/v2/pc/user-colors` -> saved color blocks/stripes (rgb as signed
  int32 ARGB, e.g. -16711936 = 0xFF00FF00 green).
- `GET /bff-app/v1/pc/feasts` / `PUT /bff-app/v1/pc/feasts` - DreamView "feast"
  config + live update. PUT body: `{"feastId","name","brightness"(0-100),
  "saturation","sensitivity","on"(0/1),"devices":[{"device","sku","brightness",
  "type":0,"segments":[{"segmentIndex","areaIndex","color":"#AARRGGBB"}]}],
  "templates":[]}`. Toggling DreamView = same PUT with on 0/1; brightness slider
  = repeated PUTs. Note: this is the *cloud* record of the mapping; the live
  per-frame colors go out over LAN/IoT, not this endpoint.
- `GET /bff-app/v1/pc/music/feasts` + `/music/light-effect/list` - music mode
  configs (colors CSV, sensitivity, musicFeastConfigId -> named effect).
- `GET /bff-app/v1/pc/scene-feast`, `/oneclick` (Get Home/Leave Home/Party
  presets, presetId per device), `/razer`, `/questionnaire-platform/red-prompt`.
- `POST /bff-app/v1/pc/iot-control` `{"device","sku","sceneParamId","id","type":1}`
  -> apply a cloud scene to a device (routed to the device via IoT/MQTT).
- `POST /bff-app/v1/pc/exception-report-log` - the app's own error telemetry
  (leaked source path `D:\GoveeDesktop1\GoveeDesktop\...`, useful for line refs).

### MQTT (AWS IoT)
- Endpoint `<id>-ats.iot.us-east-1.amazonaws.com:8883`, mutual TLS
  with the cert/key from `iot-key`. Amazon Root CA 1.
- Topics: account `GA/<hex>` (from users.topic / device.ga), per-device
  `GD/<hex>` (from device.topic). Publish control / subscribe state on these.
  Exact JSON payload schema: TODO (tap MQTT with the cert, or trace M2Mqtt
  `Publish`/`MqttMsgPublishReceived`).

### Example devices (reference)
- sku H61C3 - LED strip, 15 segments, found on LAN when "LAN Control" is on.
- sku H6056 - Light Bars, 6 segments, Wi-Fi, LAN-reachable with LAN Control on.

## 9. Capturing the Wine app's traffic (no iptables)

.NET under Wine reads the WinHTTP/IE proxy from
`HKCU\...\Internet Settings\Connections\DefaultConnectionSettings`
(binary blob; `ProxyEnable`/`ProxyServer` alone are ignored). Blob:
`46000000 01000000 03000000 <len><proxy> <len><bypass> 00000000 + 32 zero
bytes`, proxy `127.0.0.1:8080`, bypass `<local>`; write it with
`wine reg add ... /t REG_BINARY /d <hex>`, run `mitmdump -w file`, launch
GoveeDesktop.exe (mitmproxy CA is already a system trust anchor). Delete the
blob and shred the capture afterwards (it holds the account JWT).

## 10. CLI status (`src/`, Rust)

`govee scan|on|off|bri|color|temp|status|segs|effect|dreamview` over the LAN
API, verified against the H61C3 strips (192.168.1.100 / .101). `dreamview
ip[:area]...` reuses `govee-screen.py` frames (/dev/shm/govee-screen);
`ip:area:N` streams N segments in `razer` mode. `paint ip hex...` sets
segments directly. See `govee --help`.

Cloud (2026-09-15): `login [email]`, `devices`, `scenes`, `scene`; token in
`~/.config/govee/cloud.json` (mode 600). HTTPS is `curl` via `Command`, no
crates added. `govee-gui.py` wraps all of it (PyQt6: login dialog with
password + QR panes, Scenes tab, product/scene/avatar images cached in
`~/.cache/govee/img`).

## 11. Cloud auth without the envelope

- **Password login = phone-app endpoint**, no envelope:
  `POST /account/rest/account/v1/login` `{"email","password","client":<32 hex>}`
  with headers `clientId: <same 32 hex>`, `clientType: 1`, `appVersion: 5.6.01`,
  `iotVersion: 0`, `timestamp: <ms>`, phone User-Agent. Response
  `data.client.{token, refreshToken, tokenExpireCycle(=57 days), accountId,
  topic, headPortrait}`. That token works on the `/bff-app/v1/pc/*` endpoints
  too (send it with the same phone headers). Wrong password -> 400, unknown
  email -> 451.
- **QR login (desktop flow)**: `GET /bff-app/pad/v1/qrcode/generate` then poll
  `GET /bff-app/pad/v1/qrcode/status?qrCodeId=` every `pollTime` s:
  `qrCodeStatus` UNUSED -> CONFIRMING -> final with `loginVo`
  `{token, refreshToken, tokenExpireCycle: 15, tokenExpireTime(ms), accountId,
  email, topic, headPortrait, isSavvyUser}`. Both calls answer
  `500 "service is busy please try again later"` unless **all** of `model`,
  `clientName`, `sysVersion`, `timezone`, `country` headers are present (plus
  clientid/clientType 7/appVersion/pcversion). The token is the 15-minute PC
  one; the CLI refreshes it via the §8 envelope (`refresh()` in main.rs) when
  it is within a minute of `exp`, so a QR session lives as long as the refresh
  token (~30 days). (`PUT /bff-app/v1/tokens/refresh` answers "must not be
  null" to every plain-JSON body; unused.)

## 12. Scenes (LAN first)

- Catalogue: `GET /appsku/v1/light-effect-libraries?sku=&device=` (works with
  either token; ~5 MB, cached a day in `~/.config/govee/scenes-<sku>.json`).
  `categories[].scenes[]{sceneId, sceneName, iconUrls[3], lightEffects[]
  {scenceParamId, scenceName, scenceParam(base64), sceneCode}}`. The PC
  endpoint `/bff-app/v1/pc/devices/scenes` has only ids (cloud apply).
- **LAN apply** = `ptReal` with BLE-style 20-byte packets (payload padded to
  19 + XOR checksum), same framing govee2mqtt uses (`SetSceneCode` in its
  `ble.rs`, reference bytes reproduced in the CLI test):
  `a3 00 01 <lines> 02 <scenceParam bytes...>` split into 19-byte lines,
  continuation lines `a3 <i>`, last line `a3 ff`, then `33 05 04 <code lo> <hi>`.
  Built-in scenes (empty `scenceParam`, e.g. Rainbow 22, Sunrise 0) need only
  the `33 05 04` packet. Verified on the H61C3: Rainbow and Dracarys (code
  16182, param-based; code alone gives a generic blue/red effect).
  `scenceParam` itself is `<count> (<len> <bytes>)*` sub-effects, not needed
  to decode.
- Cloud apply (fallback when the light doesn't answer on LAN): `POST
  /bff-app/v1/pc/iot-control {"device","sku","sceneParamId":scenceParamId,
  "id":sceneId,"type":1}`.

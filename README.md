# govee-linux

Unofficial Govee lights app controller for Linux: a Rust CLI plus an optional Qt GUI.
Use lights directly over your LAN (no account needed) or with Govee account and use DreamView screen.

> **Unofficial project.** Not affiliated with or endorsed by Govee
> (Shenzhen Intellirocks Tech. Co., Ltd.). `Govee` and device names are
> trademarks of their owner. This tool only interoperates with devices
> you own using your own account. See [License](#license).

## Contents

- `src/main.rs` - the CLI (LAN control + cloud + effects + DreamView)
- `govee-gui.py` - optional PyQt6 desktop app which uses the cli
- `govee-screen.py` - screen capture helper (PipeWire/xdg-desktop-portal)
- `docs/PROTOCOL.md` - protocol research notes for other projects (endpoints, envelope crypto, LAN framing)

## Requirements

| Need | Provides | Required for |
|------|----------|--------------|
| Rust toolchain (`cargo`) | building `govee` | everything |
| `curl` | HTTPS to `app2.govee.com` (no HTTP crates) | cloud commands |
| `openssl` CLI | AES-ECB + RSA for the desktop token-refresh envelope | QR login sessions |
| `qrencode` | terminal QR rendering | `login` without email |
| Python 3 + PyQt6 + `qrencode` | `govee-gui.py` | GUI |
| Python 3 + PyGObject, GStreamer, `gst-plugin-pipewire`, `xdg-desktop-portal` | `govee-screen.py` | `dreamview` |

Lights must have **LAN Control** enabled per device in the Govee Home mobile app, and the PC must be on the same subnet (UDP 4001–4003).

## Install

```sh
cargo build --release
# optional:
install -Dm755 target/release/govee ~/.local/bin/govee
./target/release/govee --help
```

## Quick start (LAN)

```sh
govee scan                                  # ip<TAB>sku<TAB>id
govee on 192.168.1.100
govee bri 192.168.1.100 60
govee color 192.168.1.100 255 128 0
govee temp 192.168.1.100 4000
govee status 192.168.1.100
```

Per-segment painting:

```sh
govee segs 192.168.1.100 ff0000 00ff00 0000ff   # one hex colour per segment
govee segs 192.168.1.100                        # leave razer mode
govee effect 192.168.1.100 15 rainbow           # animated until Ctrl-C
govee effect 192.168.1.100 15 wave --colors ff0000 0000ff --speed 1.5
```

Effects: `rainbow`, `wave`, `breathe`, `chase`, `fire`.
`--colors` takes `rrggbb` values (defaults depend on the effect).

## Cloud (account, scenes)

```sh
govee login you@example.com   # password prompt (or GOVEE_PASSWORD env var)
govee login                   # or scan the printed QR code with the phone app
govee devices                 # device<TAB>sku<TAB>name<TAB>topic<TAB>imgUrl
govee scenes <device-id> <sku>
govee scene 192.168.1.100 <device-id> <sku> <scene-id-or-name>
govee scene - <device-id> <sku> <scene-id-or-name>   # via cloud when device is unavailable via LAN
```

Notes:

- Password login uses the phone-app account endpoint; the token lasts about two months.
- QR login yields a 15-minute desktop token that the CLI refreshes automatically while the ~30-day refresh token is valid.
- The session is stored in `~/.config/govee/cloud.json` (mode `600`).
- Scene catalogues cache for a day as `~/.config/govee/scenes-<SKU>.json` (~5 MB per fetch), but it should make the cli run smoother.
- Some values the CLI must send byte-identical for interop (app version strings, a static AES key, an RSA public key) are build constants extracted from the Govee desktop app; see `docs/PROTOCOL.md`.

## DreamView (screen → lights)

```sh
govee dreamview 192.168.1.100:all:15 192.168.1.101:left,left,top,right
```

Targets are `ip[:area[:segments]]`:

- Areas: `all`, `left`, `right`, `top`, `bottom`, `topleft`, `topright`, `bottomleft`, `bottomright`, `center`, or a percent rect `WxH+X+Y` (e.g. `50x20+25+0` = top-middle fifth).
- With a segment count, the area is split along its long axis and streamed per segment (`razer` mode).
- A comma list assigns one side per segment (`ip:left,left,top,right`); each side is split into as many bands as it was given.

Flags: `--sat` (saturation boost, default 1.6), `--fps` (default 15), `--reverse` (strip mounted backwards), `--edge` (side/corner thickness as a screen fraction), `--bri` (1–100; also adjustable live by piping one number per line to stdin). Set `GOVEE_DEBUG=1` for protocol dumps.

The first `dreamview` run spawns `govee-screen.py` (portal ScreenCast -> downscaled BGRx frames in `/dev/shm/govee-screen`) unless a fresh frame shows one is already running.

I would REALLY recommend using the GUI version for razer.

## GUI

```sh
pip install PyQt6            # plus qrencode for the QR pane
./govee-gui.py
```

Sidebar lists LAN lights (scan or add by IP); sign in for cloud names, product images, and scenes. Tabs:

- **Light** - power, brightness, warmth, colour presets / custom picker.
- **Scenes** - filterable catalogue, double-click to apply or press the button at the bottom after selecting (LAN first, then cloud if the light didn't answer).
- **DreamView** - click a numbered screen zone, then click the light segments that should show it (right-click = whole-screen average); Spread/Reverse helpers(CLI --reverse), preview, Start/Stop.

GUI state (devices, segment maps, sat/fps) is stored in `~/.config/govee/gui.json`; image cache is in `~/.cache/govee/img`.

## Files

| Path | What |
|------|------|
| `~/.config/govee/cloud.json` | session tokens (`600`) |
| `~/.config/govee/scenes-<SKU>.json` | scene catalogue cache (1 day) |
| `~/.config/govee/gui.json` | GUI state |
| `~/.cache/govee/img/` | product/scene/avatar image cache |
| `/dev/shm/govee-screen` | live screen frame (while DreamView runs) |

## Security & privacy

- Your password is only sent to Govee's own login endpoint over HTTPS. Prefer the prompt over `GOVEE_PASSWORD` (visible in the environment).
- Your tokens live in `cloud.json`. Please don't paste that file anywhere.
- No telemetry in here. The CLI makes no network requests except the Govee endpoints and LAN UDP documented above.

## Contributing

Issues and pull requests welcome. Please:

- Keep it dependency-light (there's a reason why I use `curl`/`openssl` instead of adding crates).
- Run `cargo build --release` before submitting.
- Don't commit tokens, captures (`.mitm`/`.nettrace`), screenshots of QR codes, or files from `~/.config/govee/`.
- Note the protocol source in `docs/PROTOCOL.md` when adding commands.

## Usage of code

You can freely use the code for any other projects, but following the license is needed.

## Usage of LLM
LLMs were used to make this project, more specifically reverse engineering the vendors desktop app and wiring it to the CLI.

Files that were touched by LLMs:

- `docs/PROTOCOL.md`


## License

GPL-3.0-only. See [LICENSE](LICENSE).

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

## Acknowledgments

- LAN scene framing matches the approach used by [govee2mqtt](https://github.com/wez/govee2mqtt) (`SetSceneCode`).
- Cloud/API behaviour was reconstructed from the vendor's Govee Desktop app for interoperability; details in `docs/PROTOCOL.md`.

#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""govee-screen - feed the Linux screen to `govee dreamview`.

Grabs the monitor through the xdg-desktop-portal ScreenCast API (PipeWire)
and writes downscaled BGRx frames to /dev/shm/govee-screen, which the
`govee dreamview` command reads. Usually started automatically by
`govee dreamview`; can also run standalone.

Frame file: 'GVSC' | u32 width | u32 height | u32 screen_w | u32 screen_h |
            u64 seq | BGRx pixels (width*height*4)
"""
import os, struct, sys, signal
import gi
gi.require_version("Gst", "1.0")
from gi.repository import Gst, Gio, GLib

OUT = "/dev/shm/govee-screen"
TOKEN_FILE = os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")),
                          "govee-desktop", "screencast-token")
WIDTH = int(os.environ.get("GOVEE_SCREEN_WIDTH", "480"))
FPS = int(os.environ.get("GOVEE_SCREEN_FPS", "20"))

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
portal = Gio.DBusProxy.new_sync(bus, Gio.DBusProxyFlags.NONE, None, "org.freedesktop.portal.Desktop",
                                "/org/freedesktop/portal/desktop", "org.freedesktop.portal.ScreenCast", None)
sender = bus.get_unique_name()[1:].replace(".", "_")
counter = 0


def call(method, args, extra_opts):
    """Portal request/response dance: returns the results dict."""
    global counter
    counter += 1
    token = f"govee{counter}"
    path = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"
    loop = GLib.MainLoop()
    result = {}

    def on_response(conn, s, p, iface, sig, params):
        code, results = params.unpack()
        result["code"], result["results"] = code, results
        loop.quit()

    sub = bus.signal_subscribe("org.freedesktop.portal.Desktop", "org.freedesktop.portal.Request", "Response",
                               path, None, Gio.DBusSignalFlags.NONE, on_response)
    opts = {"handle_token": GLib.Variant("s", token)}
    opts.update(extra_opts)
    portal.call_sync(method, GLib.Variant.new_tuple(*args, GLib.Variant("a{sv}", opts)),
                     Gio.DBusCallFlags.NONE, -1, None)
    loop.run()
    bus.signal_unsubscribe(sub)
    if result["code"] != 0:
        sys.exit(f"portal {method} failed/cancelled (code {result['code']})")
    return result["results"]


session_token = "goveesession"
res = call("CreateSession", [], {"session_handle_token": GLib.Variant("s", session_token)})
session = res["session_handle"]

opts = {"types": GLib.Variant("u", 1), "multiple": GLib.Variant("b", False),
        "cursor_mode": GLib.Variant("u", 1), "persist_mode": GLib.Variant("u", 2)}
if os.path.exists(TOKEN_FILE):
    opts["restore_token"] = GLib.Variant("s", open(TOKEN_FILE).read().strip())
call("SelectSources", [GLib.Variant("o", session)], opts)

res = call("Start", [GLib.Variant("o", session), GLib.Variant("s", "")], {})
if "restore_token" in res:
    os.makedirs(os.path.dirname(TOKEN_FILE), exist_ok=True)
    open(TOKEN_FILE, "w").write(res["restore_token"])
node_id, props = res["streams"][0]
screen_w, screen_h = props.get("size", (0, 0))

fdlist = portal.call_with_unix_fd_list_sync("OpenPipeWireRemote",
                                            GLib.Variant.new_tuple(GLib.Variant("o", session), GLib.Variant("a{sv}", {})),
                                            Gio.DBusCallFlags.NONE, -1, None, None)[1]
fd = fdlist.get(0)

Gst.init(None)
height = max(2, (WIDTH * screen_h // screen_w) // 2 * 2) if screen_w else 270
pipeline = Gst.parse_launch(
    f"pipewiresrc fd={fd} path={node_id} do-timestamp=true ! videorate drop-only=true "
    f"! video/x-raw,framerate={FPS}/1 ! videoconvert ! videoscale "
    f"! video/x-raw,format=BGRx,width={WIDTH},height={height} "
    f"! appsink name=sink emit-signals=true max-buffers=1 drop=true sync=false")
sink = pipeline.get_by_name("sink")
seq = 0


def stream_size():
    """Physical size of the PipeWire stream (portal 'size' is logical/scaled; Wine's
    screen uses physical pixels, e.g. 3072x1920 vs 1755x1097 on HiDPI)."""
    global screen_w, screen_h
    src = pipeline.iterate_sources().next()[1]
    caps = src.get_static_pad("src").get_current_caps()
    if caps:
        st = caps.get_structure(0)
        screen_w, screen_h = st.get_value("width"), st.get_value("height")


def on_sample(s):
    global seq
    try:
        return handle_sample(s)
    except Exception as e:  # never let a callback die silently
        print("govee-screen: frame error:", repr(e), file=sys.stderr, flush=True)
        return Gst.FlowReturn.OK


def handle_sample(s):
    global seq
    sample = s.emit("pull-sample")
    buf = sample.get_buffer()
    st = sample.get_caps().get_structure(0)
    w, h = st.get_value("width"), st.get_value("height")
    if seq == 0:
        stream_size()
    ok, info = buf.map(Gst.MapFlags.READ)
    if not ok:
        return Gst.FlowReturn.OK
    try:
        seq += 1
        tmp = OUT + ".tmp"
        with open(tmp, "wb") as f:
            f.write(struct.pack("<4sIIIIQ", b"GVSC", w, h, screen_w, screen_h, seq))
            f.write(info.data)
        os.replace(tmp, OUT)  # atomic swap, readers never see a torn frame
    finally:
        buf.unmap(info)
    return Gst.FlowReturn.OK


sink.connect("new-sample", on_sample)
pipeline.set_state(Gst.State.PLAYING)
print(f"govee-screen: {screen_w}x{screen_h} -> {WIDTH}x{height} @ {FPS}fps into {OUT}", flush=True)
loop = GLib.MainLoop()
for sig in (signal.SIGINT, signal.SIGTERM):
    GLib.unix_signal_add(GLib.PRIORITY_HIGH, sig, lambda *_: loop.quit())
try:
    loop.run()
finally:
    pipeline.set_state(Gst.State.NULL)
    try:
        os.remove(OUT)
    except OSError:
        pass

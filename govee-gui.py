#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
import colorsys, hashlib, json, os, shutil, signal, subprocess, sys, threading, urllib.request
from PyQt6.QtCore import Qt, QObject, QRectF, QSize, pyqtSignal, QTimer
from PyQt6.QtGui import QColor, QPainter, QPen, QFont, QAction, QPixmap, QIcon
from PyQt6.QtWidgets import (QApplication, QMainWindow, QWidget, QListWidget, QListWidgetItem, QVBoxLayout, QHBoxLayout,
                             QGridLayout, QLabel, QPushButton, QSlider, QColorDialog, QTabWidget, QCheckBox, QSpinBox,
                             QDoubleSpinBox, QLineEdit, QFrame, QScrollArea, QSizePolicy, QToolButton, QStatusBar, QDialog)

HERE = os.path.dirname(os.path.abspath(__file__))
_bins = [p for p in (os.path.join(HERE, "target/release/govee"), os.path.join(HERE, "target/debug/govee"), shutil.which("govee")) if p and os.path.exists(p)]
GOVEE = max(_bins, key=os.path.getmtime, default=None)  # newest build
if not GOVEE:
    sys.exit("govee binary not found: run `cargo build` first")
IMG_CACHE = os.path.join(os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache")), "govee", "img")
CONFIG = os.path.join(os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")), "govee", "gui.json")
CLOUD = os.path.join(os.path.dirname(CONFIG), "cloud.json")
DEFAULT_SEGS = {"H61C3": 15, "H6056": 6}
PRESETS = ["#ff0000", "#ff8000", "#ffff00", "#00ff00", "#00ffff", "#0040ff", "#8000ff", "#ff00ff", "#ffffff"]

ZH, ZV = 4, 3


def zones():
    cw, ch = 100 // ZH, 100 // ZV
    ring = [(i, 0) for i in range(ZH)] + [(ZH - 1, j) for j in range(1, ZV)] \
        + [(i, ZV - 1) for i in reversed(range(ZH - 1))] + [(0, j) for j in reversed(range(1, ZV - 1))]
    return [(str(n + 1), f"{cw}x{ch}+{i * cw}+{j * ch}", i * cw, j * ch, cw, ch) for n, (i, j) in enumerate(ring)]


ZONES = zones()
AREA = {"all": "all", **{z[0]: z[1] for z in ZONES}}
ZONE_HEX = {"all": "#ffffff"}
for n, z in enumerate(ZONES):
    ZONE_HEX[z[0]] = "#%02x%02x%02x" % tuple(int(c * 255) for c in colorsys.hsv_to_rgb(n / len(ZONES), 1, 1))


def cli(*args):
    return subprocess.run([GOVEE, *[str(a) for a in args]], capture_output=True, text=True)


def fetch_image(url):
    if not url:
        return b""
    f = os.path.join(IMG_CACHE, hashlib.sha1(url.encode()).hexdigest())
    try:
        return open(f, "rb").read()
    except OSError:
        pass
    try:
        data = urllib.request.urlopen(url, timeout=15).read()
    except (OSError, ValueError):
        return b""
    os.makedirs(IMG_CACHE, exist_ok=True)
    open(f, "wb").write(data)
    return data


class Bus(QObject):
    done = pyqtSignal(object, object)


class LoginDialog(QDialog):
    def __init__(self, parent):
        super().__init__(parent)
        self.setWindowTitle("Govee account")
        self.bus = Bus(); self.bus.done.connect(lambda cb, r: cb(r))
        self.proc = None
        v = QVBoxLayout(self)
        g = QGridLayout()
        self.email = QLineEdit(); self.email.setPlaceholderText("email")
        self.password = QLineEdit(); self.password.setPlaceholderText("password")
        self.password.setEchoMode(QLineEdit.EchoMode.Password); self.password.returnPressed.connect(self.sign_in)
        g.addWidget(QLabel("Email"), 0, 0); g.addWidget(self.email, 0, 1)
        g.addWidget(QLabel("Password"), 1, 0); g.addWidget(self.password, 1, 1)
        self.btn = QPushButton("Sign in"); self.btn.clicked.connect(self.sign_in)
        g.addWidget(self.btn, 2, 1)
        v.addLayout(g)
        v.addWidget(QLabel("<b>or</b> scan with the Govee app (Profile > scan icon):"), alignment=Qt.AlignmentFlag.AlignHCenter)
        self.qr = QLabel("generating QR code..."); self.qr.setFixedSize(260, 260); self.qr.setAlignment(Qt.AlignmentFlag.AlignCenter)
        v.addWidget(self.qr, alignment=Qt.AlignmentFlag.AlignHCenter)
        self.msg = QLabel(""); self.msg.setWordWrap(True)
        v.addWidget(self.msg)
        self.start_qr()

    def sign_in(self):
        email, pw = self.email.text().strip(), self.password.text()
        if not email or not pw:
            return
        self.btn.setEnabled(False); self.msg.setText("signing in...")
        env = {**os.environ, "GOVEE_PASSWORD": pw}
        threading.Thread(target=lambda: self.bus.done.emit(self.signed_in, subprocess.run(
            [GOVEE, "login", email], capture_output=True, text=True, env=env)), daemon=True).start()

    def signed_in(self, r):
        self.btn.setEnabled(True)
        if r.returncode:
            self.msg.setText(r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "login failed")
        else:
            self.accept()

    def start_qr(self):
        self.proc = subprocess.Popen([GOVEE, "login"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

        def pump(p=self.proc):
            for line in p.stdout:
                if line.startswith("https://"):
                    self.bus.done.emit(self.show_qr, line.strip())
            p.wait()
            self.bus.done.emit(self.qr_done, p)
        threading.Thread(target=pump, daemon=True).start()

    def show_qr(self, url):
        png = subprocess.run(["qrencode", "-o", "-", "-t", "PNG", "-s", "6", "-m", "2", url], capture_output=True).stdout
        pm = QPixmap(); pm.loadFromData(png)
        if pm.isNull():
            self.qr.setText(f"install qrencode, or open\n{url}")
        else:
            self.qr.setPixmap(pm.scaled(260, 260, Qt.AspectRatioMode.KeepAspectRatio))

    def qr_done(self, p):
        if p is not self.proc:
            return
        self.proc = None
        if p.returncode == 0:
            self.accept()
        elif self.isVisible():
            err = p.stderr.read().strip().splitlines()
            self.msg.setText(err[-1] if err else "QR login failed")
            self.qr.setText("expired, reopen to retry")

    def done(self, code):
        if self.proc:
            self.proc.terminate(); self.proc = None
        super().done(code)


class ScreenZones(QWidget):
    picked = pyqtSignal(str)

    def __init__(self):
        super().__init__()
        self.selected = "1"
        self.setFixedSize(360, 202)
        self.setCursor(Qt.CursorShape.PointingHandCursor)

    def rect_of(self, z):
        _, _, x, y, w, h = z
        W, H = self.width(), self.height()
        return QRectF(x * W / 100 + 2, y * H / 100 + 2, w * W / 100 - 4, h * H / 100 - 4)

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        p.fillRect(self.rect(), QColor("#1a1a1a"))
        p.setFont(QFont(self.font().family(), 11, QFont.Weight.Bold))
        for z in ZONES:
            r = self.rect_of(z)
            p.setPen(QPen(QColor("white" if z[0] == self.selected else "#1a1a1a"), 3))
            p.setBrush(QColor(ZONE_HEX[z[0]]))
            p.drawRoundedRect(r, 6, 6)
            p.setPen(QColor("black"))
            p.drawText(r, Qt.AlignmentFlag.AlignCenter, z[0])
        p.setPen(QColor("#666"))
        p.drawText(self.rect(), Qt.AlignmentFlag.AlignCenter, "screen")

    def mousePressEvent(self, e):
        for z in ZONES:
            if self.rect_of(z).contains(e.position()):
                self.selected = z[0]
                self.update()
                self.picked.emit(z[0])


class DeviceMap(QFrame):
    def __init__(self, app, ip, sku, cfg):
        super().__init__()
        self.app, self.ip, self.sku = app, ip, sku
        self.setFrameShape(QFrame.Shape.StyledPanel)
        v = QVBoxLayout(self)
        head = QHBoxLayout()
        self.on = QCheckBox(f"{sku}   {ip}")
        self.on.setChecked(cfg.get("on", True))
        self.on.toggled.connect(lambda _: self.paint())
        head.addWidget(self.on)
        head.addStretch()
        spread = QToolButton(); spread.setText("Spread"); spread.setToolTip("Assign zones clockwise across all segments")
        spread.clicked.connect(self.spread)
        rev = QToolButton(); rev.setText("Reverse"); rev.setToolTip("Flip segment order (strip mounted the other way)")
        rev.clicked.connect(self.reverse)
        head.addWidget(spread); head.addWidget(rev)
        head.addWidget(QLabel("segments"))
        self.segs = QSpinBox(); self.segs.setRange(0, 60)
        self.segs.setValue(cfg.get("segs", DEFAULT_SEGS.get(sku, 0)))
        self.segs.setToolTip("0 = one colour for the whole light")
        self.segs.valueChanged.connect(lambda _: self.rebuild())
        head.addWidget(self.segs)
        v.addLayout(head)
        self.tiles = QWidget()
        self.grid = QGridLayout(self.tiles); self.grid.setSpacing(3); self.grid.setContentsMargins(0, 0, 0, 0)
        v.addWidget(self.tiles)
        self.sides = list(cfg.get("sides", []))
        self.rebuild(initial=True)

    def rebuild(self, initial=False):
        n = max(self.segs.value(), 1)
        if len(self.sides) != n:
            self.sides = (self.sides + ["all"] * n)[:n]
            if not initial and self.segs.value():
                self.spread(paint=False)
        while self.grid.count():
            self.grid.takeAt(0).widget().deleteLater()
        for i, z in enumerate(self.sides):
            b = QPushButton(str(i + 1) if self.segs.value() else "whole light")
            b.setFixedSize(34 if self.segs.value() else 100, 30)
            b.setToolTip(f"segment {i + 1}: zone {z}. Click to paint with the selected zone, right-click for all")
            b.setStyleSheet(f"background:{ZONE_HEX[z]}; color:black; border-radius:4px; font-weight:bold;")
            b.clicked.connect(lambda _, i=i: self.assign(i, self.app.zones.selected))
            b.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
            b.customContextMenuRequested.connect(lambda _, i=i: self.assign(i, "all"))
            self.grid.addWidget(b, i // 15, i % 15)
        if not initial:
            self.paint()

    def assign(self, i, zone):
        self.sides[i] = zone
        self.rebuild()

    def spread(self, paint=True):
        n = len(self.sides)
        self.sides = [ZONES[i * len(ZONES) // n][0] for i in range(n)]
        if paint:
            self.rebuild()

    def reverse(self):
        self.sides.reverse()
        self.rebuild()

    def paint(self):
        if not self.app.live.isChecked() or self.app.dream:
            return
        if not self.on.isChecked():
            self.app.bg(cli, "segs", self.ip)
        elif self.segs.value() == 0:
            c = QColor(ZONE_HEX[self.sides[0]])
            self.app.bg(cli, "color", self.ip, c.red(), c.green(), c.blue())
        else:
            self.app.bg(cli, "segs", self.ip, *(ZONE_HEX[z][1:] for z in self.sides))

    def target(self):
        areas = [AREA[z] for z in self.sides]
        return f"{self.ip}:{areas[0]}:0" if self.segs.value() == 0 else f"{self.ip}:{','.join(areas)}"

    def config(self):
        return {"on": self.on.isChecked(), "segs": self.segs.value(), "sides": self.sides}


class Main(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Govee")
        self.bus = Bus()
        self.bus.done.connect(lambda cb, r: cb(r))
        self.dream = None
        self.maps = {}
        try:
            self.cfg = json.load(open(CONFIG))
        except (OSError, ValueError):
            self.cfg = {"devices": {}}

        root = QWidget(); self.setCentralWidget(root)
        h = QHBoxLayout(root)

        # -- sidebar: devices --
        side = QVBoxLayout()
        self.list = QListWidget(); self.list.setFixedWidth(230); self.list.setIconSize(QSize(40, 40))
        self.list.currentItemChanged.connect(lambda cur, _: self.select(cur))
        side.addWidget(QLabel("<b>Lights</b>"))
        side.addWidget(self.list)
        add = QHBoxLayout()
        self.add_ip = QLineEdit(); self.add_ip.setPlaceholderText("IP address")
        self.add_ip.returnPressed.connect(self.add_device)
        add.addWidget(self.add_ip)
        b = QPushButton("Add"); b.clicked.connect(self.add_device); add.addWidget(b)
        side.addLayout(add)
        b = QPushButton("Rescan"); b.clicked.connect(self.scan); side.addWidget(b)
        self.account = QPushButton(); self.account.clicked.connect(self.login)
        side.addWidget(self.account)
        self.cloud = {}  # device id -> cloud device record (name, sku, topic)
        h.addLayout(side)

        tabs = QTabWidget()
        tabs.addTab(self.light_tab(), "Light")
        tabs.addTab(self.scene_tab(), "Scenes")
        tabs.addTab(self.dream_tab(), "DreamView")
        h.addWidget(tabs, 1)
        self.setStatusBar(QStatusBar())
        QTimer.singleShot(0, self.scan)
        QTimer.singleShot(0, self.cloud_sync)

    def bg(self, fn, *args, cb=None):
        threading.Thread(target=lambda: self.bus.done.emit(cb or (lambda r: None), fn(*args)), daemon=True).start()

    def say(self, text):
        self.statusBar().showMessage(text, 5000)

    def image(self, url, cb):
        def done(data):
            pm = QPixmap(); pm.loadFromData(data)
            if not pm.isNull():
                cb(pm)
        self.bg(fetch_image, url, cb=done)

    def ip(self):
        it = self.list.currentItem()
        return it.data(Qt.ItemDataRole.UserRole)["ip"] if it else None

    def login(self):
        if LoginDialog(self).exec():
            self.cloud_sync()

    def cloud_sync(self):
        try:
            acct = json.load(open(CLOUD))
        except (OSError, ValueError):
            self.account.setText("Sign in to Govee cloud")
            return
        self.account.setText(f"Account: {acct.get('email') or acct.get('accountId') or 'signed in'}")
        self.account.setIconSize(QSize(24, 24))
        self.image(acct.get("headPortrait"), lambda pm: self.account.setIcon(QIcon(pm)))
        self.bg(cli, "devices", cb=self.cloud_devices)

    def cloud_devices(self, r):
        if r.returncode:
            err = r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "cloud device list failed"
            self.say(err)
            if "401" in err:
                self.account.setText("Session expired: sign in again")
            return
        for line in r.stdout.splitlines():
            dev, sku, name, topic, img = (line.split("\t") + [""] * 5)[:5]
            self.cloud[dev] = {"sku": sku, "name": name, "topic": topic, "img": img}
        for i in range(self.list.count()):
            self.decorate(self.list.item(i))
        self.select(self.list.currentItem())

    def scan(self):
        self.say("Scanning...")
        self.bg(cli, "scan", cb=self.scanned)

    def scanned(self, r):
        found = [line.split("\t") for line in r.stdout.splitlines()]
        known = {self.list.item(i).data(Qt.ItemDataRole.UserRole)["ip"] for i in range(self.list.count())}
        for ip, sku, dev_id in found:
            if ip not in known:
                self.add_item(ip, sku, dev_id)
        for ip, d in self.cfg["devices"].items():  # remembered lights that didn't answer this time
            if ip not in known and ip not in {f[0] for f in found}:
                self.add_item(ip, d.get("sku", "?"), d.get("id", "?"))
        if self.list.count() and not self.list.currentItem():
            self.list.setCurrentRow(0)
        self.say(f"{len(found)} light(s) answered" + (f"  ({r.stderr.strip()})" if r.stderr.strip() else ""))

    def add_item(self, ip, sku, dev_id):
        it = QListWidgetItem(f"{sku}\n{ip}")
        it.setData(Qt.ItemDataRole.UserRole, {"ip": ip, "sku": sku, "id": dev_id})
        it.setToolTip(dev_id)
        self.list.addItem(it)
        self.decorate(it)
        m = DeviceMap(self, ip, sku, self.cfg["devices"].get(ip, {}))
        self.maps[ip] = m
        self.map_box.insertWidget(self.map_box.count() - 1, m)

    def decorate(self, it):
        d = it.data(Qt.ItemDataRole.UserRole)
        if c := self.cloud.get(d["id"]):
            it.setText(f"{c['name']} ({d['sku']})\n{d['ip']}")
            self.image(c["img"], lambda pm, it=it: it.setIcon(QIcon(pm)))

    def add_device(self):
        ip = self.add_ip.text().strip()
        if ip and ip not in self.maps:
            self.add_item(ip, "?", "?")
            self.add_ip.clear()

    def light_tab(self):
        w = QWidget(); v = QVBoxLayout(w); v.setSpacing(12)
        top = QHBoxLayout()
        self.power = QPushButton("Power"); self.power.setCheckable(True); self.power.setMinimumHeight(40)
        self.power.clicked.connect(lambda on: self.send("on" if on else "off"))
        top.addWidget(self.power, 1)
        self.swatch = QLabel(); self.swatch.setFixedSize(40, 40); self.swatch.setStyleSheet("border-radius:20px; background:#444;")
        top.addWidget(self.swatch)
        self.state = QLabel("—"); self.state.setMinimumWidth(180)
        top.addWidget(self.state)
        v.addLayout(top)

        g = QGridLayout(); g.setColumnStretch(1, 1)
        g.addWidget(QLabel("Brightness"), 0, 0)
        self.bri = QSlider(Qt.Orientation.Horizontal); self.bri.setRange(1, 100); self.bri.setValue(100)
        self.bri_lbl = QLabel("100%"); self.bri_lbl.setFixedWidth(50)
        self.bri.valueChanged.connect(lambda x: (self.bri_lbl.setText(f"{x}%"), self.slide("bri", x)))
        g.addWidget(self.bri, 0, 1); g.addWidget(self.bri_lbl, 0, 2)
        g.addWidget(QLabel("Warmth"), 1, 0)
        self.temp = QSlider(Qt.Orientation.Horizontal); self.temp.setRange(20, 90); self.temp.setValue(40)
        self.temp_lbl = QLabel("4000K"); self.temp_lbl.setFixedWidth(50)
        self.temp.valueChanged.connect(lambda x: (self.temp_lbl.setText(f"{x * 100}K"), self.slide("temp", x * 100)))
        g.addWidget(self.temp, 1, 1); g.addWidget(self.temp_lbl, 1, 2)
        v.addLayout(g)

        v.addWidget(QLabel("<b>Colour</b>"))
        row = QHBoxLayout()
        for hx in PRESETS:
            b = QPushButton(); b.setFixedSize(36, 36)
            b.setStyleSheet(f"background:{hx}; border-radius:18px;")
            b.clicked.connect(lambda _, hx=hx: self.set_color(QColor(hx)))
            row.addWidget(b)
        pick = QPushButton("Custom..."); pick.clicked.connect(self.pick_color)
        row.addWidget(pick); row.addStretch()
        v.addLayout(row)
        v.addStretch()
        self.slide_timer = QTimer(singleShot=True, interval=80)
        self.slide_timer.timeout.connect(lambda: self.send(*self.pending))
        self.pending = None
        return w

    def select(self, item):
        if item:
            self.refresh()
            self.load_scenes()

    def refresh(self):
        if ip := self.ip():
            self.bg(cli, "status", ip, cb=lambda r, ip=ip: self.show_state(ip, r))

    def show_state(self, ip, r):
        if ip != self.ip():
            return
        try:
            d = json.loads(r.stdout)
        except ValueError:
            self.state.setText("no reply (LAN Control off / offline?)")
            self.lan_ok[ip] = False
            return
        self.lan_ok[ip] = True
        c = d.get("color", {})
        k, on = d.get("colorTemInKelvin", 0), bool(d.get("onOff"))
        self.power.setChecked(on); self.power.setText("On" if on else "Off")
        for s in (self.bri, self.temp):
            s.blockSignals(True)
        self.bri.setValue(int(d.get("brightness", 100)))
        self.bri_lbl.setText(f"{self.bri.value()}%")
        if k:
            self.temp.setValue(int(k) // 100); self.temp_lbl.setText(f"{k}K")
        for s in (self.bri, self.temp):
            s.blockSignals(False)
        hx = "#%02x%02x%02x" % (c.get("r", 0), c.get("g", 0), c.get("b", 0))
        self.swatch.setStyleSheet(f"border-radius:20px; background:{'#f5e6c8' if k else hx};")
        self.state.setText(f"{'On' if on else 'Off'} · {d.get('brightness')}% · " + (f"{k}K white" if k else hx))

    def send(self, cmd, *args):
        if ip := self.ip():
            r = cli(cmd, ip, *args)
            if r.returncode:
                self.say(r.stderr.strip())
            elif cmd in ("on", "off"):
                self.power.setText("On" if cmd == "on" else "Off")
        if cmd == "bri" and self.dream:  # razer mode ignores the LAN brightness cmd; dreamview scales its own frames
            self.dream.stdin.write(f"{args[0]}\n".encode()); self.dream.stdin.flush()

    def slide(self, cmd, value):
        self.pending = (cmd, value)
        self.slide_timer.start()

    def set_color(self, c):
        self.send("color", c.red(), c.green(), c.blue())
        self.swatch.setStyleSheet(f"border-radius:20px; background:{c.name()};")

    def pick_color(self):
        c = QColorDialog.getColor(parent=self, title="Light colour")
        if c.isValid():
            self.set_color(c)

    # ---- scenes tab ----
    def scene_tab(self):
        w = QWidget(); v = QVBoxLayout(w)
        self.scene_filter = QLineEdit(); self.scene_filter.setPlaceholderText("filter")
        self.scene_filter.textChanged.connect(self.filter_scenes)
        v.addWidget(self.scene_filter)
        self.scene_list = QListWidget(); self.scene_list.setIconSize(QSize(36, 36))
        self.scene_list.itemDoubleClicked.connect(lambda _: self.apply_scene())
        v.addWidget(self.scene_list, 1)
        row = QHBoxLayout()
        self.scene_hint = QLabel("")
        row.addWidget(self.scene_hint, 1)
        b = QPushButton("Apply"); b.clicked.connect(self.apply_scene); row.addWidget(b)
        v.addLayout(row)
        self.scenes = {}  # device id -> list of (id, name, category)
        self.scene_gen = 0  # bumped on every clear(): icon callbacks for older fills must not touch freed items
        self.lan_ok = {}  # ip -> answered last status poll
        return w

    def load_scenes(self):
        it = self.list.currentItem(); d = it.data(Qt.ItemDataRole.UserRole)
        self.scene_list.clear(); self.scene_gen += 1
        if d["id"] not in self.cloud:
            self.scene_hint.setText("scenes need the cloud account (sign in) and a device it knows")
            return
        if d["id"] in self.scenes:
            self.fill_scenes(d["id"])
            return
        self.scene_hint.setText("loading scenes...")
        self.bg(cli, "scenes", d["id"], d["sku"], cb=lambda r, dev=d["id"]: self.got_scenes(dev, r))

    def got_scenes(self, dev, r):
        if r.returncode:
            self.scene_hint.setText(r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "scene list failed")
            return
        self.scenes[dev] = [tuple((line.split("\t") + [""] * 4)[:4]) for line in r.stdout.splitlines()]
        it = self.list.currentItem()
        if it and it.data(Qt.ItemDataRole.UserRole)["id"] == dev:
            self.fill_scenes(dev)

    def fill_scenes(self, dev):
        self.scene_list.clear(); self.scene_gen += 1
        gen = self.scene_gen
        for sid, name, cat, icon in self.scenes[dev]:
            it = QListWidgetItem(f"{cat} · {name}" if cat else name)
            it.setData(Qt.ItemDataRole.UserRole, sid)
            self.scene_list.addItem(it)
            self.image(icon, lambda pm, it=it: gen == self.scene_gen and it.setIcon(QIcon(pm)))
        self.filter_scenes(self.scene_filter.text())
        self.scene_hint.setText(f"{len(self.scenes[dev])} scenes; double-click to apply")

    def filter_scenes(self, text):
        t = text.lower()
        for i in range(self.scene_list.count()):
            it = self.scene_list.item(i)
            it.setHidden(t not in it.text().lower())

    def apply_scene(self):
        it, sc = self.list.currentItem(), self.scene_list.currentItem()
        if not it or not sc:
            return
        d = it.data(Qt.ItemDataRole.UserRole)
        via = d["ip"] if self.lan_ok.get(d["ip"], True) else "-"  # LAN first, cloud only when the light didn't answer
        self.scene_hint.setText(f"applying {sc.text()} via {'LAN' if via != '-' else 'cloud'}...")
        self.bg(cli, "scene", via, d["id"], d["sku"], sc.data(Qt.ItemDataRole.UserRole),
                cb=lambda r, n=sc.text(): self.scene_hint.setText(n if not r.returncode else r.stderr.strip().splitlines()[-1]))

    # ---- dreamview tab ----
    def dream_tab(self):
        w = QWidget(); v = QVBoxLayout(w)
        top = QHBoxLayout()
        self.zones = ScreenZones()
        top.addWidget(self.zones)
        opts = QVBoxLayout()
        opts.addWidget(QLabel("Click a zone, then click the segments of a light that should show it.\n"
                              "Right-click a segment for the whole-screen average."))
        self.live = QCheckBox("Preview zone colours on the lights"); self.live.setChecked(True)
        opts.addWidget(self.live)
        f = QGridLayout()
        f.addWidget(QLabel("Saturation"), 0, 0)
        self.sat = QDoubleSpinBox(); self.sat.setRange(1.0, 4.0); self.sat.setSingleStep(0.1); self.sat.setValue(self.cfg.get("sat", 1.6))
        f.addWidget(self.sat, 0, 1)
        f.addWidget(QLabel("Updates/s"), 1, 0)
        self.fps = QSpinBox(); self.fps.setRange(1, 30); self.fps.setValue(self.cfg.get("fps", 15))
        f.addWidget(self.fps, 1, 1)
        f.setColumnStretch(2, 1)
        opts.addLayout(f)
        opts.addStretch()
        self.dream_btn = QPushButton("Start DreamView"); self.dream_btn.setMinimumHeight(40)
        self.dream_btn.clicked.connect(self.toggle_dream)
        opts.addWidget(self.dream_btn)
        top.addLayout(opts, 1)
        v.addLayout(top)
        scroll = QScrollArea(); scroll.setWidgetResizable(True); scroll.setFrameShape(QFrame.Shape.NoFrame)
        inner = QWidget(); self.map_box = QVBoxLayout(inner); self.map_box.addStretch()
        scroll.setWidget(inner)
        v.addWidget(scroll, 1)
        return w

    def toggle_dream(self):
        if self.dream:
            self.dream.send_signal(signal.SIGINT)  # cli leaves razer mode on SIGINT
            self.dream.wait()
            self.dream = None
            self.dream_btn.setText("Start DreamView")
            self.say("DreamView stopped")
            return
        targets = [m.target() for m in self.maps.values() if m.on.isChecked()]
        if not targets:
            self.say("Tick at least one light in DreamView")
            return
        self.dream = subprocess.Popen([GOVEE, "dreamview", *targets, "--sat", str(self.sat.value()), "--fps", str(self.fps.value()),
                                       "--bri", str(self.bri.value())], stdin=subprocess.PIPE)
        self.dream_btn.setText("Stop DreamView")
        self.say("DreamView running")
        self.save()

    def save(self):
        for i in range(self.list.count()):
            d = self.list.item(i).data(Qt.ItemDataRole.UserRole)
            self.cfg["devices"][d["ip"]] = {"sku": d["sku"], "id": d["id"], **self.maps[d["ip"]].config()}
        self.cfg["sat"], self.cfg["fps"] = self.sat.value(), self.fps.value()
        os.makedirs(os.path.dirname(CONFIG), exist_ok=True)
        json.dump(self.cfg, open(CONFIG, "w"), indent=1)

    def closeEvent(self, e):
        if self.dream:
            self.toggle_dream()
        self.save()
        e.accept()


if __name__ == "__main__":
    app = QApplication(sys.argv)
    app.setApplicationName("Govee")
    win = Main()
    win.resize(900, 640)
    win.show()
    sys.exit(app.exec())

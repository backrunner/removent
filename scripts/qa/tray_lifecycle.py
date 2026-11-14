"""Shared native tray acceptance against real daemon IPC and QUIC video sessions."""
import argparse
import json
import os
from pathlib import Path
import queue
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback

import psutil
from PIL import ImageGrab


def wait_for(check, message, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError(message)


def visible_app(ui, window, name):
    """A mapped/focused window can still have an unpainted or lost surface."""
    def rendered():
        shot = ImageGrab.grab().convert("RGB")
        left, top, right, bottom = ui.app_rect(window)
        width, height = right - left, bottom - top
        # Exclude the native frame and title bar, including resize artifacts.
        content = shot.crop((left + width // 8, top + height // 4,
                             right - width // 8, bottom - height // 8))
        colors = content.getcolors(content.width * content.height)
        low, high = content.convert("L").getextrema()
        shot.save(ui.args.evidence_dir / f"{name}.png")
        return colors and len(colors) > 30 and high - low > 50
    wait_for(rendered, f"Desktop content was not visibly rendered: {name}", 25)


class Stream:
    def __init__(self, command, env, log):
        self.events = queue.Queue()
        self.process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=log, text=True,
                                        encoding="utf-8", errors="replace", bufsize=1)
        def read():
            for line in self.process.stdout:
                log.write(f"probe {self.process.pid}: {line}")
                log.flush()
                try:
                    self.events.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
        threading.Thread(target=read, daemon=True).start()

    def event(self, predicate, seconds=15):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                event = self.events.get(timeout=0.2)
                if predicate(event):
                    return event
            except queue.Empty:
                if self.process.poll() is not None:
                    raise AssertionError(f"Probe exited early: {self.process.returncode}")
        raise AssertionError("Expected probe event was not received")

    def send(self, command):
        self.process.stdin.write(command + "\n")
        self.process.stdin.flush()

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=5)


def run(desktop_type):
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--probe-dir", type=Path, default=Path("target/release/examples"))
    parser.add_argument("--evidence-dir", type=Path, default=Path("target/qa-evidence/tray"))
    parser.add_argument("--soak-seconds", type=int, default=60)
    parser.add_argument("--locale", choices=["En", "ZhCn"], default="En")
    parser.add_argument("--theme", choices=["light", "dark"], default="light")
    parser.add_argument("--scale", type=int, choices=[1, 2], default=1)
    parser.add_argument("--presentation-only", action="store_true")
    parser.add_argument("--second-monitor", action="store_true")
    args = parser.parse_args()
    if args.second_monitor and os.name == "nt":
        parser.error("--second-monitor requires the isolated Linux RandR fixture")
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if os.name == "nt" else ""
    binary = args.bin_dir.resolve() / ("removent-tray" + suffix)
    probe = args.probe_dir.resolve() / ("tray_probe" + suffix)
    session_binary = args.probe_dir.resolve() / ("tray_session" + suffix)
    zh = args.locale == "ZhCn"
    labels = {
        "open": "打开 Removent" if zh else "Open Removent",
        "start": "启动后台服务" if zh else "Start background service",
        "stop": "停止后台服务" if zh else "Stop background service",
        "share": "共享本机" if zh else "Share this desktop",
        "pair": "显示配对码" if zh else "Show pairing code",
        "quit": "退出托盘" if zh else "Quit tray",
        "pair_title": "配对码" if zh else "Pairing code",
        "admit": "远程访问请求" if zh else "Remote access request",
    }
    with tempfile.TemporaryDirectory(prefix="removent-tray-full-") as directory:
        root = Path(directory)
        data = root / "data"
        data.mkdir()
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reserve:
            reserve.bind(("127.0.0.1", 0))
            port = reserve.getsockname()[1]
        (data / "settings.toml").write_text(
            f'language = "{args.locale}"\ntheme = "System"\nhost_enabled = false\n'
            f'host_port = {port}\nprevent_idle_sleep = false\n'
            '[authentication]\nmode = "pairing_code"\n', encoding="utf-8")
        env = dict(os.environ, REMOVENT_DATA_DIR=str(data), REMOVENT_DESKTOP_TEST="1",
                   REMOVENT_TEST_CLIENT_DIR=str(root / "client"), REMOVENT_NO_TRAY="1")
        results, resources, streams = [], [], []
        completed = False
        tray = None
        stimulus = None
        daemon_pid = None
        with (args.evidence_dir / "tray.log").open("w", encoding="utf-8") as log:
            def passed(name):
                results.append(name)
                print("PASS: " + name, flush=True)
            def ipc(request):
                result = subprocess.run([str(probe), json.dumps(request)], env=env,
                                        capture_output=True, text=True, encoding="utf-8", timeout=5)
                if result.returncode:
                    return None
                return json.loads(result.stdout.strip().splitlines()[-1])
            def health():
                value = ipc({"type": "update_health"})
                return value if value and value.get("type") == "update_health" else None
            def start_tray(ui):
                process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
                ui.attach(process.pid)
                wait_for(ui.registered, "Tray icon was not registered")
                return process
            def connect(watcher):
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    stream = Stream([str(session_binary), str(port)], env, log)
                    streams.append(stream)
                    while time.monotonic() < deadline:
                        try:
                            event = watcher.events.get(timeout=0.1)
                            if event.get("type") == "admission_request":
                                return stream, event
                        except queue.Empty:
                            pass
                        try:
                            event = stream.events.get_nowait()
                        except queue.Empty:
                            continue
                        # A rejection reaches the client just before the host's
                        # connection teardown releases its single-session slot.
                        # Wait for that boundary, without retrying any denial or
                        # masking a host that never releases the slot.
                        assert event == {"event": "rejected", "reason": "rejected: Busy"}, event
                        assert stream.process.wait(timeout=5) == 0
                        log.write("Waiting for the previous host admission to finish teardown\n")
                        log.flush()
                        time.sleep(0.1)
                        break
                raise AssertionError("Host did not admit a new request within 10 seconds")
            try:
                subprocess.run([str(session_binary), "prepare"], env=env, check=True,
                               stdout=log, stderr=log, timeout=15)
                with desktop_type(root, env, args, log) as ui:
                    tray = start_tray(ui)
                    duplicate = subprocess.run([str(binary)], env=env, stdout=log, stderr=log, timeout=5)
                    assert duplicate.returncode == 0 and tray.poll() is None
                    passed("tray singleton")
                    ui.click(labels["start"])
                    state = wait_for(health, "Daemon did not start")
                    daemon_pid = state["pid"]
                    ui.click(labels["share"])
                    wait_for(lambda: (health() or {}).get("status", {}).get("host_ready"), "Sharing listener did not become ready")
                    for index in range(2):
                        ui.click(labels["pair"])
                        wait_for(lambda: ui.dialog(labels["pair_title"]), "Pairing dialog did not open")
                        ui.snapshot(f"pairing-{index}")
                        ui.answer(labels["pair_title"], None)
                        wait_for(lambda: not ui.dialog(labels["pair_title"]), "Pairing dialog did not close")
                    assert ipc({"type": "pairing_revoke"})["type"] == "ok"
                    passed("real icon clicks, sharing and repeatable native pairing dialogs")
                    ui.click(labels["open"])
                    window = wait_for(ui.app_window, "Tray failed to open the real desktop", 30)
                    visible_app(ui, window, "desktop-open")
                    if args.second_monitor:
                        position = ui.move_to_secondary(window)
                        visible_app(ui, window, "desktop-secondary")
                    ui.minimize(window)
                    ui.click(labels["open"])
                    wait_for(lambda: ui.app_active(window), "Existing desktop was not restored and activated")
                    assert ui.app_count() == 1, "Tray created duplicate desktop windows"
                    if args.second_monitor:
                        wait_for(lambda: ui.app_position(window) == position,
                                 f"Reactivation did not restore the desktop position {position}", 5)
                        visible_app(ui, window, "desktop-secondary-restored")
                        passed("reactivation preserves the desktop position on a second logical monitor")
                    else:
                        visible_app(ui, window, "desktop-restored")
                    passed("open and reactivate a single real desktop window")
                    if args.presentation_only:
                        ui.close_app(window)
                        wait_for(lambda: not ui.app_window(), "Desktop did not close")
                    else:
                        watcher = Stream([str(probe), "watch"], env, log)
                        streams.append(watcher)
                        watcher.event(lambda e: e.get("type") == "status")
                        pending, request = connect(watcher)
                        time.sleep(1)
                        assert not ui.dialog(labels["admit"]), "Tray duplicated the desktop's prompt immediately"
                        assert ipc({"type": "admission_reply", "request_id": request["request_id"], "allow": False})["type"] == "ok"
                        pending.event(lambda e: e.get("event") == "rejected")
                        time.sleep(8)
                        assert not ui.dialog(labels["admit"]), "Resolved request produced a delayed duplicate prompt"
                        ui.close_app(window)
                        wait_for(lambda: not ui.app_window(), "Desktop did not close")
                        passed("desktop-first admission and resolution by another management client")
                        pending, _ = connect(watcher)
                        wait_for(lambda: ui.dialog(labels["admit"]), "Default-answer prompt missing")
                        ui.accept_default(labels["admit"])
                        pending.event(lambda e: e.get("event") == "rejected")
                        wait_for(lambda: not ui.dialog(labels["admit"]), "Default-denied prompt remained visible")
                        passed("pressing Enter defaults to denying remote access")
                        pending, request = connect(watcher)
                        wait_for(lambda: ui.dialog(labels["admit"]), "Competing-answer prompt missing")
                        assert ipc({"type": "admission_reply", "request_id": request["request_id"], "allow": False})["type"] == "ok"
                        pending.event(lambda e: e.get("event") == "rejected")
                        wait_for(lambda: not ui.dialog(labels["admit"]), "Another client's answer did not dismiss the prompt")
                        passed("another management client's answer dismisses an already visible prompt")
                        for decision in (False, True):
                            pending, _ = connect(watcher)
                            wait_for(lambda: ui.dialog(labels["admit"]), "Admission dialog missing")
                            ui.snapshot("admission-allow" if decision else "admission-deny")
                            ui.answer(labels["admit"], decision)
                            pending.event(lambda e: e.get("event") == ("connected" if decision else "rejected"))
                            wait_for(lambda: not ui.dialog(labels["admit"]), "Answered prompt remained visible")
                            if decision:
                                pending.event(lambda e: e.get("event") == "frames" and e["count"] > 0)
                                # Capture intentionally suppresses unchanged frames. Animate
                                # an independent native window to prove continued delivery.
                                stimulus = subprocess.Popen([sys.executable, str(Path(__file__).with_name("tray_video_stimulus.py"))],
                                                            env=env, stdout=log, stderr=log)
                                ui.click(labels["quit"])
                                assert tray.wait(timeout=5) == 0
                                before = pending.event(lambda e: e.get("event") == "frames")["count"]
                                pending.event(lambda e: e.get("event") == "frames" and e["count"] > before + 2)
                                assert health()["pid"] == daemon_pid
                                passed("quitting the tray preserves the daemon and real QUIC video")
                                tray = start_tray(ui)
                                ui.restart_shell()
                                wait_for(ui.registered, "Tray icon did not recover after shell restart", 25)
                                ui.click(labels["pair"])
                                # Pairing is refused during an active session; the native error
                                # dialog also proves menu dispatch survived shell replacement.
                                wait_for(lambda: ui.dialog("Removent"), "Expected busy-session feedback")
                                ui.answer("Removent", None)
                                passed("shell restart preserves tray registration and menu actions")
                                baseline = None
                                soak_started = time.monotonic()
                                deadline = soak_started + args.soak_seconds
                                last_frames = before
                                while time.monotonic() < deadline:
                                    event = pending.event(lambda e: e.get("event") == "frames" and e["count"] > last_frames, seconds=10)
                                    last_frames = event["count"]
                                    process = psutil.Process(tray.pid)
                                    sample = {"rss": process.memory_info().rss, "threads": process.num_threads(),
                                              "handles": process.num_handles() if os.name == "nt" else process.num_fds(),
                                              "elapsed_seconds": time.monotonic() - soak_started,
                                              "decoded_frames": last_frames}
                                    baseline = baseline or sample
                                    assert sample["rss"] < baseline["rss"] + 64 * 1024 * 1024, sample
                                    assert sample["handles"] < baseline["handles"] + 64, sample
                                    assert sample["threads"] < baseline["threads"] + 8, sample
                                    resources.append(sample)
                                    assert tray.poll() is None and health()["pid"] == daemon_pid
                                pending.send("close")
                                pending.event(lambda e: e.get("event") == "closed")
                                assert pending.process.wait(timeout=5) == 0
                                stimulus.terminate()
                                stimulus.wait(timeout=5)
                                passed(f"{args.soak_seconds}s live video soak with tray memory, handle and thread bounds")
                            else:
                                assert pending.process.wait(timeout=5) == 0
                        passed("native denial and approval produce the corresponding real remote-session result")
                        pending, _ = connect(watcher)
                        wait_for(lambda: ui.dialog(labels["admit"]), "Cancellation prompt missing")
                        pending.send("cancel")
                        pending.event(lambda e: e.get("event") == "cancelled")
                        wait_for(lambda: not ui.dialog(labels["admit"]), "Remote cancellation did not close prompt", 5)
                        passed("controller cancellation dismisses the native approval dialog")
                        pending, _ = connect(watcher)
                        wait_for(lambda: ui.dialog(labels["admit"]), "Timeout prompt missing")
                        pending.event(lambda e: e.get("event") == "rejected", seconds=35)
                        wait_for(lambda: not ui.dialog(labels["admit"]), "Expired approval dialog remained visible")
                        passed("real 30-second admission timeout rejects and dismisses the prompt")
                        pending, _ = connect(watcher)
                        wait_for(lambda: ui.dialog(labels["admit"]), "Disconnect prompt missing")
                        psutil.Process(daemon_pid).kill()
                        wait_for(lambda: not ui.dialog(labels["admit"]), "Lost-daemon prompt remained visible")
                        state = wait_for(lambda: (h if (h := health()) and h["pid"] != daemon_pid else None), "Tray did not recover the crashed daemon", 20)
                        daemon_pid = state["pid"]
                        wait_for(lambda: health()["status"]["host_ready"], "Recovered host not ready")
                        passed("daemon crash clears stale prompts and restarts sharing")
                        pending.stop()
                    ui.click(labels["share"])
                    wait_for(lambda: not health()["status"]["running"], "Sharing did not stop")
                    ui.click(labels["stop"])
                    wait_for(lambda: not health(), "Daemon did not stop")
                    assert (data / "run/service-stopped").exists()
                    time.sleep(6)
                    assert not health(), "Watchdog overrode an explicit stop"
                    delayed = subprocess.run([str(args.bin_dir.resolve() / ("removentd" + suffix)), "--background"],
                                             env=env, stdout=log, stderr=log, timeout=5)
                    assert delayed.returncode == 0 and not health(), "Delayed background launch overrode an explicit stop"
                    ui.click(labels["quit"])
                    assert tray.wait(timeout=5) == 0
                    tray = start_tray(ui)
                    time.sleep(2)
                    assert not health(), "Tray restart overrode an explicit stop"
                    ui.click(labels["start"])
                    daemon_pid = wait_for(health, "Explicit restart did not work")["pid"]
                    ui.click(labels["stop"])
                    wait_for(lambda: not health(), "Final service stop failed")
                    ui.click(labels["quit"])
                    assert tray.wait(timeout=5) == 0
                    passed("manual stop survives watchdog and tray restart; explicit start remains usable")
                    completed = True
            finally:
                if sys.exc_info()[0] is not None:
                    traceback.print_exc(file=log)
                    log.flush()
                if stimulus and stimulus.poll() is None:
                    stimulus.terminate()
                    stimulus.wait(timeout=5)
                if tray and tray.poll() is None:
                    tray.terminate()
                    tray.wait(timeout=5)
                for stream in streams:
                    stream.stop()
                try:
                    ipc({"type": "shutdown"})
                except (subprocess.TimeoutExpired, OSError):
                    pass
                if daemon_pid and psutil.pid_exists(daemon_pid):
                    try:
                        psutil.Process(daemon_pid).wait(timeout=5)
                    except psutil.TimeoutExpired:
                        psutil.Process(daemon_pid).kill()
                if (data / "logs").is_dir():
                    shutil.copytree(data / "logs", args.evidence_dir / "logs", dirs_exist_ok=True)
                (args.evidence_dir / "results.json").write_text(json.dumps({"completed": completed, "passed": results, "resources": resources,
                    "locale": args.locale, "theme": args.theme, "scale": args.scale,
                    "second_monitor": args.second_monitor}, indent=2), encoding="utf-8")

#!/usr/bin/env python3
"""Real pseudo-terminal checks for the inline CLI (no installed state modified).

Run after cargo build -p removent-cli:
    python3 scripts/qa/test_cli_tui.py [path/to/removent-cli]
"""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import sys
import tempfile
import termios
import threading
import time
import unittest

BINARY = str(Path(sys.argv.pop(1) if len(sys.argv) > 1 else "target/debug/removent-cli").resolve())


class Terminal:
    def __init__(self, root, args=(), columns=80, extra_env=None):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 24, columns, 0, 0))
            env = dict(os.environ, REMOVENT_DATA_DIR=str(root), TERM="xterm-256color", NO_COLOR="1")
            env.pop("RUST_LOG", None)
            for key, value in (extra_env or {}).items():
                if value is None:
                    env.pop(key, None)
                else:
                    env[key] = value
            os.execve(BINARY, [BINARY, *args], env)
        self.output = b""
        self.status = None

    def read(self, timeout=.05):
        if select.select([self.fd], [], [], timeout)[0]:
            try:
                data = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                data = b""
            self.output += data
            return bool(data)
        return True

    def expect(self, text, timeout=5):
        deadline = time.monotonic() + timeout
        while text.encode() not in self.output:
            if time.monotonic() >= deadline or not self.read():
                raise AssertionError(f"Missing {text!r}: {self.output[-5000:].decode(errors='replace')}")

    def send(self, text):
        os.write(self.fd, text.encode())

    def finish(self, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.read()
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)
                while self.read(.01):
                    if not select.select([self.fd], [], [], .01)[0]:
                        break
                return self.status
        raise AssertionError(f"CLI did not exit: {self.output[-5000:].decode(errors='replace')}")

    def close(self):
        if self.status is None:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


class InlineCliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="removent-tui-", dir="/tmp")
        self.root = Path(self.temp.name)
        (self.root / "settings.toml").write_text('language = "En"\n')
        self.terminals = []

    def tearDown(self):
        for terminal in self.terminals:
            terminal.close()
        self.temp.cleanup()

    def terminal(self, *args, **kwargs):
        terminal = Terminal(self.root, args, **kwargs)
        self.terminals.append(terminal)
        return terminal

    def test_menu_keeps_scrollback_and_restores_cursor_on_cancel(self):
        for cancel, expected in [("q", 0), *[("\x03", 130)] * 5]:
            terminal = self.terminal()
            terminal.expect("Choose an action")
            terminal.send(cancel)
            self.assertEqual(terminal.finish(), expected)
            flags = termios.tcgetattr(terminal.fd)[3]
            self.assertTrue(flags & termios.ECHO)
            self.assertTrue(flags & termios.ICANON)
            self.assertIn(b"\x1b[?25h", terminal.output)
            self.assertNotIn(b"\x1b[?1049", terminal.output)
            self.assertNotIn(b"\x1b[2J", terminal.output)
            self.assertNotIn(b"local diagnostics initialized", terminal.output)

    def test_settings_edit_uses_real_prompt_and_persists_after_confirmation(self):
        terminal = self.terminal("interactive")
        terminal.expect("Choose an action")
        terminal.send("\x1b[B" * 3 + "\r")
        terminal.expect("device_name:")
        terminal.send("\r")
        terminal.expect("Edit value")
        terminal.send("\r")
        terminal.expect("New value")
        terminal.send("\x15Office TUI\r")
        terminal.expect("Save this change?")
        terminal.send("y\r")
        terminal.expect("Settings saved")
        terminal.expect("Office TUI")
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertIn('device_name = "Office TUI"', (self.root / "settings.toml").read_text())

    def test_secret_entry_is_hidden_and_queries_remain_redacted(self):
        terminal = self.terminal("interactive")
        terminal.expect("Choose an action")
        terminal.send("\x1b[B" * 3 + "\r")
        terminal.expect("device_name:")
        terminal.send("\x1b[B" * 9 + "\r")
        terminal.expect("Edit value")
        terminal.send("\r")
        terminal.expect("New secret")
        secret = "TuiSecret-7x!"
        terminal.send(secret + "\r")
        terminal.expect("Repeat secret")
        terminal.send(secret + "\r")
        terminal.expect("Save this change?")
        terminal.send("y\r")
        terminal.expect("Settings saved")
        terminal.expect("[configured, hidden]")
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertNotIn(secret.encode(), terminal.output)
        log = (self.root / "logs/removent-cli.log").read_text()
        self.assertNotIn(secret, log)

    def test_cancelling_hidden_input_restores_terminal(self):
        terminal = self.terminal("interactive")
        terminal.expect("Choose an action")
        terminal.send("\x1b[B" * 3 + "\r")
        terminal.expect("device_name:")
        terminal.send("\x1b[B" * 9 + "\r")
        terminal.expect("Edit value")
        terminal.send("\r")
        terminal.expect("New secret")
        terminal.send("partial-password\x03")
        self.assertEqual(terminal.finish(), 130)
        flags = termios.tcgetattr(terminal.fd)[3]
        self.assertTrue(flags & termios.ECHO)
        self.assertTrue(flags & termios.ICANON)
        self.assertNotIn(b"partial-password", terminal.output)
        self.assertNotIn("authentication", (self.root / "settings.toml").read_text())

    def test_declining_reset_does_not_write_settings(self):
        original = (self.root / "settings.toml").read_bytes()
        terminal = self.terminal("interactive")
        terminal.expect("Choose an action")
        terminal.send("\x1b[B" * 3 + "\r")
        terminal.expect("device_name:")
        terminal.send("\x1b[B" * 2 + "\r")
        terminal.expect("Reset to default")
        terminal.send("\x1b[B\r")
        terminal.expect("Save this change?")
        terminal.send("n\r")
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertEqual((self.root / "settings.toml").read_bytes(), original)

    def test_unavailable_service_returns_to_menu(self):
        terminal = self.terminal()
        terminal.expect("Choose an action")
        terminal.send("\r")
        terminal.expect("background service is currently unavailable")
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertIn(b"daemon start", terminal.output)

    def test_search_keeps_action_identity_and_escape_clears_filter(self):
        terminal = self.terminal()
        terminal.expect("Choose an action")
        terminal.send("settings")
        terminal.expect("/ settings")
        terminal.send("\r")
        terminal.expect("device_name:")
        terminal.send("authentication.password")
        terminal.expect("1/1")
        terminal.send("\r")
        terminal.expect("Edit value")
        terminal.send("q")
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertNotIn("authentication", (self.root / "settings.toml").read_text())

    def test_invalid_numeric_value_stays_in_form_and_bracketed_paste_does_not_submit(self):
        terminal = self.terminal()
        terminal.expect("Choose an action")
        terminal.send("settings\r")
        terminal.expect("device_name:")
        terminal.send("host_port\r")
        terminal.expect("Edit value")
        terminal.send("\r")
        terminal.expect("New value")
        terminal.send("\x15\x1b[200~0\n\x1b[201~")
        # Enter is explicit: the pasted newline cannot submit a form.
        terminal.send("\r")
        terminal.expect("Invalid value for host_port")
        self.assertNotIn("Review changes", terminal.output.decode())
        terminal.send("\x1548690\r")
        terminal.expect("Review changes")
        terminal.expect("Save this change?")
        terminal.send("n\rq")
        self.assertEqual(terminal.finish(), 0)
        self.assertNotIn("host_port", (self.root / "settings.toml").read_text())

    def test_resize_and_narrow_menu_keep_history_and_restore_input(self):
        terminal = self.terminal(columns=40)
        terminal.expect("Choose an action")
        fcntl.ioctl(terminal.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 8, 20, 0, 0))
        terminal.send("\x1b[B")
        terminal.read(.1)
        fcntl.ioctl(terminal.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        terminal.send("\x1b[B")
        terminal.read(.1)
        terminal.send("q")
        self.assertEqual(terminal.finish(), 0)
        self.assertNotIn(b"\x1b[2J", terminal.output)
        self.assertNotIn(b"\x1b[?1049", terminal.output)
        self.assertTrue(termios.tcgetattr(terminal.fd)[3] & termios.ECHO)

    def test_dumb_terminal_shows_help_and_json_stays_json_on_tty(self):
        terminal = self.terminal(extra_env={"TERM": "dumb"})
        self.assertEqual(terminal.finish(), 0)
        self.assertIn(b"Usage:", terminal.output)
        terminal = self.terminal("daemon", "service-status", "--json")
        self.assertEqual(terminal.finish(), 0)
        result = json.loads(terminal.output)
        self.assertFalse(result["reachable"])
        self.assertNotIn(b"\x1b", terminal.output)

    def test_progress_is_transient_and_json_remains_clean(self):
        run = self.root / "run"
        run.mkdir()
        for json_mode in [False, True]:
            address = run / "removentd.sock"
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(str(address))
                listener.listen()
                def serve():
                    connection, _ = listener.accept()
                    with connection:
                        connection.recv(4096)
                        time.sleep(.35)
                        connection.sendall(b'{"type":"status","running":false,"host_ready":false,"sessions":[]}\n')
                args = ["daemon", "status", "--json"] if json_mode else ["daemon", "status"]
                terminal = self.terminal(*args)
                server = threading.Thread(target=serve, daemon=True)
                server.start()
                self.assertEqual(terminal.finish(), 0)
                server.join(2)
                if json_mode:
                    self.assertFalse(json.loads(terminal.output)["running"])
                    self.assertNotIn(b"\x1b", terminal.output)
                else:
                    self.assertIn(b"Processing request", terminal.output)
                    self.assertIn(b"\x1b[2K", terminal.output)
                    self.assertIn("╭".encode(), terminal.output)
            address.unlink()

    def test_narrow_chinese_pairing_is_human_and_no_color(self):
        (self.root / "settings.toml").write_text('language = "ZhCn"\n')
        run = self.root / "run"
        run.mkdir()
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(run / "removentd.sock"))
            listener.listen()
            def serve():
                connection, _ = listener.accept()
                with connection:
                    connection.recv(4096)
                    connection.sendall(b'{"type":"pairing_code","code":"123456","expires_at_unix":null}\n')
            terminal = self.terminal("pairing", "show", columns=32)
            server = threading.Thread(target=serve, daemon=True)
            server.start()
            self.assertEqual(terminal.finish(), 0)
            server.join(2)
        text = terminal.output.decode()
        self.assertIn("配对码", text)
        self.assertIn("123 456", text)
        self.assertNotIn("expires_at_unix", text)
        self.assertNotIn("\x1b", text)
        for line in text.splitlines():
            self.assertFalse(line.lstrip().startswith(tuple("，。、：；？！）】》」』”’")), line)


if __name__ == "__main__":
    unittest.main(verbosity=2)

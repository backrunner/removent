import importlib.util
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import tempfile
import threading
import unittest
import urllib.error
import urllib.request

spec = importlib.util.spec_from_file_location('relay_control', Path(__file__).with_name('relay_cloudflare.py'))
relay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(relay)


class RelayControlTests(unittest.TestCase):
    def test_credentials_cannot_travel_in_urls_or_insecure_requests(self):
        self.assertEqual(relay.endpoint('https://relay.example', 'stop'), 'https://relay.example/admin/stop')
        for base in ['http://relay.example', 'https://user:token@relay.example', 'https://relay.example?token=x', 'https://relay.example/path']:
            with self.assertRaises(ValueError):
                relay.endpoint(base, 'start')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'token'
            path.write_text('11' * 32)
            path.chmod(0o600)
            self.assertEqual(relay.read_token(path), '11' * 32)
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                relay.read_token(path)
            link = Path(directory) / 'link'
            link.symlink_to(path)
            with self.assertRaises(OSError):
                relay.read_token(link)

    def test_redirect_does_not_forward_management_credential(self):
        requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                self.send_response(302)
                self.send_header('Location', '/steal')
                self.end_headers()

            def log_message(self, *_args):
                pass

        with HTTPServer(('127.0.0.1', 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                request = urllib.request.Request(f'http://127.0.0.1:{server.server_port}/admin/status', headers={'Authorization': 'Bearer fixture'})
                with self.assertRaises(urllib.error.HTTPError) as failure:
                    urllib.request.build_opener(relay.NoRedirect).open(request, timeout=2)
                self.assertEqual(failure.exception.code, 302)
                failure.exception.close()
                self.assertEqual(requests, ['/admin/status'])
            finally:
                server.shutdown()
                thread.join()


if __name__ == '__main__':
    unittest.main()

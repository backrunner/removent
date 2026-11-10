#!/usr/bin/env python3
"""MCP stdio smoke test, using isolated local data and no remote machine."""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
import unittest

BINARY = str(Path(sys.argv.pop(1) if len(sys.argv) > 1 else 'target/debug/removent-mcp').resolve())

class Mcp(unittest.TestCase):
    def test_stdio_lifecycle_and_legacy_negotiation(self):
        for version in ['2025-06-18', '2025-11-25', '2026-07-28']:
            with self.subTest(version=version), tempfile.TemporaryDirectory(prefix='removent-mcp-') as directory:
                env = {**os.environ, 'REMOVENT_DATA_DIR': directory}
                process = subprocess.Popen([BINARY], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
                pending = b''
                def send(value):
                    process.stdin.write(json.dumps(value).encode() + b'\n')
                    process.stdin.flush()
                def receive(request_id):
                    nonlocal pending
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        if b'\n' not in pending:
                            ready, _, _ = select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))
                            self.assertTrue(ready, 'MCP response timeout')
                            chunk = os.read(process.stdout.fileno(), 1024 * 1024)
                            self.assertTrue(chunk, 'Unexpected stdout EOF')
                            pending += chunk
                            continue
                        line, pending = pending.split(b'\n', 1)
                        message = json.loads(line)
                        if message.get('id') == request_id:
                            return message
                    self.fail('MCP response timeout')
                def call(request_id, name, arguments):
                    send({'jsonrpc':'2.0','id':request_id,'method':'tools/call','params':{'name':name,'arguments':arguments}})
                    return receive(request_id)['result']
                try:
                    send({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':version,'capabilities':{},'clientInfo':{'name':'test','version':'1'}}})
                    # initialize-based stdio negotiates the SDK's latest legacy version.
                    expected = '2025-11-25' if version == '2026-07-28' else version
                    self.assertEqual(receive(1)['result']['protocolVersion'], expected)
                    send({'jsonrpc':'2.0','method':'notifications/initialized'})
                    send({'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}})
                    tools = receive(2)['result']['tools']
                    names = [tool['name'] for tool in tools]
                    self.assertIn('removent_connect', names)
                    self.assertIn('computer', names)
                    self.assertFalse(any('browser' in name for name in names))
                    hosts = call(3, 'removent_hosts', {})['structuredContent']
                    self.assertEqual(hosts['hosts'], [])
                    self.assertEqual(len(hosts['client_fingerprint']), 64)
                    pending_connection = call(4, 'removent_connect', {'saved_id':'missing'})['structuredContent']
                    session_id = pending_connection['session_id']
                    for request_id in range(5, 15):
                        status = call(request_id, 'removent_status', {'session_id':session_id})['structuredContent']
                        if status['status'] == 'failed':
                            break
                        time.sleep(.01)
                    self.assertEqual(status['status'], 'failed')
                    self.assertTrue(call(15, 'computer', {'session_id':'unknown','operation_id':'test','command':{'action':'state'}})['isError'])
                    invalid = call(16, 'computer', {'session_id':'unknown','operation_id':'test','command':{'action':'state','extra':True}})
                    self.assertTrue(invalid['isError'])
                    self.assertEqual(call(17, 'removent_disconnect', {'session_id':session_id})['structuredContent']['status'], 'disconnected')
                    process.stdin.close()
                    self.assertEqual(process.wait(timeout=5), 0)
                    self.assertNotIn(b'panicked', process.stderr.read())
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    process.stdout.close()
                    process.stderr.close()
                    if not process.stdin.closed:
                        process.stdin.close()

if __name__ == '__main__':
    unittest.main()

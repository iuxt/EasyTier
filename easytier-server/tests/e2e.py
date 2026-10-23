#!/usr/bin/env python3
"""Exercise the real Go/MySQL control plane and Rust client without TUN/root."""
import http.cookiejar
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
ADMIN = 'e2e-admin-secret-at-least-24-characters'
TOKEN = 'e2e-enroll-secret-at-least-24-characters'


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def listening(p):
    with socket.socket() as s:
        s.settimeout(.2)
        return s.connect_ex(('127.0.0.1', p)) == 0


def wait_for(label, fn, timeout=25):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = fn()
            if result:
                print('PASS:', label, flush=True)
                return result
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(.5)
    raise AssertionError('timeout: ' + label)


def terminate(process):
    if process and process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    dsn = os.environ.get('MYSQL_TEST_DSN')
    if not dsn:
        raise SystemExit('MYSQL_TEST_DSN must point to a dedicated test database')
    binary = ROOT / 'target/debug/easytier-core'
    if not binary.exists():
        raise SystemExit('build the Rust client first: cargo build -p easytier --bin easytier-core')
    processes = []
    with tempfile.TemporaryDirectory(prefix='easytier-e2e-') as directory:
        work = Path(directory)
        server_binary = work / 'server'
        subprocess.run(['go', 'build', '-o', str(server_binary), '.'], cwd=ROOT / 'easytier-server', check=True)
        server_port, a_port, b_port = port(), port(), port()
        base = f'http://127.0.0.1:{server_port}'
        env = dict(os.environ, MYSQL_DSN=dsn, ADMIN_PASSWORD=ADMIN, ENROLLMENT_TOKEN=TOKEN,
                   COOKIE_SECURE='false', LISTEN_ADDR=f'127.0.0.1:{server_port}')
        server_log = open(work / 'server.log', 'w+')
        server = None
        clients = []
        files = []
        ids = []
        jar = http.cookiejar.CookieJar()
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

        def api(path, body=None, method=None):
            request = urllib.request.Request(base + path,
                data=json.dumps(body).encode() if body is not None else None,
                headers={'Content-Type': 'application/json'}, method=method)
            with opener.open(request, timeout=3) as response:
                return json.load(response)

        def start_server():
            process = subprocess.Popen([str(server_binary)], env=env, stdout=server_log, stderr=server_log)
            processes.append(process)
            wait_for('management server and MySQL ready', lambda: api('/healthz'))
            api('/api/login', {'password': ADMIN})
            return process

        def start_client(name):
            output = open(work / (name + '.stdout'), 'a+')
            error = open(work / (name + '.stderr'), 'a+')
            files.extend([output, error])
            process = subprocess.Popen([str(binary), '-h', base, '-k', TOKEN, '--state-dir', str(work / name)],
                                       stdout=output, stderr=error)
            processes.append(process)
            clients.append((name, process))
            return process

        def device(device_id):
            return next(d for d in api('/api/admin/devices') if d['id'] == device_id)

        def update(device_id, status, settings):
            d = device(device_id)
            return api('/api/admin/devices/' + device_id,
                       {'status': status, 'settings': settings, 'revision': d['revision']}, 'PUT')

        def settings(address, peer=''):
            return {'hostname': 'e2e-node', 'auto_ip': False, 'ipv4': address,
                    'allow_inbound': True, 'enable_ipv6': True, 'direct_connection': True,
                    'udp_hole_punch': True, 'peer_id': peer, 'public_address': '127.0.0.1'}

        def prepare(device_id, port):
            subprocess.run(['go', 'run', './tests/cleanup.go', '--prepare', device_id, str(port)],
                           cwd=ROOT / 'easytier-server', env=dict(os.environ, MYSQL_TEST_DSN=dsn), check=True)

        try:
            server = start_server()
            a = start_client('a')
            b = start_client('b')
            def enrolled():
                identities = list((work / 'a').glob('*.json')) + list((work / 'b').glob('*.json'))
                if len(identities) != 2:
                    return False
                values = [json.loads(p.read_text())['id'] for p in identities]
                all_ids = {d['id'] for d in api('/api/admin/devices')}
                return values if set(values) <= all_ids else False
            ids = wait_for('both clients registered', enrolled)
            assert all(device(i)['status'] == 'pending' for i in ids)
            time.sleep(2)
            assert not listening(a_port) and not listening(b_port), 'pending client opened network listener'
            print('PASS: no listeners before approval', flush=True)
            prepare(ids[0], a_port)
            prepare(ids[1], b_port)
            a_config, b_config = settings('10.144.0.1/16'), settings('10.144.0.2/16', ids[0])
            assert all('config' not in device(i) and 'network_secret' not in json.dumps(device(i)) and 'listeners' not in json.dumps(device(i)) for i in ids)
            update(ids[0], 'approved', a_config)
            wait_for('approved client opens real listener', lambda: listening(a_port))
            assert not listening(b_port), 'unapproved client started'
            update(ids[1], 'approved', b_config)
            wait_for('second approved client starts', lambda: listening(b_port))
            wait_for('runtime revision reported', lambda: all(device(i)['runtime_status'] == 'running' and device(i)['applied_revision'] == device(i)['revision'] for i in ids))
            logs_path = '/api/admin/devices/' + ids[1] + '/logs'
            wait_for('actual peer connection logged centrally', lambda: any('new peer connection added' in e['message'] for e in api(logs_path)), timeout=35)
            all_logs = json.dumps(api(logs_path), ensure_ascii=False)
            assert TOKEN not in all_logs
            update(ids[0], 'approved', dict(a_config, hostname='updated-node'))
            wait_for('configuration revision updated', lambda: device(ids[0])['applied_revision'] == device(ids[0])['revision'])
            try:
                update(ids[1], 'approved', dict(b_config, ipv4='invalid'))
                raise AssertionError('invalid form settings accepted')
            except urllib.error.HTTPError as e:
                assert e.code == 400
            print('PASS: invalid form setting rejected before runtime changes', flush=True)
            update(ids[1], 'approved', dict(b_config, direct_connection=False, enable_ipv6=False, udp_hole_punch=False))
            wait_for('switch changes applied to running client', lambda: device(ids[1])['applied_revision'] == device(ids[1])['revision'])
            assert device(ids[1])['settings']['enable_ipv6'] is False
            update(ids[1], 'revoked', b_config)
            wait_for('revocation closes listener', lambda: not listening(b_port))
            terminate(b)
            b = start_client('b')
            time.sleep(6)
            assert device(ids[1])['status'] == 'revoked' and not listening(b_port)
            assert len(list((work / 'b').glob('*.json'))) == 1
            print('PASS: restart preserves identity and revocation', flush=True)
            update(ids[1], 'approved', b_config)
            wait_for('reapproval resumes network', lambda: listening(b_port))
            terminate(server)
            print('Checking 45-second offline authorization expiry…', flush=True)
            wait_for('offline lease closes both listeners', lambda: not listening(a_port) and not listening(b_port), timeout=55)
            server = start_server()
            wait_for('server restart recovers approved clients', lambda: listening(a_port) and listening(b_port))
            for name, _ in clients:
                error = (work / (name + '.stderr')).read_text()
                assert not error.strip(), f'client printed runtime logs: {error}'
                stdout = (work / (name + '.stdout')).read_text()
                assert 'INFO ' not in stdout and 'CORE:' not in stdout
            print('PASS: console contains only brief status messages', flush=True)
            print('All Go/MySQL/Rust end-to-end checks passed.', flush=True)
        except Exception:
            for file in work.glob('*.log'):
                print(file.name, file.read_text()[-6000:])
            for file in work.glob('*.stderr'):
                print(file.name, file.read_text()[-6000:])
            for file in work.glob('*.stdout'):
                print(file.name, file.read_text()[-6000:])
            raise
        finally:
            for process in reversed(processes):
                terminate(process)
            # Remove only this run's rows, preserving unrelated test data.
            if ids:
                subprocess.run(['go', 'run', './tests/cleanup.go', *ids], cwd=ROOT / 'easytier-server',
                               env=dict(os.environ, MYSQL_TEST_DSN=dsn), check=True)
            server_log.close()
            for file in files:
                file.close()

if __name__ == '__main__':
    main()

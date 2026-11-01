#!/usr/bin/env python3
"""Read-only hardware smoke tests and exporter cost measurement (stdlib only).

Run with --bin-dir target/release --vendor apple|nvidia. JSON goes to stdout.
Never changes GPU clocks, limits, CPU policies, or other running workloads.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time
import urllib.request


def capture(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f'{command}: {result.stderr}')
    return json.loads(result.stdout)


def exporter(binary, vendor, seconds):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([str(binary), '--system', '--' + vendor, '--interval', '1',
                                 '--bind', '127.0.0.1', '--port', str(port)], stdout=log, stderr=log)
        start = time.monotonic()
        samples, rss_kib, threads = [], [], []
        body = ''
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        try:
            while time.monotonic() - start < seconds:
                if proc.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode())
                try:
                    before = time.monotonic()
                    with opener.open(f'http://127.0.0.1:{port}/metrics', timeout=2) as response:
                        body = response.read().decode()
                    samples.append((time.monotonic() - before) * 1000)
                    with opener.open(f'http://127.0.0.1:{port}/healthz', timeout=2) as response:
                        assert response.read() == b'OK'
                    rss_kib.append(int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(proc.pid)])))
                    status = Path(f'/proc/{proc.pid}/status')
                    if status.exists():
                        threads.append(int(next(x.split()[1] for x in status.read_text().splitlines() if x.startswith('Threads:'))))
                except (OSError, ValueError):
                    pass  # Initial socket startup only; final assertions require successful scrapes.
                time.sleep(1)
            assert len(samples) >= 3, 'exporter failed to serve metrics'
            assert 'system_memory_total_bytes{' in body
            assert f'vendor="{vendor}"' in body
            assert 'gpu_power_usage_watts{' in body
            if vendor == 'apple':
                assert 'apple_unified_memory_total_bytes{' in body
                assert not any(x.startswith('gpu_memory_total_bytes{') and 'vendor="apple"' in x for x in body.splitlines())
            proc.send_signal(signal.SIGTERM)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                pid, status, usage = os.wait4(proc.pid, os.WNOHANG)
                if pid:
                    proc.returncode = os.waitstatus_to_exitcode(status)
                    assert proc.returncode == 0, f'SIGTERM exit: {proc.returncode}'
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError('exporter did not stop on SIGTERM')
            wall = time.monotonic() - start
            return {'wall_seconds': round(wall, 3), 'cpu_seconds': round(usage.ru_utime + usage.ru_stime, 4),
                    'peak_sampled_rss_kib': max(rss_kib), 'peak_threads': max(threads, default=None),
                    'scrapes': len(samples), 'scrape_mean_ms': round(sum(samples) / len(samples), 3),
                    'scrape_max_ms': round(max(samples), 3), 'metric_lines': len(body.splitlines())}
        finally:
            if proc.returncode is None:
                proc.kill()
                proc.wait()


def tui(binary):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
    original = termios.tcgetattr(slave)
    proc = subprocess.Popen([str(binary), 'top'], stdin=slave, stdout=slave, stderr=slave,
                            env={**os.environ, 'TERM': 'xterm-256color'})
    output = bytearray()
    try:
        started = time.monotonic()
        keys = [(2, b'2'), (3, b'3'), (5, b'1'), (6, b'?'), (6.5, b'x'), (7, b'q')]
        while time.monotonic() - started < 12:
            elapsed = time.monotonic() - started
            while keys and elapsed >= keys[0][0]:
                os.write(master, keys.pop(0)[1])
            if select.select([master], [], [], 0.1)[0]:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                # Crossterm asks the terminal for its cursor position at startup.
                if b'\x1b[6n' in chunk:
                    os.write(master, b'\x1b[1;1R')
            if proc.poll() is not None:
                break
        assert proc.poll() == 0, output[-3000:].decode(errors='replace')
        assert b'Processes' in output, 'process tab was not rendered'
        assert termios.tcgetattr(slave) == original, 'terminal settings were not restored'
        return {'exit_code': proc.returncode, 'rendered_bytes': len(output), 'terminal_restored': True}
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        os.close(master)
        os.close(slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--vendor', choices=['apple', 'nvidia'], required=True)
    parser.add_argument('--seconds', type=int, default=15)
    args = parser.parse_args()
    bins = args.bin_dir.resolve()
    cli = str(bins / 'plow-smi')
    listing = capture([cli, 'ctl', args.vendor + '-list', '--format', 'json'])
    info = capture([cli, 'ctl', args.vendor + '-info', '0', '--format', 'json'])
    result = {'vendor': args.vendor, 'binary_bytes': {name: (bins / name).stat().st_size for name in
              ['plow-smi', 'plows-exporter', 'plows-top', 'plows-ctl']}, 'list': listing, 'info': info}
    result['exporter'] = exporter(bins / 'plows-exporter', args.vendor, args.seconds)
    result['tui'] = tui(bins / 'plow-smi')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()

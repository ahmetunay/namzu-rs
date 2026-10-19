#!/usr/bin/env python3
"""5.2 real release CLI + loopback speech wire/cancel acceptance. No live inference."""
import email.parser
import email.policy
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import threading
import wave

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'


def wav(seconds=0.1):
    data = io.BytesIO()
    with wave.open(data, 'wb') as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(8000)
        output.writeframes(struct.pack('<h', 1234) * int(8000 * seconds))
    return data.getvalue()


class Fixture(http.server.HTTPServer):
    def __init__(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    size = int(self.headers['Content-Length'])
                    assert 0 < size < 13 * 1024 * 1024
                    body = self.rfile.read(size)
                    assert self.headers['Authorization'] == 'Bearer fixture-only-not-real'
                    self.server.requests.append(self.path)
                    if self.path == '/v1/audio/transcriptions':
                        message = email.parser.BytesParser(policy=email.policy.default).parsebytes(
                            b'MIME-Version: 1.0\r\nContent-Type: ' + self.headers['Content-Type'].encode() + b'\r\n\r\n' + body)
                        parts = {p.get_param('name', header='Content-Disposition'): p for p in message.iter_parts()}
                        assert set(parts) == {'model', 'response_format', 'language', 'file'}
                        assert parts['file'].get_filename() == 'input.wav'
                        assert parts['file'].get_payload(decode=True) == wav()
                        assert parts['model'].get_payload(decode=True) == b'stt-fixture'
                        assert parts['language'].get_payload(decode=True) == b'tr'
                        assert parts['response_format'].get_payload(decode=True) == b'json'
                        payload = json.dumps({'text': 'Türkçe deneme ✓', 'language': 'tr'}).encode()
                    else:
                        assert self.path == '/v1/audio/speech'
                        assert json.loads(body) == {'model': 'tts-fixture', 'input': 'Merhaba ✓', 'voice': 'alloy', 'response_format': 'wav'}
                        payload = wav(61) if self.server.mode == 'long' else wav()
                        if self.server.mode == 'normal':
                            streamed = bytearray(payload)
                            streamed[4:8] = b'\xff' * 4
                            streamed[40:44] = b'\xff' * 4
                            payload = bytes(streamed)
                    if self.server.mode == 'error':
                        self.send_response(503)
                        payload = b'private-service-body-and-private-key'
                    else:
                        self.send_response(200)
                    if self.server.mode == 'stall':
                        self.send_header('Content-Length', '100000')
                        self.end_headers()
                        self.wfile.write(b'R')
                        self.wfile.flush()
                        self.server.started.set()
                        self.connection.settimeout(5)
                        assert self.connection.recv(1) == b''
                        self.server.closed.set()
                        return
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    self.server.errors.append((type(error).__name__, str(error)))
                    try: self.send_error(500)
                    except OSError: pass

            def log_message(self, *_): pass
        super().__init__(('127.0.0.1', 0), Handler)
        self.requests = []
        self.errors = []
        self.mode = 'normal'
        self.started = threading.Event()
        self.closed = threading.Event()
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.shutdown()
        self.thread.join()
        self.server_close()
        assert not self.errors, self.errors


def main():
    env = dict(os.environ)
    env['NAMZU_SPEECH_FIXTURE_KEY'] = 'fixture-only-not-real'
    for key in ('NO_PROXY', 'no_proxy'):
        env[key] = ','.join(filter(None, [env.get(key), '127.0.0.1', 'localhost']))
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-audio-acceptance-') as directory:
        root = Path(directory)
        (root / 'input.wav').write_bytes(wav())
        server = Fixture()
        service = ['--base-url', f'http://127.0.0.1:{server.server_port}/v1', '--api-key-env', 'NAMZU_SPEECH_FIXTURE_KEY']
        def command(*args, success=True):
            p = subprocess.run([str(BINARY), 'audio', '--cwd', str(root), *args], env=env, capture_output=True, timeout=15)
            assert (p.returncode == 0) == success, (p.stdout.decode(), p.stderr.decode())
            return json.loads(p.stdout)
        def speech(name, timeout=None):
            return [str(BINARY), 'audio', '--cwd', str(root), *(['--timeout', str(timeout)] if timeout else []), 'synthesize', 'Merhaba ✓', '--output', name, '--voice', 'alloy', '--model', 'tts-fixture', *service]
        try:
            inspected = command('inspect', 'input.wav')['output']
            assert inspected['offline'] and inspected['audio']['frames'] == 800
            assert inspected['audio']['sha256'] == hashlib.sha256(wav()).hexdigest()
            assert not server.requests
            checks.append('offline_wav_inspection')
            transcribed = command('transcribe', 'input.wav', '--language', 'tr', '--model', 'stt-fixture', *service)['output']
            assert transcribed['transcript']['text'] == 'Türkçe deneme ✓'
            assert [e['type'] for e in transcribed['events']] == ['started', 'input_validated', 'completed']
            assert 'Türkçe deneme' not in json.dumps(transcribed['events'], ensure_ascii=False)
            checks.append('multipart_exact_input_and_unicode_transcript')
            p = subprocess.run(speech('out.wav'), env=env, capture_output=True, timeout=15)
            assert p.returncode == 0, p.stdout.decode()
            value = json.loads(p.stdout)['output']
            assert (root / 'out.wav').read_bytes() == wav()
            assert (root / 'out.wav').stat().st_mode & 0o777 == 0o600
            assert value['audio']['sha256'] == hashlib.sha256(wav()).hexdigest()
            checks.append('speech_json_and_private_atomic_wav_output')
            count = len(server.requests)
            p = subprocess.run(speech('out.wav'), env=env, capture_output=True, timeout=15)
            assert p.returncode != 0 and len(server.requests) == count
            assert (root / 'out.wav').read_bytes() == wav()
            (root / 'bad.wav').write_bytes(b'not audio')
            command('transcribe', 'bad.wav', '--model', 'stt-fixture', *service, success=False)
            assert len(server.requests) == count
            checks.append('invalid_input_and_existing_output_prevent_requests')
            server.mode = 'long'
            p = subprocess.run(speech('long.wav'), env=env, capture_output=True, timeout=15)
            assert p.returncode != 0 and not (root / 'long.wav').exists()
            checks.append('over_duration_service_audio_never_published')
            server.mode = 'error'
            count = len(server.requests)
            p = subprocess.run(speech('error.wav'), env=env, capture_output=True, timeout=15)
            assert p.returncode != 0 and len(server.requests) == count + 1
            assert b'private-service' not in p.stdout + p.stderr
            assert not (root / 'error.wav').exists()
            checks.append('service_failure_redacted_no_retry_no_file')
            for cancel in (True, False):
                server.mode = 'stall'
                server.started.clear()
                server.closed.clear()
                p = subprocess.Popen(speech('cancel.wav' if cancel else 'deadline.wav', None if cancel else 1), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                assert server.started.wait(5)
                if cancel: p.send_signal(signal.SIGINT)
                stdout, stderr = p.communicate(timeout=5)
                assert p.returncode != 0
                assert ('cancelled' if cancel else 'deadline') in json.loads(stdout)['error']
                assert server.closed.wait(3), 'HTTP must close when the CLI stops waiting'
                assert not (root / ('cancel.wav' if cancel else 'deadline.wav')).exists()
                checks.append('cli_sigint_closes_http' if cancel else 'cli_deadline_closes_http')
            assert not list(root.glob('.namzu-audio-*'))
        finally: server.close()
    report = {'checks': checks, 'live_model_calls': 0, 'speech_inference_calls': 0}
    (ROOT / 'artifacts/audio52-acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__': main()

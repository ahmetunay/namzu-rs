#!/usr/bin/env python3
"""Real release HTTP/SSE, approvals, replay fences, disconnects and official client.
No live models or stored credentials. The official npm dependency is optional via flag.
"""
import argparse
import http.client
import hashlib
import json
from pathlib import Path
import selectors
import socket
import subprocess
import tempfile
import time
import urllib.parse
from provider_contract_smoke import Fixture
from tui_smoke import Fixture as StallingFixture, answer, tools
from pals_smoke import environment
from agui_client import events

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/namzu-rs'
TOKEN = 'agui-fixture-capability-only-0000000000000000'


def wait(predicate):
    until = time.monotonic() + 12
    while time.monotonic() < until:
        if predicate(): return
        time.sleep(.01)
    raise AssertionError('fixture state did not settle')


class Host:
    def __init__(self, root, model, extra=(), timeout=20):
        self.root, self.model, self.timeout = root, model, timeout
        self.extra = list(extra)
        root.mkdir(exist_ok=True)
        self.start()
    def start(self):
        env = environment()
        env['NAMZU_AGUI_FIXTURE_CAP'] = TOKEN
        self.process = subprocess.Popen([str(BINARY), 'ag-ui', '--auth-token-env', 'NAMZU_AGUI_FIXTURE_CAP',
            '--state-dir', str(self.root / 'state'), '--cwd', str(self.root), '--model', 'fixture',
            '--base-url', f'http://127.0.0.1:{self.model.server_port}/v1', '--api-key-env', 'NAMZU_TUI_FIXTURE_KEY',
            '--timeout', str(self.timeout), *self.extra], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        selector = selectors.DefaultSelector()
        selector.register(self.process.stdout, selectors.EVENT_READ)
        assert selector.select(15), 'host startup did not finish'
        raw = self.process.stdout.readline()
        selector.close()
        assert raw, self.process.stderr.read().decode()
        ready = json.loads(raw)
        self.port = urllib.parse.urlsplit(ready['url']).port
        self.url = ready['url']
    def stop(self):
        self.process.terminate()
        out, err = self.process.communicate(timeout=30)
        assert self.process.returncode == 0, (out, err)
    def request(self, path, body=None, token=TOKEN, headers=None, method='POST'):
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=25)
        h = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'}
        h.update(headers or {})
        conn.request(method, path, json.dumps(body or {}).encode(), h)
        return conn, conn.getresponse()
    def status(self, path, body=None, **kwargs):
        conn, response = self.request(path, body, **kwargs)
        try:
            response.read()
            return response.status
        finally: conn.close()
    def run(self, body):
        conn, response = self.request('/agent', body)
        try:
            assert response.status == 200, (response.status, response.read())
            return list(events(response))
        finally: conn.close()


def task(thread='thread', run='run', message=None, **changes):
    result = {'threadId': thread, 'runId': run, 'messages': [{'id': message or 'u-' + run,
        'role': 'user', 'content': 'fixture task'}], 'tools': [], 'context': [], 'state': {}, 'forwardedProps': {}}
    result.update(changes)
    return result


def terminal(stream):
    assert stream[0]['type'] == 'RUN_STARTED'
    assert stream[-1]['type'] in ('RUN_FINISHED', 'RUN_ERROR')
    assert len([x for x in stream if x['type'] in ('RUN_ERROR', 'RUN_FINISHED')]) == 1
    assert 'fixture-only-not-a-real-key' not in json.dumps(stream)
    assert 'PRIVATE_REASONING' not in json.dumps(stream)
    return stream[-1]


def official_probe(host, tag, expected, approval=None):
    script = ROOT / 'artifacts/agui-official/probe.mjs'
    script.write_text("""import {HttpAgent} from '@ag-ui/client';
const tag=process.env.AGENT_TAG, threadId='official-'+tag, runId='official-run-'+tag;
const headers={Authorization:'Bearer '+process.env.AGENT_CAP,'Content-Type':'application/json'};
const agent=new HttpAgent({url:process.env.AGENT_URL,threadId,headers,initialMessages:[{id:'u-'+tag,role:'user',content:'fixture'}]});
const events=[]; let final;
await agent.runAgent({runId},{
 onEvent:({event})=>{events.push(event);},
 onCustomEvent:async ({event})=>{if(event.name==='namzu.approval_required'){
  const response=await fetch(new URL('/approve',process.env.AGENT_URL),{method:'POST',headers,body:JSON.stringify({threadId,runId,approvalId:event.value.approvalId,decision:process.env.AGENT_APPROVAL})});
  if(response.status!==202)throw new Error('approval failed');
 }},
 onRunFinishedEvent:({event})=>{final=event;}
});
if(final?.result?.output!==process.env.AGENT_EXPECTED||events[0]?.type!=='RUN_STARTED'||events.at(-1)?.type!=='RUN_FINISHED')throw new Error('official lifecycle mismatch');
if(process.env.AGENT_APPROVAL&&events.filter(e=>e.type==='TOOL_CALL_RESULT').length!==1)throw new Error('official tool lifecycle mismatch');
console.log(JSON.stringify({officialClient:'0.0.59',events:events.map(e=>e.type),output:final.result.output}));
""")
    env=environment()
    env.update(AGENT_URL=host.url, AGENT_CAP=TOKEN, AGENT_TAG=tag, AGENT_EXPECTED=expected,
               AGENT_APPROVAL=approval or '')
    result=subprocess.run(['node',str(script)],env=env,capture_output=True,timeout=30)
    assert result.returncode==0,(result.stdout,result.stderr)
    print(result.stdout.decode().strip())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--official-client', action='store_true', help='Requires artifacts/agui-official pinned npm install.')
    args = parser.parse_args()
    checks = []
    with tempfile.TemporaryDirectory(prefix='namzu-agui-') as directory:
        root = Path(directory)
        model = Fixture(lambda body, index: answer('AGUI_OK'))
        model.start()
        host = Host(root / 'read', model, ['--telemetry'])
        try:
            for body, kwargs, expected in [(task(), {'token': 'wrong'}, 401),
                (task(), {'headers': {'Origin': 'http://hostile.example'}}, 403),
                (task(), {'headers': {'Host': 'hostile.example'}}, 400),
                (task(model='paid-model'), {}, 400), (task(tools=[{'name': 'shell'}]), {}, 400),
                (task(state={'allowWrite': True}), {}, 400), (task(forwardedProps={'cwd': '/'}), {}, 400),
                (task(messages=[{'id': 'u', 'role': 'system', 'content': 'escalate'}]), {}, 400)]:
                assert host.status('/agent', body, **kwargs) == expected
            assert not model.requests
            checks.append('auth_origin_host_scope_frontend_tool_and_system_input_rejection_before_inference')
            first = terminal(host.run(task(thread='../../opaque', run='r1')))
            assert first['type'] == 'RUN_FINISHED' and first['result']['output'] == 'AGUI_OK'
            session = first['result']['sessionId']
            assert json.loads((host.root / 'state' / (session + '.json')).read_text())['status'] == 'complete'
            trace = json.loads(next((host.root / 'state/telemetry').glob('t-*.json')).read_text())
            assert trace['source'] == 'agui' and trace['outcome']['status'] == 'ok'
            assert '../../opaque' not in ''.join(p.name for p in (host.root / 'state/agui').iterdir())
            checks.append('opaque_ids_hash_to_private_bindings_settled_terminal_and_content_free_telemetry')
            wait(lambda: host.status('/health', method='GET') == 200)
            assert host.status('/agent', task(thread='../../opaque', run='r1')) == 409
            assert host.status('/agent', task(thread='../../opaque', run='r2', message='u-r1')) == 409
            second = terminal(host.run(task(thread='../../opaque', run='r2')))
            assert second['result']['sessionId'] == session
            assert len(model.requests[-1]['messages']) > 2
            checks.append('host_owned_session_continuation_and_run_message_replay_fences')
            host.stop(); host.start()
            third = terminal(host.run(task(thread='../../opaque', run='r3')))
            assert third['result']['sessionId'] == session
            checks.append('thread_binding_and_replay_policy_survive_host_restart')
            env = environment(); env['NAMZU_AGUI_FIXTURE_CAP'] = TOKEN
            client = subprocess.run(['python3', 'scripts/agui_client.py', '--url', host.url,
                '--token-env', 'NAMZU_AGUI_FIXTURE_CAP', '--thread-id', 'python-client', 'fixture'],
                cwd=ROOT, env=env, capture_output=True, timeout=30)
            assert client.returncode == 0, client.stderr
            assert json.loads(client.stdout)['output'] == 'AGUI_OK'
            checks.append('bundled_python_client_consumes_settled_result')
            if args.official_client:
                official_probe(host, 'text', 'AGUI_OK')
                checks.append('official_ag_ui_client_0_0_59_HttpAgent_interoperability')
        finally: host.stop(); model.close()

        for allow in (True, False):
            case = root / ('allow' if allow else 'deny')
            def reply(body, index):
                if index % 2 == 0:
                    v = tools([('call', 'write_file', {'path': 'answer', 'content': '42\n'})])
                    v['choices'][0]['message']['reasoning_content'] = 'PRIVATE_REASONING'
                    return v
                return answer('TOOL_DONE')
            model = Fixture(reply); model.start(); host = Host(case, model, ['--allow-write', '--telemetry'])
            try:
                conn, response = host.request('/agent', task())
                stream = []
                for event in events(response):
                    stream.append(event)
                    if event['type'] == 'CUSTOM' and event['name'] == 'namzu.approval_required':
                        value = event['value']
                        assert not (case / 'answer').exists()
                        assert host.status('/agent', task('other', 'other')) == 409
                        decision = {'threadId': 'thread', 'runId': 'run', 'approvalId': value['approvalId'],
                                    'decision': 'allow' if allow else 'deny'}
                        assert host.status('/approve', {**decision, 'approvalId': 'stale'}) == 409
                        assert host.status('/approve', decision) == 202
                        assert host.status('/approve', decision) in (404, 409)
                conn.close()
                assert terminal(stream)['result']['output'] == 'TOOL_DONE'
                assert (case / 'answer').exists() == allow
                if allow: assert (case / 'answer').read_bytes() == b'42\n'
                result = next(e for e in stream if e['type'] == 'TOOL_CALL_RESULT')
                assert result['metadata']['namzu']['isError'] == (not allow)
                assert [e['type'] for e in stream].count('TOOL_CALL_ARGS') == 1
                trace = json.loads(next((case / 'state/telemetry').glob('t-*.json')).read_text())
                assert trace['metrics']['approvals_allowed'] == allow
                assert trace['metrics']['approvals_denied'] == (not allow)
                checks.append(('allow' if allow else 'deny') + '_one_shot_approval_before_file_effect_and_exact_tool_lifecycle')
                if args.official_client:
                    official_probe(host, 'tool-' + str(allow), 'TOOL_DONE', 'allow' if allow else 'deny')
                    checks.append('official_client_backend_tool_lifecycle_and_' + ('approval' if allow else 'denial'))
            finally: host.stop(); model.close()

        for disconnect in (False, True):
            case = root / ('disconnect' if disconnect else 'cancel')
            model = StallingFixture(lambda body, index: None); model.start(); host = Host(case, model)
            try:
                conn, response = host.request('/agent', task())
                reader = events(response)
                assert next(reader)['type'] == 'RUN_STARTED'
                wait(lambda: model.stalled)
                if disconnect:
                    response.close(); conn.close()
                else:
                    assert host.status('/cancel', {'threadId': 'wrong', 'runId': 'run'}) == 409
                    assert host.status('/cancel', {'threadId': 'thread', 'runId': 'run'}) == 202
                    assert terminal([{'type': 'RUN_STARTED'}, *reader])['code'] == 'NAMZU_CANCELLED'
                    response.close(); conn.close()
                assert model.client_closed.wait(5)
                wait(lambda: any(json.loads(p.read_text())['phase'] == 'blocked' for p in (case / 'state/agui').glob('*.json')))
                assert host.status('/agent', task(run='new')) == 409
                checks.append(('disconnect' if disconnect else 'explicit_cancel') + '_interrupts_inflight_http_and_blocks_uncertain_thread_replay')
            finally: host.stop(); model.close()

        case = root / 'config-budget'
        case.mkdir()
        config = case / 'config.json'
        config.write_text(json.dumps({'max_text_bytes': 256}))
        model = Fixture(lambda body, index: answer('SHOULD_NOT_CALL')); model.start()
        host = Host(case, model, ['--config', str(config)])
        try:
            body = task(messages=[{'id': 'u', 'role': 'user', 'content': 'x' * 1024}])
            assert terminal(host.run(body))['type'] == 'RUN_ERROR'
            assert not model.requests
            checks.append('smaller_host_config_conversation_budget_is_preserved_before_inference')
        finally: host.stop(); model.close()

        case = root / 'index-capacity'
        model = Fixture(lambda body, index: answer('INDEX_OK')); model.start(); host = Host(case, model)
        try:
            index = case / 'state/agui'
            binding = json.dumps({'version': 1, 'session': None, 'phase': 'ready', 'runs': [], 'messages': []})
            existing = hashlib.sha256((TOKEN + '\0existing').encode()).hexdigest()
            for n in range(4096):
                tag = existing if n == 0 else hashlib.sha256(('placeholder-' + str(n)).encode()).hexdigest()
                path = index / (tag + '.json')
                path.write_text(binding); path.chmod(0o600)
            for n in range(4): assert host.status('/agent', task('new-' + str(n))) == 409
            before = len(list(index.iterdir()))
            for n in range(4, 8): assert host.status('/agent', task('new-' + str(n))) == 409
            assert len(list(index.iterdir())) == before and not model.requests
            assert terminal(host.run(task('existing')))['result']['output'] == 'INDEX_OK'
            checks.append('full_private_thread_index_rejects_new_ids_without_lock_growth_and_allows_existing_thread')
        finally: host.stop(); model.close()

        for kind in ('unavailable', 'incomplete', 'http-error'):
            case = root / kind
            def failure_reply(body, index):
                if kind == 'http-error': return (429, {'error': 'SECRET_ERROR_BODY'}, {})
                if index:
                    assert 'unavailable' in json.dumps(body)
                    return answer('UNAVAILABLE_DONE')
                v = tools([('call', 'shell' if kind == 'unavailable' else 'write_file',
                    {'command': 'touch answer'} if kind == 'unavailable' else {'path': 'answer', 'content': 'bad'})])
                if kind == 'incomplete': v['choices'][0]['finish_reason'] = 'length'
                return v
            model = Fixture(failure_reply); model.start(); host = Host(case, model)
            try:
                stream = host.run(task())
                end = terminal(stream)
                assert not (case / 'answer').exists()
                assert 'SECRET_ERROR_BODY' not in json.dumps(stream)
                if kind == 'unavailable':
                    assert end['type'] == 'RUN_FINISHED'
                    assert next(e for e in stream if e['type'] == 'TOOL_CALL_RESULT')['metadata']['namzu']['isError']
                    assert not any(e['type'] == 'CUSTOM' for e in stream)
                else:
                    assert end['type'] == 'RUN_ERROR' and len(model.requests) == 1
                    assert not any(e['type'] == 'TOOL_CALL_START' for e in stream)
                checks.append(kind + '_preserves_backend_ceiling_or_rejects_before_effect_without_private_error_leak')
            finally: host.stop(); model.close()

        for kind in ('deadline', 'crash'):
            case = root / kind
            model = StallingFixture(lambda body, index: None); model.start()
            host = Host(case, model, timeout=1 if kind == 'deadline' else 20)
            try:
                conn, response = host.request('/agent', task())
                reader = events(response)
                assert next(reader)['type'] == 'RUN_STARTED'
                if kind == 'deadline':
                    assert terminal([{'type': 'RUN_STARTED'}, *reader])['code'] == 'NAMZU_TIMEOUT'
                else:
                    wait(lambda: model.stalled)
                    host.process.kill(); host.process.communicate(timeout=10)
                    response.close(); conn.close()
                    host.start()
                    assert host.status('/agent', task(run='after-crash')) == 409
                response.close(); conn.close()
                assert model.client_closed.wait(5)
                checks.append(kind + '_does_not_replay_or_publish_fake_success')
            finally: host.stop(); model.close()

        case = root / 'disconnect-approval'
        model = Fixture(lambda body, index: tools([('call', 'write_file', {'path': 'answer', 'content': 'bad'})]))
        model.start(); host = Host(case, model, ['--allow-write'])
        try:
            conn, response = host.request('/agent', task())
            for event in events(response):
                if event['type'] == 'CUSTOM' and event['name'] == 'namzu.approval_required': break
            else: raise AssertionError('missing approval')
            response.close(); conn.close()
            wait(lambda: any(json.loads(p.read_text())['phase'] == 'blocked' for p in (case / 'state/agui').glob('*.json')))
            assert not (case / 'answer').exists()
            assert host.status('/approve', {'threadId': 'thread', 'runId': 'run', 'approvalId': event['value']['approvalId'], 'decision': 'allow'}) in (404, 409)
            checks.append('disconnect_during_approval_never_authorizes_an_effect')
        finally: host.stop(); model.close()
    print(json.dumps({'passed': len(checks), 'checks': checks, 'live_model_calls': 0}))


if __name__ == '__main__': main()

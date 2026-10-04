#!/usr/bin/env python3
"""One conservative worker step for the LOCAL demo service. Run repeatedly to drain."""
import argparse
import base64
import fcntl
import json
import os
from pathlib import Path
import subprocess
import urllib.request


def save(path, value):
    tmp = path.with_suffix('.tmp')
    with tmp.open('w') as f:
        json.dump(value, f)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    fd = os.open(path.parent, os.O_RDONLY)
    try: os.fsync(fd)
    finally: os.close(fd)


def reconcile(path, cli):
    if not path.exists(): return
    pending = json.loads(path.read_text())
    if not pending.get('txhash'):
        raise RuntimeError('ambiguous broadcast: reconcile journal manually; refusing to retry')
    receipt = cli('query', 'tx', pending['txhash'])
    if int(receipt.get('height', 0)) <= 0:
        raise RuntimeError('transaction not confirmed; refusing to retry')
    print(json.dumps({'confirmed_tx': pending['txhash'], 'code': receipt.get('code', 0)}))
    path.unlink()  # Next step queries onchain state again, including after a failed tx.


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--wasmd', default='wasmd')
    p.add_argument('--home', required=True)
    p.add_argument('--node', default='tcp://127.0.0.1:26657')
    p.add_argument('--chain-id', default='durable-local')
    p.add_argument('--key', default='demo')
    p.add_argument('--workflow', required=True)
    p.add_argument('--service', required=True)
    p.add_argument('--journal', type=Path, required=True)
    p.add_argument('--mode', choices=['success', 'remote-error', 'invalid'], default='success')
    args = p.parse_args()
    # A journal is scoped to one chain/service/account; do not share it between workers.
    args.journal.parent.mkdir(parents=True, exist_ok=True)
    with args.journal.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        def cli(*parts):
            cmd = [args.wasmd, *parts, '--home', args.home, '--node', args.node, '--output', 'json']
            return json.loads(subprocess.check_output(cmd, text=True, timeout=30))
        def query(address, msg):
            return cli('query', 'wasm', 'contract-state', 'smart', address, json.dumps(msg))['data']
        scope = {'chain_id': args.chain_id, 'node': args.node, 'home': str(Path(args.home).resolve()),
                 'key': args.key, 'workflow': args.workflow, 'service': args.service}
        if args.journal.exists() and json.loads(args.journal.read_text()).get('scope') != scope:
            raise RuntimeError('journal scope mismatch; refusing to reconcile another worker')
        reconcile(args.journal, cli)
        requests = query(args.service, {'Pending': {'start_after': None, 'limit': 1}})
        if not requests:
            print('No pending requests.'); return
        req = requests[0]
        correlation = req['correlation']
        instance = query(args.workflow, {'Instance': {'workflow_id': correlation['workflow_id']}})
        print(json.dumps({'workflow': correlation['workflow_id'], 'status': instance['status']}))
        wait = instance['status'].get('Waiting', {}).get('wait')
        address = args.service
        if not wait or wait['sequence'] != correlation['wait_sequence']:
            msg = {'Prune': {'id': req['id']}}
        else:
            with urllib.request.urlopen(args.node.replace('tcp://', 'http://') + '/status', timeout=10) as r:
                height = int(json.load(r)['result']['sync_info']['latest_block_height'])
            deadline = wait['deadline']
            if 'Height' not in deadline: raise RuntimeError('demo expects height deadlines')
            if height >= deadline['Height']:
                address = args.workflow
                msg = {'Expire': correlation}
            else:
                if args.mode == 'remote-error':
                    outcome = {'Error': {'code': 'demo_declined', 'message': 'illustrative failure'}}
                elif args.mode == 'invalid':
                    outcome = {'Success': base64.b64encode(b'invalid JSON').decode()}
                else:
                    if wait['operation'] == 'payment_received': value = {'reference': 'DEMO-NO-FUNDS'}
                    elif wait['operation'] == 'shipment_received': value = {'tracking': 'DEMO-NO-SHIPMENT'}
                    else: raise RuntimeError('unknown operation')
                    outcome = {'Success': base64.b64encode(json.dumps(value).encode()).decode()}
                msg = {'Deliver': {'id': req['id'], 'outcome': outcome}}
        save(args.journal, {'intent': msg, 'contract': address, 'scope': scope})
        tx = cli('tx', 'wasm', 'execute', address, json.dumps(msg), '--from', args.key,
                 '--keyring-backend', 'test', '--chain-id', args.chain_id, '--gas', '2000000',
                 '--fees', '5000stake', '--broadcast-mode', 'sync', '--yes')
        if int(tx.get('code', 0)) != 0:
            args.journal.unlink()
            raise RuntimeError(f'CheckTx rejected: {tx}')
        if not tx.get('txhash'): raise RuntimeError('ambiguous broadcast without txhash')
        save(args.journal, {'txhash': tx['txhash'], 'intent': msg, 'contract': address, 'scope': scope})
        print(json.dumps({'broadcast_tx': tx['txhash'], 'next': 'rerun to confirm and rediscover'}))


if __name__ == '__main__': main()

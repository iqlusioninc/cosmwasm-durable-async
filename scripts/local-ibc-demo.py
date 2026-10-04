#!/usr/bin/env python3
"""Run two fresh local wasmd chains and Hermes; never touches an existing chain home."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request
import urllib.error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasmd', default='wasmd')
    parser.add_argument('--hermes', default='hermes')
    parser.add_argument('--artifact', type=Path, default=Path('target/artifacts/ibc_query_example.wasm'))
    parser.add_argument('--home', type=Path, help='must not exist; keeps logs and receipts')
    parser.add_argument('--port', type=int, default=28657, help='first RPC port; also uses +10, -1, +9, +100, +110')
    args = parser.parse_args()
    home = args.home or Path(tempfile.mkdtemp(prefix='durable-ibc-')) / 'demo'
    home.mkdir(parents=True, exist_ok=False)
    artifact = args.artifact.resolve()
    checksum = hashlib.sha256(artifact.read_bytes()).hexdigest()
    processes = []
    logs = []
    def run(command, timeout=90):
        result = subprocess.run(command, text=True, capture_output=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f"{command[0:4]} failed: {result.stderr[-4000:]} {result.stdout[-4000:]}")
        return result.stdout
    def spawn(command, name):
        log = (home / name).open('w')
        logs.append(log)
        process = subprocess.Popen(command, stdout=log, stderr=log)
        processes.append(process)
        return process
    class Chain:
        def __init__(self, name, port):
            self.name, self.port = name, port
            self.path = home / name
            self.node = f'tcp://127.0.0.1:{port}'
        def cli(self, *parts):
            return run([args.wasmd, *map(str, parts), '--home', str(self.path)])
        def query(self, *parts):
            return json.loads(self.cli('query', *parts, '--node', self.node, '--output', 'json'))
        def tx(self, *parts):
            result = json.loads(self.cli('tx', 'wasm', *parts, '--from', 'owner', '--keyring-backend', 'test', '--chain-id', self.name, '--node', self.node, '--gas', '10000000', '--fees', '10000stake', '--yes', '--output', 'json'))
            if int(result.get('code', 0)):
                raise RuntimeError(result)
            for _ in range(100):
                try:
                    receipt = self.query('tx', result['txhash'])
                except RuntimeError:
                    time.sleep(.3)
                    continue
                if int(receipt.get('height', 0)):
                    (self.path / f"tx-{result['txhash']}.json").write_text(json.dumps(receipt, indent=2))
                    if int(receipt.get('code', 0)):
                        raise RuntimeError(receipt)
                    return receipt
                time.sleep(.3)
            raise RuntimeError(f"transaction not confirmed: {result}")
        def status(self, wid):
            return self.query('wasm', 'contract-state', 'smart', self.contract, json.dumps({'instance': {'workflow_id': wid}}))['data']['status']
        def start(self):
            self.cli('init', self.name, '--chain-id', self.name)
            for key in ['owner', 'relayer']:
                record = json.loads(self.cli('keys', 'add', key, '--keyring-backend', 'test', '--output', 'json'))
                self.cli('genesis', 'add-genesis-account', record['address'], '100000000000stake')
                if key == 'owner': self.owner = record['address']
                else:
                    mnemonic = self.path / 'relayer.mnemonic'
                    mnemonic.write_text(record['mnemonic'])
                    mnemonic.chmod(0o600)
            self.cli('genesis', 'gentx', 'owner', '100000000stake', '--chain-id', self.name, '--keyring-backend', 'test')
            self.cli('genesis', 'collect-gentxs')
            config = self.path / 'config/config.toml'
            config.write_text(config.read_text().replace('timeout_commit = "5s"', 'timeout_commit = "1s"'))
            app = self.path / 'config/app.toml'
            app.write_text(app.read_text().replace('minimum-gas-prices = ""', 'minimum-gas-prices = "0stake"'))
            process = spawn([args.wasmd, 'start', '--home', str(self.path), '--rpc.laddr', self.node, '--p2p.laddr', f'tcp://127.0.0.1:{self.port-1}', '--grpc.address', f'127.0.0.1:{self.port+100}', '--grpc-web.enable=false'], f'{self.name}.log')
            for _ in range(100):
                if process.poll() is not None: raise RuntimeError(f'{self.name} exited; see logs')
                try:
                    with urllib.request.urlopen(self.node.replace('tcp://', 'http://') + '/status', timeout=2) as response:
                        if int(json.load(response)['result']['sync_info']['latest_block_height']) > 0: return
                except (urllib.error.URLError, TimeoutError): pass
                time.sleep(.3)
            raise RuntimeError('node startup timeout')
    def attr(receipt, key):
        return [a['value'] for e in receipt['events'] for a in e['attributes'] if a['key'] == key][-1]
    try:
        a, b = Chain('durable-ibc-a', args.port), Chain('durable-ibc-b', args.port + 10)
        for chain in [a, b]: chain.start()
        config = home / 'hermes.toml'
        config.write_text('''[global]
log_level = 'info'
[mode.clients]
enabled = true
refresh = true
misbehaviour = true
[mode.connections]
enabled = false
[mode.channels]
enabled = false
[mode.packets]
enabled = true
clear_interval = 10
clear_on_start = true
tx_confirmation = true
''' + ''.join(f'''
[[chains]]
id = '{c.name}'
type = 'CosmosSdk'
rpc_addr = 'http://127.0.0.1:{c.port}'
grpc_addr = 'http://127.0.0.1:{c.port+100}'
event_source = {{ mode = 'pull', interval = '500ms', max_retries = 4 }}
account_prefix = 'wasm'
key_name = 'relayer'
key_store_folder = '{home / "hermes-keys"}'
store_prefix = 'ibc'
default_gas = 1000000
max_gas = 10000000
gas_price = {{ price = 0.01, denom = 'stake' }}
gas_multiplier = 1.2
max_msg_num = 10
max_tx_size = 2097152
clock_drift = '5s'
max_block_time = '10s'
trusting_period = '14days'
trust_threshold = '1/3'
address_type = {{ derivation = 'cosmos' }}
''' for c in [a, b]))
        def hermes(*parts, timeout=120):
            return run([args.hermes, '--config', str(config), *parts], timeout=timeout)
        for chain in [a, b]:
            hermes('keys', 'add', '--chain', chain.name, '--mnemonic-file', str(chain.path / 'relayer.mnemonic'))
            chain.code = attr(chain.tx('store', artifact), 'code_id')
            chain.contract = chain.cli('query', 'wasm', 'build-address', checksum, chain.owner, '64656d6f').strip()
        (home / 'connection.log').write_text(hermes('create', 'connection', '--a-chain', a.name, '--b-chain', b.name))
        for chain, peer in [(a, b), (b, a)]:
            actual = attr(chain.tx('instantiate2', chain.code, json.dumps({'connection_id': 'connection-0', 'counterparty_port': f'wasm.{peer.contract}', 'timeout_seconds': 30}), '64656d6f', '--label', 'ibc-demo', '--no-admin'), '_contract_address')
            assert actual == chain.contract, (actual, chain.contract)
        (home / 'channel.log').write_text(hermes('create', 'channel', '--a-chain', a.name, '--a-connection', 'connection-0', '--a-port', f'wasm.{a.contract}', '--b-port', f'wasm.{b.contract}', '--order', 'unordered', '--channel-version', 'durable-query-1'))
        def start_relayer(log_name):
            process = spawn([args.hermes, '--config', str(config), 'start'], log_name)
            for _ in range(100):
                if process.poll() is not None: raise RuntimeError('relayer startup failed')
                if 'Hermes has started' in (home / log_name).read_text(): return process
                time.sleep(.2)
            raise RuntimeError('relayer startup timeout')
        relayer = start_relayer('relayer.log')
        def wait_terminal(wid, desired):
            for _ in range(120):
                state = a.status(wid)
                if desired in state: return state
                if 'Waiting' not in state: raise RuntimeError(state)
                if relayer.poll() is not None: raise RuntimeError('relayer exited; see log')
                time.sleep(.5)
            raise RuntimeError(f'workflow {wid} did not reach {desired}')
        a.tx('execute', a.contract, json.dumps({'start': {'value': 5}}))
        success = wait_terminal(1, 'Completed')
        import base64
        assert json.loads(base64.b64decode(success['Completed']['output'])) == 20
        a.tx('execute', a.contract, json.dumps({'start': {'value': 2**64-1}}))
        failure = wait_terminal(2, 'Failed')
        assert 'overflow' in base64.b64decode(failure['Failed']['error']).decode()
        # Stop relaying before sending: destination cannot receive this packet.
        relayer.terminate()
        relayer.wait(timeout=15)
        a.tx('execute', a.contract, json.dumps({'start': {'value': 7}}))
        time.sleep(32)
        relayer = start_relayer('relayer-timeout.log')
        timeout = wait_terminal(3, 'Failed')
        assert 'Timeout' in base64.b64decode(timeout['Failed']['error']).decode()
        evidence = {'artifact_sha256': checksum, 'wasmd': a.cli('version').strip(), 'hermes': run([args.hermes, '--version']).strip(), 'contracts': [a.contract, b.contract], 'success': success, 'remote_error': failure, 'ibc_timeout': timeout}
        (home / 'results.json').write_text(json.dumps(evidence, indent=2))
        print(f'PASS: two actual chains, Hermes handshake, two waits, error acknowledgement, IBC timeout. Evidence: {home}')
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=10)
                except subprocess.TimeoutExpired: process.kill(); process.wait()
        for log in logs: log.close()


if __name__ == '__main__': main()

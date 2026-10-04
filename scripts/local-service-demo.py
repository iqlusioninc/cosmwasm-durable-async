#!/usr/bin/env python3
"""Start a fresh LOCAL wasmd chain and exercise delayed demo service callbacks."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.request
import urllib.error


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--wasmd', default='wasmd')
    p.add_argument('--artifacts', type=Path, default=Path('target/artifacts'))
    p.add_argument('--home', type=Path, help='must not exist; default creates a fresh temporary directory')
    p.add_argument('--port', type=int, default=27657)
    args = p.parse_args()
    home = args.home or Path(tempfile.mkdtemp(prefix='durable-local-')) / 'chain'
    if home.exists(): raise RuntimeError('refusing to reuse an existing chain home')
    root = Path(__file__).resolve().parent
    node = f'tcp://127.0.0.1:{args.port}'
    def cli(*parts):
        return subprocess.check_output([args.wasmd, *parts, '--home', str(home)], text=True, timeout=60, stderr=subprocess.PIPE)
    def query(*parts): return json.loads(cli('query', *parts, '--node', node, '--output', 'json'))
    def wait_tx(txhash, allow_failure=False):
        for _ in range(60):
            try:
                tx = query('tx', txhash)
                if int(tx.get('height', 0)) > 0:
                    if int(tx.get('code', 0)) and not allow_failure: raise RuntimeError(tx)
                    return tx
            except (subprocess.CalledProcessError, json.JSONDecodeError): pass
            time.sleep(0.25)
        raise RuntimeError(f'unconfirmed transaction {txhash}; inspect {home}')
    def tx(*parts, gas='5000000', allow_failure=False):
        result=json.loads(cli('tx', 'wasm', *parts, '--from','demo','--keyring-backend','test',
          '--chain-id','durable-local','--node',node,'--gas',gas,'--fees','5000stake',
          '--broadcast-mode','sync','--yes','--output','json'))
        if int(result.get('code',0)): raise RuntimeError(result)
        return wait_tx(result['txhash'], allow_failure)
    def attr(receipt,key):
        values=[a['value'] for e in receipt['events'] for a in e['attributes'] if a['key']==key]
        if not values: raise RuntimeError(f'missing {key}: {receipt}')
        return values[-1]
    def smart(address,msg): return query('wasm','contract-state','smart',address,json.dumps(msg))['data']
    cli('init','durable-demo','--chain-id','durable-local')
    # Disposable test-key material stays in this isolated local chain home.
    cli('keys','add','demo','--keyring-backend','test','--output','json')
    address=cli('keys','show','demo','--address','--keyring-backend','test').strip()
    cli('genesis','add-genesis-account',address,'100000000000stake')
    cli('genesis','gentx','demo','100000000stake','--chain-id','durable-local','--keyring-backend','test')
    cli('genesis','collect-gentxs')
    config=home/'config/config.toml'
    config.write_text(config.read_text().replace('timeout_commit = "5s"','timeout_commit = "1s"'))
    app=home/'config/app.toml'
    app.write_text(app.read_text().replace('minimum-gas-prices = ""','minimum-gas-prices = "0stake"'))
    log=(home/'node.log').open('w')
    chain=subprocess.Popen([args.wasmd,'start','--home',str(home),'--rpc.laddr',node,
        '--p2p.laddr',f'tcp://127.0.0.1:{args.port-1}','--grpc.enable=false','--grpc-web.enable=false'],stdout=log,stderr=log)
    try:
        for _ in range(60):
            if chain.poll() is not None: raise RuntimeError(f'node exited; inspect {home}/node.log')
            try:
                with urllib.request.urlopen(node.replace('tcp://','http://')+'/status') as r:
                    height=int(json.load(r)['result']['sync_info']['latest_block_height'])
                if height>0: break
                time.sleep(0.5)
            except (subprocess.CalledProcessError, urllib.error.URLError): time.sleep(0.5)
        else: raise RuntimeError('node did not start')
        service_code=attr(tx('store',str(args.artifacts/'demo_service.wasm')),'code_id')
        workflow_code=attr(tx('store',str(args.artifacts/'fulfill_example.wasm')),'code_id')
        service=attr(tx('instantiate',service_code,'{}','--label','demo-service','--no-admin'),'_contract_address')
        workflow=attr(tx('instantiate',workflow_code,json.dumps({'payment_service':service,'shipment_service':service,'deadline_blocks':15}), '--label','fulfill','--no-admin'),'_contract_address')
        tx('execute',service,json.dumps({'Bind':{'workflow':workflow}}))
        (home/'addresses.json').write_text(json.dumps({'service':service,'workflow':workflow},indent=2))
        def drive(mode='success'):
            subprocess.check_call([sys.executable,str(root/'service-driver.py'),'--wasmd',args.wasmd,
                '--home',str(home),'--node',node,'--workflow',workflow,'--service',service,
                '--journal',str(home/'driver.json'),'--mode',mode])
            time.sleep(1.5)
        def pending(): return smart(service,{'Pending':{'start_after':None,'limit':10}})
        def finish():
            for _ in range(8):
                # Each call is a new process: restart between every submission.
                drive()
                if not pending() and not (home/'driver.json').exists(): return
            raise RuntimeError('driver did not drain')
        def start(order):tx('execute',workflow,json.dumps({'Start':{'order':{'id':order}}}))
        def status(wid):return smart(workflow,{'Instance':{'workflow_id':wid}})['status']
        start(1);finish();assert 'Completed' in status(1)
        start(2);drive('remote-error');finish();assert 'Failed' in status(2)
        start(3);before=pending();state_before=status(3)
        low_gas=tx('execute',service,json.dumps({'Deliver':{'id':before[0]['id'],'outcome':{'Error':{'code':'demo','message':'test'}}}}),gas='100000',allow_failure=True)
        assert int(low_gas.get('code',0))==11, low_gas
        assert pending()==before and status(3)==state_before
        (home/'out-of-gas.json').write_text(json.dumps(low_gas,indent=2))
        drive('invalid');assert pending()==before
        finish();assert 'Completed' in status(3)
        start(4)
        deadline=status(4)['Waiting']['wait']['deadline']['Height']
        while True:
            with urllib.request.urlopen(node.replace('tcp://','http://')+'/status') as r:
                height=int(json.load(r)['result']['sync_info']['latest_block_height'])
            if height>=deadline:break
            time.sleep(0.5)
        finish();assert 'Failed' in status(4)
        results={str(wid):status(wid) for wid in range(1,5)}
        (home/'results.json').write_text(json.dumps(results,indent=2))
        print(f'PASS: success, remote failure, failed-tx retry, low-gas rollback, expiry, process restarts. Evidence: {home}')
    finally:
        chain.terminate()
        try:chain.wait(timeout=10)
        except subprocess.TimeoutExpired:chain.kill();chain.wait()
        log.close()


if __name__=='__main__':main()

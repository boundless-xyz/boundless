#!/usr/bin/env python3
"""Deploy fixtures and execute staging upgrades only on ephemeral localhost Anvil forks."""
import argparse,concurrent.futures,datetime,hashlib,json,os,socket,subprocess,time,urllib.request
from pathlib import Path
from urllib.parse import urlparse
ROOT=Path(__file__).resolve().parents[4]
if not (ROOT/'branches.json').is_file():ROOT=Path('/Users/angelo/dev/boundless-sunset')
SLOT='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc'
ZERO='0x'+'00'*20
SAFE='0xb04d1a222789a76e74168a919b43b20f66e24f0b'
TAIKO_SAFE='0x8e537aaeddbc98a9a095397adb7c0d738458cc8b'
MULTISEND='0x40A2aCCbd92BCA938b02010E17A5b8929b49130D'
SENDER='0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266'
READS={'eth_chainId','eth_getBlockByNumber','eth_getStorageAt','eth_getCode','eth_call'}
CHAINS={'base':(84532,'https://sepolia.base.org',['base']),'taiko':(167000,'https://rpc.taiko.xyz',['taiko']),'ethereum':(11155111,'https://ethereum-sepolia-rpc.publicnode.com',['accounting','mint','zkc'])}

def cmd(*args):return subprocess.check_output(args,text=True).strip()
def encode(signature,*args):return cmd('cast','calldata',signature,*map(str,args))
def keccak(data):return subprocess.check_output(['cast','keccak'],input=data.encode()).decode().strip()

class RPC:
    def __init__(self,url,local=False):
        p=urlparse(url)
        if local:assert p.scheme=='http' and p.hostname=='127.0.0.1' and p.port,'local writes require loopback'
        else:assert p.scheme=='https','upstream is read-only HTTPS'
        self.url=url;self.local=local
    def call(self,method,params):
        assert self.local or method in READS,'upstream write forbidden'
        payload=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params}).encode()
        for attempt in range(4):
            try:
                request=urllib.request.Request(self.url,data=payload,headers={'Content-Type':'application/json','User-Agent':'Boundless-local-staging-rehearsal'})
                with urllib.request.urlopen(request,timeout=40) as response:result=json.load(response)
                if 'error' in result:raise RuntimeError(json.dumps(result['error']))
                return result['result']
            except Exception:
                if attempt==3:raise
                time.sleep(attempt+1)
    def view(self,target,signature,*args,block='latest'):
        return self.call('eth_call',[{'to':target,'data':encode(signature,*args)},block])
    def uint(self,target,signature,*args,block='latest'):return int(self.view(target,signature,*args,block=block),16)
    def address(self,target,signature,*args):return '0x'+self.view(target,signature,*args)[-40:]
    def send(self,to,data,sender=SENDER):
        assert self.local and self.call('web3_clientVersion',[]).lower().startswith('anvil'),'writes require Anvil'
        tx={'from':sender,'data':data}
        if to:tx['to']=to
        estimate=int(self.call('eth_estimateGas',[tx]),16)
        tx['gas']=hex((estimate * 6 + 4)//5)
        txhash=self.call('eth_sendTransaction',[tx])
        for _ in range(60):
            receipt=self.call('eth_getTransactionReceipt',[txhash])
            if receipt:
                assert int(receipt['status'],16)==1,('local transaction failed',txhash)
                return receipt
            time.sleep(.1)
        raise TimeoutError(txhash)

def abi_addresses(raw):
    data=bytes.fromhex(raw[2:]);offset=int.from_bytes(data[:32]);n=int.from_bytes(data[offset:offset+32]);return ['0x'+data[offset+32+i*32+12:offset+64+i*32].hex() for i in range(n)]

def verify_runtime(rpc,target,artifact,candidate):
    expected=bytearray.fromhex(artifact['deployedBytecode']['object'][2:])
    refs=artifact['deployedBytecode'].get('immutableReferences',{});names={}
    def visit(v):
        if isinstance(v,dict):
            if str(v.get('id')) in refs and 'name' in v:names[str(v['id'])]=v['name']
            for child in v.values():visit(child)
        elif isinstance(v,list):
            for child in v:visit(child)
    visit(artifact.get('ast',{}))
    unknown=set(refs)-set(names)
    assert len(unknown)<=1,'unresolved immutable names'
    for identifier,locations in refs.items():
        name=names.get(identifier,'__self')
        value=('0x'+candidate[2:].rjust(64,'0')) if name=='__self' else rpc.view(target,name+'()')
        for ref in locations:expected[ref['start']:ref['start']+ref['length']]=bytes.fromhex(value[2:])
    actual=rpc.call('eth_getCode',[candidate,'latest'])
    assert actual.lower()=='0x'+expected.hex(),'candidate runtime/dependency mismatch'
    return keccak(actual)

def export_transaction(rpc,label,chain_id,steps,path,block):
    safe=TAIKO_SAFE if label=='taiko' else SAFE
    owners=abi_addresses(rpc.view(safe,'getOwners()'));threshold=rpc.uint(safe,'getThreshold()')
    assert threshold==3 and len(owners)==(5 if label=='ethereum' else 6),'unexpected Safe threshold or owner count'
    calls=[encode('upgradeToAndCall(address,bytes)',s['implementation'],'0x') for s in steps]
    if len(steps)==1:target=steps[0]['proxy'];data=calls[0];operation=0
    else:
        assert keccak(rpc.call('eth_getCode',[MULTISEND,'latest']))=='0xa9865ac2d9c7a1591619b188c4d88167b50df6cc0c5327fcbd1c8c75f7c066ad'
        packed='0x'+''.join('00'+s['proxy'][2:]+'00'*32+(len(c[2:])//2).to_bytes(32,'big').hex()+c[2:] for s,c in zip(steps,calls))
        target=MULTISEND;data=encode('multiSend(bytes)',packed);operation=1
    nonce=rpc.uint(safe,'nonce()')
    hash_args=[target,0,data,operation,0,0,0,ZERO,ZERO,nonce]
    txhash=rpc.view(safe,'getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)',*hash_args)
    transaction={'scope':'LOCAL FORK FIXTURE ONLY. Candidate addresses are not deployed on the live chain. Regenerate after actual staging deployment; do not submit this file to a live Safe.','chainId':chain_id,'blockNumber':block,'safe':safe,'to':target,'value':0,'data':data,'operation':operation,'safeTxGas':0,'baseGas':0,'gasPrice':0,'gasToken':ZERO,'refundReceiver':ZERO,'nonce':nonce,'safeTxHash':txhash,'threshold':threshold,'owners':owners,'steps':steps,'live_submission_ready':False}
    path.write_text(json.dumps(transaction,indent=2)+'\n')
    return transaction

def execute_exact(rpc,transaction):
    safe=transaction['safe'];owners=sorted(transaction['owners'],key=lambda a:int(a,16))[:transaction['threshold']]
    assert rpc.uint(safe,'nonce()')==transaction['nonce']
    for owner in owners:
        rpc.call('anvil_impersonateAccount',[owner]);rpc.call('anvil_setBalance',[owner,hex(10**20)])
        rpc.send(safe,encode('approveHash(bytes32)',transaction['safeTxHash']),owner)
    signatures='0x'+''.join(o[2:].rjust(64,'0')+'00'*32+'01' for o in owners)
    receipt=rpc.send(safe,encode('execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)',transaction['to'],0,transaction['data'],transaction['operation'],0,0,0,ZERO,ZERO,signatures))
    assert rpc.uint(safe,'nonce()')==transaction['nonce']+1
    for step in transaction['steps']:assert rpc.call('eth_getStorageAt',[step['proxy'],SLOT,'latest'])[-40:].lower()==step['implementation'][2:].lower()
    return {'transaction_hash':receipt['transactionHash'],'gas_used':int(receipt['gasUsed'],16),'real_threshold':transaction['threshold'],'owners_impersonated_locally':owners,'implementation_slots_verified':len(transaction['steps'])}

def run_one(label,output,manifest):
    chain_id,url,keys=CHAINS[label];upstream=RPC(url)
    assert int(upstream.call('eth_chainId',[]),16)==chain_id
    pinned=upstream.call('eth_getBlockByNumber',['latest',False]);number=int(pinned['number'],16)
    assert int(pinned['timestamp'],16)<1793836800,'rehearsal requires a pre-cutoff snapshot'
    print(label+': pinned block '+str(number),flush=True)
    folder=output/label;folder.mkdir(parents=True,exist_ok=True)
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    log=(folder/'anvil.log').open('w')
    hardfork={'base':'beryl','taiko':'shanghai','ethereum':'amsterdam'}[label]
    process=subprocess.Popen(['anvil','--network','base' if label=='base' else 'ethereum','--hardfork',hardfork,'--silent','--host','127.0.0.1','--port',str(port),'--fork-url',url,'--fork-block-number',str(number),'--chain-id',str(chain_id),'--gas-limit','40000000'],stdout=log,stderr=subprocess.STDOUT)
    rpc=RPC('http://127.0.0.1:'+str(port),local=True)
    try:
        for _ in range(80):
            if process.poll() is not None:raise RuntimeError(label+': Anvil stopped during startup; see local log')
            try:
                if rpc.call('web3_clientVersion',[]).lower().startswith('anvil'):break
            except Exception:time.sleep(.25)
        else:raise RuntimeError('Anvil startup timeout')
        assert int(rpc.call('eth_chainId',[]),16)==chain_id
        assert rpc.call('eth_getBlockByNumber',[hex(number),False])['hash']==pinned['hash']
        steps=[]
        for key in keys:
            entry=next(v for v in manifest['targets'] if v['environment']=='rehearsal' and v['contract']==key)
            artifact_path=ROOT/manifest['artifacts'][key]['path'];artifact=json.loads(artifact_path.read_text())
            assert hashlib.sha256(artifact_path.read_bytes()).hexdigest()==manifest['artifacts'][key]['artifact_sha256']
            assert rpc.call('eth_getStorageAt',[entry['proxy'],SLOT,'latest'])[-40:].lower()==entry['baseline_implementation'][2:].lower(),'staging baseline changed'
            assert keccak(rpc.call('eth_getCode',[entry['baseline_implementation'],'latest']))==entry['baseline_code_hash'],'baseline runtime changed'
            for arg in entry['constructor_inputs']:
                field={'router':'ROUTER','collateralTokenContract':'COLLATERAL_TOKEN_CONTRACT','legacyImpl':'LEGACY_IMPL','verifier':'VERIFIER','token':'TOKEN','logUpdaterId':'LOG_UPDATER_ID','accounting':'ACCOUNTING','mintCalculatorId':'MINT_CALCULATOR_ID','tokenRewards':'TOKEN_REWARDS'}[arg['name']]
                raw=rpc.view(entry['proxy'],field+'()')
                expected='0x'+arg['value'][2:].rjust(64,'0');assert raw.lower()==expected.lower(),'constructor dependency changed'
            safe=TAIKO_SAFE if label=='taiko' else SAFE
            if key in ['accounting','mint']:assert rpc.address(entry['proxy'],'owner()').lower()==safe.lower()
            else:assert rpc.uint(entry['proxy'],'hasRole(bytes32,address)','0x'+'00'*32,safe)==1
            creation=(ROOT/'preparation-evidence/deployment-inputs'/entry['creation_data_file']).read_text().strip()
            assert keccak(creation)==entry['creation_data_keccak256']
            receipt=rpc.send(None,creation);candidate=receipt['contractAddress']
            runtime_hash=verify_runtime(rpc,entry['proxy'],artifact,candidate)
            steps.append({'contract':key,'proxy':entry['proxy'],'baseline':entry['baseline_implementation'],'baselineCodeHash':keccak(rpc.call('eth_getCode',[entry['baseline_implementation'],'latest'])),'implementation':candidate,'runtimeCodeHash':runtime_hash,'artifact_sha256':manifest['artifacts'][key]['artifact_sha256'],'localCreationGasUsed':int(receipt['gasUsed'],16)})
        block=int(rpc.call('eth_blockNumber',[]),16)
        repo=ROOT/('zkc' if label=='ethereum' else 'market')
        local_bundle=repo/'out/staging-preparation'/label/'transaction.json';local_bundle.parent.mkdir(parents=True,exist_ok=True)
        reference=export_transaction(rpc,label,chain_id,steps,local_bundle,block)
        prepare_args=['python3',str(Path(__file__).with_name('prepare-staging.py')),'--chain',label,'--rpc-url',rpc.url,'--output',str(local_bundle)]
        for step in steps:prepare_args.extend(['--candidate',step['contract']+'='+step['implementation']])
        prepared=subprocess.run(prepare_args,capture_output=True,text=True)
        (folder/'preparation.log').write_text(prepared.stdout+prepared.stderr)
        assert prepared.returncode==0,label+': candidate preparation failed; see '+str(folder/'preparation.log')
        tx=json.loads(local_bundle.read_text())
        assert tx['data']==reference['data'] and tx['safeTxHash']==reference['safeTxHash'] and tx['nonce']==reference['nonce']
        assert tx['local_fixture'] and tx['rpc_reads_only'] and not tx['live_submission_ready']
        (folder/'transaction.json').write_text(local_bundle.read_text())
        env=os.environ.copy();env.update(FOUNDRY_FFI='false',FOUNDRY_TEST='staging-fork' if label=='ethereum' else 'contracts/staging-fork',FOUNDRY_PROFILE='shanghai' if label=='taiko' else 'default',SUNSET_STAGING_BUNDLE=str(local_bundle),FOUNDRY_BUILD_INFO_PATH='out/staging-build-info-'+label)
        contract='StagingEthereumForkTest' if label=='ethereum' else 'StagingMarketForkTest'
        tests=['forge','test','--offline','--network','base' if label=='base' else 'ethereum','--hardfork','base:'+hardfork if label=='base' else hardfork,'--fork-url',rpc.url,'--fork-block-number',str(block),'--out','out/staging-fork-'+label,'--cache-path','cache/staging-fork-'+label,'--match-contract',contract,'-vv']
        print(label+': candidates and exact Safe transaction prepared; running fork regressions',flush=True)
        with (folder/'tests.log').open('w') as tests_log:completed=subprocess.run(tests,cwd=repo,env=env,stdout=tests_log,stderr=subprocess.STDOUT,timeout=600)
        assert completed.returncode==0,label+': fork tests failed; see '+str(folder/'tests.log')
        # Foundry's fork is independent. Execute the same exported bytes on the parent Anvil too.
        execution=execute_exact(rpc,tx)
        result={'status':'passed','label':label,'chain_id':chain_id,'source_block':number,'source_block_hash':pinned['hash'],'local_candidate_block':block,'anvil_hardfork':hardfork,'foundry_runtime_hardfork':hardfork,'foundry_compiler_profile':env['FOUNDRY_PROFILE'],'anvil_version':cmd('anvil','--version').splitlines()[0],'transaction_file':str(folder/'transaction.json'),'tests_log':str(folder/'tests.log'),'anvil_execution':execution,'upstream_calls':'read-only; Anvil fetches fork state','live_writes':False,'live_submission_ready':False,'read_only_candidate_preparer_checked':True,'limitations':'Local fixture addresses and impersonated owners. Not a live staging deployment or signer-availability check.'}
        (folder/'result.json').write_text(json.dumps(result,indent=2)+'\n');print(label+': fork regressions and exact Anvil Safe execution passed',flush=True);return result
    finally:
        process.terminate()
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:process.kill();process.wait()
        log.close()

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--chain',choices=['all',*CHAINS],default='all');args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    manifest=json.loads((ROOT/'preparation-evidence/deployment-inputs/manifest.json').read_text())
    labels=list(CHAINS) if args.chain=='all' else [args.chain]
    results=[]
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        futures={pool.submit(run_one,label,args.output,manifest):label for label in labels}
        for future in concurrent.futures.as_completed(futures):
            try:results.append(future.result())
            except Exception as error:results.append({'label':futures[future],'status':'failed','error':str(error)});print(futures[future]+': '+str(error),flush=True)
    report={'checked_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'local_only':True,'results':results}
    (args.output/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    raise SystemExit(0 if all(r['status']=='passed' for r in results) else 1)
if __name__=='__main__':main()

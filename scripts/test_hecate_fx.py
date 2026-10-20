import argparse,json,subprocess,sys
from pathlib import Path
parser=argparse.ArgumentParser(description='Read or round-trip verified G2 Pro effects; --apply temporarily changes settings.')
parser.add_argument('--exe',type=Path,default=Path(__file__).resolve().parents[1]/'PeripheralDesk.exe')
parser.add_argument('--apply',action='store_true')
parser.add_argument('--report',type=Path,required=True)
args=parser.parse_args()
exe=args.exe.resolve()
config=json.loads((exe.parent/'data/hecate-sdk.json').read_text(encoding='utf-8-sig'))
def query(action=None):
    result=subprocess.run([str(exe),'--audio-fx-worker'],input=json.dumps({'path':config['path'],'action':action}).encode(),capture_output=True,timeout=12)
    if result.returncode:raise RuntimeError(f'Audio worker exited with code {result.returncode}')
    response=json.loads(result.stdout)
    if response['error']:raise RuntimeError(response['error'])
    return response['values']
before=query();results=[];errors=[]
# Fixed protocol limits, independently exercised against the actual driver.
limits={k:(40 if k.startswith('Eq') and k!='EqEnabled' else 100 if k=='SmartVolumeLevel' else 10 if k in ['BrillianceLevel','BassLevel','ClarityLevel'] else 3 if k=='VoiceMode' else 2 if k=='SurroundRoom' else 1) for k in before}
args.report.parent.mkdir(parents=True,exist_ok=True)
# A baseline survives a killed test process and can be used for manual recovery.
backup=args.report.with_suffix('.baseline.json');backup.write_text(json.dumps(before,indent=2),encoding='utf-8')
try:
    if args.apply:
        for key,original in before.items():
            changed=(original+1)%(limits[key]+1)
            try:
                after=query([key,changed])
                if after.get(key)!=changed:raise AssertionError(f'{key}: read-back differs')
                unexpected={k:(value,after.get(k)) for k,value in before.items() if k!=key and after.get(k)!=value}
                if unexpected:raise AssertionError(f'Unrelated fields changed: {unexpected}')
                results.append({'key':key,'before':original,'test':changed,'passed':True})
                print(f'{key}: {original} -> {changed} -> restore',flush=True)
            finally:
                query([key,original])
finally:
    final=query()
    for key,original in before.items():
        if final.get(key)!=original:
            try:query([key,original])
            except Exception as error:errors.append(str(error))
    restored=query()==before
    args.report.write_text(json.dumps({'mode':'roundtrip' if args.apply else 'read-only','values':before,'tests':results,'restored':restored,'restore_errors':errors},ensure_ascii=False,indent=2),encoding='utf-8')
    if not restored or errors:raise RuntimeError('Settings could not be fully restored; see report and baseline')
print(f'{len(results)} roundtrips; baseline restored: {restored}')

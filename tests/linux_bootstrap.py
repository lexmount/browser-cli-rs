#!/usr/bin/env python3
import tempfile,pathlib,subprocess,os,shutil
root=pathlib.Path(__file__).resolve().parents[1] / 'skills/lexmount-browser/scripts'
with tempfile.TemporaryDirectory() as t:
 p=pathlib.Path(t); scripts=p/'skill/scripts';scripts.mkdir(parents=True); fake=p/'path';fake.mkdir();out=p/'install'
 for n in ['bootstrap.sh','doctor.sh']:shutil.copy(root/n,scripts/n)
 (fake/'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
 (fake/'curl').write_text('''#!/bin/sh
while [ "$#" -gt 0 ]; do case "$1" in https:*) url="$1";; -o) shift; output="$1";; esac; shift; done
case "$url" in
 *SHA256SUMS) printf '%s  %s\\n' "$(openssl dgst -sha256 "$FIXTURE" | awk '{print $NF}')" 'browser-cli-v1.3.0-x86_64-unknown-linux-musl' > "$output";;
 *x86_64-unknown-linux-musl) cp "$FIXTURE" "$output";;
 *) exit 9;;
esac
''')
 fixture=p/'binary';fixture.write_text('#!/bin/sh\necho browser-cli-fixture\n')
 for f in fake.iterdir():f.chmod(0o755)
 env=dict(os.environ,PATH=str(fake)+':/usr/bin:/bin',FIXTURE=str(fixture),LEXMOUNT_BROWSER_CLI_INSTALL_DIR=str(out))
 r=subprocess.run(['sh',str(scripts/'bootstrap.sh')],env=env,capture_output=True,text=True);assert r.returncode==0,r.stderr
 assert (out/'browser-cli').exists()
 print('Linux bootstrap selected musl asset, verified checksum, installed and executed fixture: PASS')

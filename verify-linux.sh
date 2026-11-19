#!/bin/sh
set -eu
cd "$(dirname "$0")"
native="${1:-./target/release/pattern-native}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM
uname -smr
node --version

node vendor/draft-toni.mjs baseline "$scratch/toni-960.json"
node vendor/draft-toni.mjs chest-plus-40 "$scratch/toni-1000.json"
"$native" draft-import "$scratch/toni-960.json" "$scratch/toni-960.pattern.json" >"$scratch/toni-audit.json"
"$native" draft-import "$scratch/toni-1000.json" "$scratch/toni-1000.pattern.json" >"$scratch/toni-plus-audit.json"
"$native" audit "$scratch/toni-960.pattern.json" >"$scratch/toni-reopen.json"
cmp "$scratch/toni-audit.json" "$scratch/toni-reopen.json"

"$native" clo-import fixtures/carlton-edges.json fixtures/carlton-edge-map.json fixtures/carlton-shell-seams.json "$scratch/carlton.pattern.json" >"$scratch/carlton-audit.json"
"$native" audit "$scratch/carlton.pattern.json" >"$scratch/carlton-reopen.json"
cmp "$scratch/carlton-audit.json" "$scratch/carlton-reopen.json"
"$native" constraints "$scratch/carlton.pattern.json" >"$scratch/carlton-constraints.json"

node --input-type=module - "$scratch" <<'JS'
import assert from 'node:assert/strict';
import fs from 'node:fs';
const base=process.argv[2];
const read=f=>JSON.parse(fs.readFileSync(base+'/'+f,'utf8'));
const audit=read('carlton-audit.json');
assert.equal(audit.pieces,28);
assert.equal(audit.declared_seams,66);
assert.equal(audit.valid_endpoints,132);
assert.equal(audit.errors.length,0);
assert.ok(audit.segmented_seams>0&&audit.reversed_intervals>0);
const graph = read('carlton.pattern.json');
const constraints = read('carlton-constraints.json');
assert.ok(constraints.length>500);
for(let seam=1;seam<=graph.seams.length;seam++){
  const pairs=constraints.filter(c=>c.seam===seam);
  assert.ok(pairs.length>=2);
  const record=graph.seams[seam-1];
  for (const [key,index] of [['a',0],['b',1]]) {
    const [piece,edge,start,end]=record[index];
    assert.deepEqual(pairs[0][key],{piece,edge,offset:start});
    assert.deepEqual(pairs.at(-1)[key],{piece,edge,offset:end});
    assert.ok(pairs.every(p=>p[key].offset>=Math.min(start,end)-1e-9&&p[key].offset<=Math.max(start,end)+1e-9));
  }
}
console.log('分段缝合采样：'+constraints.length+' 对边参数约束，逐组核对了端点与方向');

const toni=read('toni-960.pattern.json');
const larger=read('toni-1000.pattern.json');
assert.equal(toni.pieces.length,3);
assert.equal(larger.pieces.length,3);
assert.equal(toni.draft_settings.sa,10);
assert.equal(larger.draft_settings.measurements.chest-toni.draft_settings.measurements.chest,40);
assert.ok(toni.pieces.every(p=>p.lines.some(e=>e.role==='sa')));
assert.ok(toni.pieces.every(p=>p.draft_paths.sa.some(op=>op.type==='curve')));
const outline=(p,name)=>p.pieces.find(p=>p.name===name).lines.find(l=>l.role==='outline').length;
const back=[outline(toni,'toni.back'),outline(larger,'toni.back')];
assert.ok(Math.abs(back[1]-back[0])>1);
console.log('Carlton 归一化与重开：',JSON.stringify(audit));
console.log('Toni 量体再出样：三种裁片，10 毫米缝份，胸围 960→1000 毫米，后片净线长度 '+back.map(x=>x.toFixed(2)).join('→')+' 毫米');
const invalid=read('carlton.pattern.json');
invalid.seams[0][0][1]=999999;
fs.writeFileSync(base+'/broken.pattern.json',JSON.stringify(invalid));
JS
if "$native" audit "$scratch/broken.pattern.json" > /dev/null 2>"$scratch/broken.log"; then
  echo '错误边号未被拦截' >&2
  exit 1
fi
grep -q '引用不存在的边' "$scratch/broken.log"
echo '错误边号反例：拒绝通过'

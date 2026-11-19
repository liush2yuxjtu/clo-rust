#!/bin/sh
set -eu
cd "$(dirname "$0")"
native="${1:-./target/release/pattern-native}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM

cat >"$scratch/joined.json" <<'JSON'
{"pieces":3,"seam_groups":1,"piece_groups":[["前片",[0]],["后片",[0]],["口袋",[]]],"garment_groups":{"groups":[{"patternIndices":[2]},{"patternIndices":[1,0]}]}}
JSON
cat >"$scratch/mismatch.json" <<'JSON'
{"pieces":3,"seam_groups":1,"piece_groups":[["前片",[0]],["后片",[0]],["口袋",[]]],"garment_groups":{"groups":[{"patternIndices":[0,1,2]}]}}
JSON
"$native" connectivity "$scratch/joined.json" --strict >"$scratch/joined-report.json"
"$native" connectivity "$scratch/mismatch.json" >"$scratch/mismatch-report.json"
status=0
"$native" connectivity "$scratch/mismatch.json" --strict >"$scratch/strict-report.json" 2>"$scratch/strict-error.log" || status=$?
test "$status" -eq 1
grep -q '缝合连通分组与参照结果不一致' "$scratch/strict-error.log"
cmp "$scratch/mismatch-report.json" "$scratch/strict-report.json"

cat >"$scratch/seam.json" <<'JSON'
{"version":1,"units":"mm","source":"合成缝合样本","pieces":[{"name":"前片","lines":[{"lineIndex":0,"length":20,"start":{"x":0,"y":0},"end":{"x":20,"y":0}}]},{"name":"后片","lines":[{"lineIndex":0,"length":20,"start":{"x":20,"y":0},"end":{"x":0,"y":0}}]}],"seams":[[["前片",0,0.25,0.75],["后片",0,0.75,0.25],10,10]]}
JSON
"$native" constraints "$scratch/seam.json" >"$scratch/constraints.json"

cat >"$scratch/contact.obj" <<'OBJ'
v 0 0 0
v 2 0 0
v 0 2 0
v 0.1 0.1 -1
v 0.1 0.1 1
v 1 1 1
v 0.1 0.1 2
usemtl body
f 1 2 3
usemtl cloth_front
f 4 5 6
f 5 6 7
OBJ
"$native" collision-obj "$scratch/contact.obj" body cloth_ "$scratch/contact.vtk" >"$scratch/contact.json"
node vendor/draft-toni.mjs baseline "$scratch/toni-960-draft.json"
node vendor/draft-toni.mjs chest-plus-40 "$scratch/toni-1000-draft.json"
for size in 960 1000; do
  "$native" draft-import "$scratch/toni-$size-draft.json" "$scratch/toni-$size.json" >"$scratch/toni-$size-import.json"
  "$native" audit "$scratch/toni-$size.json" >"$scratch/toni-$size-reopen.json"
  cmp "$scratch/toni-$size-import.json" "$scratch/toni-$size-reopen.json"
done

node --input-type=module - "$scratch" <<'JS'
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {join} from 'node:path';

const directory = process.argv[2];
const read = name => readFileSync(join(directory, name), 'utf8');
const json = name => JSON.parse(read(name));
const joined = json('joined-report.json');
assert.deepEqual(joined.components, [[0, 1], [2]]);
assert.equal(joined.same_as_clo, true);
assert.equal(json('mismatch-report.json').same_as_clo, false);
console.log('缝合关系：正确分组通过，错误分组在严格模式退出 1，保留 JSON');

const constraints = json('constraints.json');
assert.deepEqual(constraints.map(pair => [pair.a.offset, pair.b.offset]),
  [[0.25, 0.75], [0.5, 0.5], [0.75, 0.25]]);
console.log('布料连接约束：分段反向缝合的三个采样点均正确');

const contact = json('contact.json');
assert.equal(contact.body_triangles, 1);
assert.equal(contact.cloth_triangles, 2);
assert.equal(contact.unique_triangle_pairs, 1);
assert.match(read('contact.vtk'), /^POINTS 1 double$/m);
console.log('人体与布料：两个合成布片三角形仅一个相交，VTK 标记一处');

const toni = [960, 1000].map(size => json(`toni-${size}.json`));
for (const [index, size] of [960, 1000].entries()) {
  assert.equal(toni[index].pieces.length, 3);
  assert.equal(toni[index].draft_settings.measurements.chest, size);
  assert.equal(toni[index].draft_settings.sa, 10);
}
const length = toni.map(project => project.pieces.find(piece => piece.name === 'toni.back')
  .lines.find(edge => edge.role === 'outline').length);
assert.ok(length[1] > length[0] + 1);
console.log(`Toni 出样与重开：胸围 960→1000 毫米，三片，后片净线 ${length.map(value => value.toFixed(2)).join('→')} 毫米`);
JS

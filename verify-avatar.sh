#!/bin/sh
set -eu
cd "$(dirname "$0")"
native="${1:-./target/release/pattern-native}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM
node --input-type=module - "$scratch/avatar.glb" <<'JS'
import fs from 'node:fs';

const chunks = [];
const bufferViews = [];
const accessors = [];
let length = 0;
function accessor(type, componentType, values, count, bounds = {}) {
  const padding = (4 - length % 4) % 4;
  chunks.push(Buffer.alloc(padding));
  length += padding;
  const bytes = Buffer.from(values.buffer, values.byteOffset, values.byteLength);
  const index = accessors.length;
  bufferViews.push({buffer:0, byteOffset:length, byteLength:bytes.length});
  chunks.push(bytes);
  length += bytes.length;
  accessors.push({bufferView:index, componentType, type, count, ...bounds});
  return index;
}
const surface = accessor('VEC3', 5126, new Float32Array([0,0,0,1,0,0,0,1,0]), 3,
  {min:[0,0,0],max:[1,1,0]});
const joints = accessor('VEC4', 5121, new Uint8Array([0,1,0,0,0,1,0,0,0,1,0,0]), 3);
const weights = accessor('VEC4', 5126, new Float32Array([.25,.75,0,0,.25,.75,0,0,.25,.75,0,0]), 3);
const inverse = [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,-.25,0,1];
const binds = accessor('MAT4', 5126, new Float32Array([...inverse,...inverse]), 2);
const cloth = accessor('VEC3', 5126, new Float32Array([.65,.45,.2,.65,.45,.8,.83,.7,.8]), 3,
  {min:[.65,.45,.2], max:[.83,.7,.8]});
const primitive = attributes => ({primitives:[{attributes}]});
const document = {
  asset:{version:'2.0'}, scene:0, scenes:[{nodes:[0]}], bufferViews, accessors,
  nodes:[
    {translation:[.25,0,0], children:[1,2,3,4]},
    {name:'人体', mesh:0, skin:0, translation:[10,20,30]},
    {name:'关节一', translation:[0,.5,.5]},
    {name:'关节二', translation:[.5,.5,.5]},
    {name:'衣片', mesh:1},
  ],
  meshes:[primitive({POSITION:surface,JOINTS_0:joints,WEIGHTS_0:weights}),primitive({POSITION:cloth})],
  skins:[{joints:[2,3],inverseBindMatrices:binds}], buffers:[{byteLength:length}],
};
const json = Buffer.from(JSON.stringify(document));
const aligned = Buffer.concat([json,Buffer.alloc((4-json.length%4)%4,32)]);
const binary = Buffer.concat([...chunks,Buffer.alloc((4-length%4)%4)]);
const header = Buffer.alloc(28);
header.write('glTF',0);
header.writeUInt32LE(2,4);
header.writeUInt32LE(28+aligned.length+binary.length,8);
header.writeUInt32LE(aligned.length,12);
header.write('JSON',16);
header.writeUInt32LE(binary.length,20);
header.writeUInt32LE(0x004e4942,24);
fs.writeFileSync(process.argv[2],Buffer.concat([header.subarray(0,20),aligned,header.subarray(20),binary]));
JS
"$native" avatar-import "$scratch/avatar.glb" 人体 m "$scratch/body.obj" > "$scratch/body.json"
"$native" collision-avatar "$scratch/avatar.glb" 人体 m "$scratch/avatar.glb" '["衣片"]' m "$scratch/contact.vtk" > "$scratch/contact.json"
if "$native" avatar-import "$scratch/avatar.glb" 不存在 m >"$scratch/bad.log" 2>&1; then
  echo '不存在的网格名应该被拒绝' >&2
  exit 1
fi
node --input-type=module - "$scratch" <<'JS'
import assert from 'node:assert/strict';
import fs from 'node:fs';
const dir = process.argv[2];
const file = name => fs.readFileSync(dir+'/'+name,'utf8');
const body = JSON.parse(file('body.json'));
assert.equal(body.skinned_vertices,3);
assert.equal(body.triangles,1);
assert.deepEqual(body.bounds_mm,[[625,250,500],[1625,1250,500]]);
assert.deepEqual(body.joints.map(joint=>joint.position_mm),[[250,500,500],[750,500,500]]);
assert.match(file('body.obj'),/^v 625 250 500$/m);
assert.equal(JSON.parse(file('contact.json')).unique_triangle_pairs,1);
assert.match(file('contact.vtk'),/^POINTS 1 double$/m);
assert.match(file('bad.log'),/活动场景找不到网格/);
console.log('GLB 人台：双关节蒙皮、逆绑定、场景变换、米转毫米、碰撞及错误网格均通过');
JS

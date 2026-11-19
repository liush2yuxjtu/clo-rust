import {Toni} from '@freesewing/toni';
import {writeFile} from 'node:fs/promises';

const mode = process.argv[2];
const output = process.argv[3];
if (!['baseline','chest-plus-40'].includes(mode) || !output) {
  throw new Error('用法：node draft-toni.mjs baseline|chest-plus-40 输出工程');
}
// Example measurements in millimetres for reproducible drafting.
const measurements = {
  head:550, biceps:300, chest:mode === 'baseline' ? 960 : 1000,
  hpsToWaistBack:410, neck:360, shoulderToShoulder:380, shoulderSlope:12,
  waistToArmpit:195, waistToSeat:200, seat:1000, waist:810,
  shoulderToWrist:580, wrist:160,
};
const settings = {measurements, sa:10, options:{sleeveLength:1, draftForHighBust:false}};
const draft = new Toni(settings).draft();
const point = p => ({x:p.x,y:p.y});
const useful = keys => Object.entries(keys).filter(([name]) =>
  /^(outline|mirroredOutline|seam|sa|mirroredSa|hemSa|armhole|sleevecap|shoulder|sideSeam|collar|center|hem|leftEdge|rightEdge|__macro_grainline_grainline_line)$/.test(name));
const pieces = Object.entries(draft.parts[0]).filter(([,p])=>!p.hidden).map(([name,part]) => {
  const draft_paths = {};
  const lines = useful(part.paths).map(([role,path],lineIndex) => {
    draft_paths[role] = path.ops.map(({type,to,cp1,cp2}) => ({
      type, ...(to && {to:point(to)}), ...(cp1 && {cp1:point(cp1)}), ...(cp2 && {cp2:point(cp2)})
    }));
    return { lineIndex, role, length:path.length(), start:point(path.start()), end:point(path.end()) };
  }).filter(edge=>edge.length>1e-6);
  if (!lines.length) throw new Error(`裁片没有可编辑曲线：${name}`);
  return {name,lines,draft_paths};
});
const project = {
  version:1, units:'mm', source:'FreeSewing Toni 4.10.2',
  pieces, seams:[],
  draft_settings:{...settings, measurementsKind:'示例量体数据', mode},
};
await writeFile(output, JSON.stringify(project,null,2)+'\n');

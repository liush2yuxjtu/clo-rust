# CLO Rust / Boat 运行入口与文件交接

## 准备可读输入

- 纸样复核用 Rust 的 `audit`、`constraints` 和 `connectivity --strict`。若有边记录、边映射和原始缝合数据，先用 `clo-import` 归一化再重开。针对仓库支持的 Carlton 或 Toni 参数样例，用固定 FreeSewing 适配器出样，再用 `draft-import` 和 `audit` 核查源草稿与重开结果。
- DXF 和 ZPRJ 需要兼容的转换或求解器导出。当前 Rust CLI 读取已归一化的纸样 JSON、缝合导出和服装 GLB。独立动态试穿、工程写入、换料和新姿势求解需要具备相应能力的求解器结果。只有原始纸样或工程时先做可验证的二维核查，并指出会影响当前判断的缺项。把原生图采集视为证据交接，根据用户授权使用已有求解器来源。
- 每姿势记录工程与模拟版本、模特、面料、扣合状态、视角、网格设置和裁片顺序。使用单一服装网格、原始毫米位置、平面的毫米 UV、Y 向上和 +Z 向前。用对应纸样边长或面积核实 UV。用插件的 `prepare_meshes.py` 准备需要运行的网格。跨姿势比较时核对 UV 和三角面拓扑。只有订单中必要且已授权的文件才传给 Boat；原始工程、人台和贴图可以留在证据来源侧。

## 选择执行环境

- **CLO Rust MCP**：每个要验收的姿势用 `export_fit_maps` 且设置 `require_all_maps: true` 交付合体、应变、应力和折角四类图。另用 `fit_pressure` 登记同一姿势的原生接触压力图，保留方法、单位和色标。需要批量几何复核时用 `review_boat_batch`；需要时用 `verify_seam` 和 `physics_debug_view`。以上在 MCP 所在机器执行。`prewarm_runtime` 可检查配置或下载指定发行版。
- **Boat Linux VM**：优先使用已授权的运行中实例和已安装的 CLO Rust Boat 插件。用插件同级的 `scripts/prepare_meshes.py` 在本机预处理，然后用 `scripts/run_boat.py <sandbox-id> <mode> <input> <output>` 上传清单、执行、取回。已经在 Boat 内可用 `scripts/boat_batch.py <mode> <input> <output>`。有发行版二进制时通过 `--remote-native` 或 `--native` 使用。若需独立新开机器，沿用授权的快照与额度，取回后停止自己创建的实例。
- **批处理模式**：完整评审使用 `fit-export`，为至少两个已核实的姿势在 `maps/<pose-id>/` 附 `fit-front.png`、`fit-back.png`、`stress-front.png`、`stress-back.png`。另外保存并并入每姿势的接触压力图。既有 `review` 和 `seam-walk` 可产生中间证据。批处理退出成功后仍须用本技能的验收脚本逐姿势检查五图。
- **线稿、回放与调试**：用 `compare_line_art` 生成可追溯的并排图。只有带真实时间戳的图像或 GLB 序列才用 `physics_replay`。用 `physics_debug_view` 生成应变、折角和可选同姿势静态相交 VTK。

核对二进制版本及摘要、运行回执、视角和模式标签、裁片数量及三角面分母。实际交付的逐片 JSON、CSV 或 VTK 应能回读对应。折角和应变用统一阈值和色标，同局部跨姿势比较用同一平面掩码；原生压力和应力分开，并保留原图与数值来源。再按报告参考写观察、可能解释和能执行的下一轮复测。

[MCP 安装、环境变量和工具列表](../../../../clo_mcp/README.md)

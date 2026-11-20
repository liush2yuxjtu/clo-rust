# CLO Rust MCP · 缝合与试穿验证

CLO Rust MCP 使用标准输入输出协议。Linux 和 macOS 可运行，分析文件留在 MCP 所在的机器。请用同一服装版型、姿势、项目、坐标系及同一次仿真导出作为证据。

## 配置

在此仓库运行，使用 Python 3.11 或更新版本。

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r clo_mcp/requirements.txt
```

私有发行版需要 GitHub CLI，并使用已有的仓库访问权限。预热当前平台的固定版本，进行 SHA-256 校验及版本检查，并输出可执行文件路径。

```sh
.venv/bin/python -m clo_mcp.server --prewarm
```

默认版本为 `v0.1.0-mcp.2`。用 `CLO_RUST_RELEASE` 指定另一个精确的已发布版本。缓存按版本和平台索引。已有本机构建或 Boat 插件随附二进制时，设置 `CLO_RUST_BINARY` 为可执行文件的绝对路径。源代码构建使用 `cargo build --release --locked`。在私有 Boat 沙箱中，可用插件随附的 Linux 二进制，或使用自己已授权的 GitHub 下载环境。

在 Codex 注册 stdio 服务。从仓库根目录执行以下命令。

```sh
codex mcp add clo-rust -- "$PWD/.venv/bin/python" "$PWD/clo_mcp/stdio.py"
```

如需设置环境变量，在注册命令中加入 `--env NAME=VALUE`，放在 `--` 前。也可以在 MCP 客户端配置中设置同名环境变量。用 `CLO_RUST_OUTPUT_DIR` 指定本机结果目录，服务会为每次执行创建独立子目录。

### 共用 CLO MCP 接口

同一个 stdio 入口提供 48 个 CLO MCP 工具外加 11 个试穿诊断工具。
48 项接口的名称、输入和输出声明取自 clo3d-mcp 提交
`d1af0b151e7f20be205102ffcb39d99f7c42d15d`。契约文件旁附有上游 MIT 许可证。

- `CLO_MCP_BACKEND=rust` 使用本项目的 `pattern-native mcp-compat`，默认启用。
  运行 `cargo build --release --locked` 后把 `CLO_RUST_BINARY` 指向本次构建的
  `target/release/pattern-native`。Rust 工程状态保存在该 MCP 会话的引擎进程内。
- `CLO_MCP_BACKEND=official` 通过 MCP stdio 连接已有的 clo3d-mcp 服务。
  如果它与本服务使用不同的 Python 环境，把 `CLO_MCP_OFFICIAL_COMMAND`
  设为 JSON 数组，例如 `["/path/to/clo3d-mcp/.venv/bin/python","-m","clo3d_mcp.server"]`。
  官方后端的返回内容及错误直接透传。

48 项工具的接口已登记，行为兼容性需要逐项核实。Rust 端当前可从已验证的
`.zprj` 和 `.zpac` 工程中读取纸样轮廓、缝合组及其区间，以及已验证的面料
分配和正面颜色。能转换的完整裁片边会建立 Rust 缝线。替换模式也会载入验证过的
保存姿态网格，并读取可用的 `.zprj` 内嵌静态人台。此类导入会返回 `partial`
及明确的缺项。MCP 的 GLB 导出使用 CLO 实测的毫米坐标。
`avatar_load` 和 `scene_open` 可读取 `.avt` 的静态网格和排布点。可用 `CLO_RUST_AVATAR_DIR`
指定本地人台库。骨骼动作和原生量体校准尚未包含。
`fabric_library` 和 `fabric_apply` 可以读取本地 CLO 面料库。将
`CLO_RUST_FABRIC_DIR` 指向包含 `.zfab` 的 `Fabric` 目录，或通过
`CLO_MCP_ASSETS` 指定其上一级。也可以直接向 `fabric_apply` 传入
`.zfab` 文件路径。Rust 读取文件中的面料名称、正面颜色、贴图和物理参数。
CPU 仿真按裁片局部 X/Y 轴和斜向分别使用拉伸、剪切和弯曲参数。
`.zprj` 和 `.zpac` 中保存的面料物理参数也用于求解。有原生面料密度时，
CPU 根据裁片的静止网格面积和密度分配顶点质量，缺少密度的裁片按 300 g/m² 计算。
旋转后的布纹角、摩擦和 CLO 的材料标定尚未用于求解。贴图通道的显示和导出取决于目标格式。
`simulate` 未指定新粒距时，可从保存的网格和零初速度启动 Rust CPU 求解，
后续调用保留 Rust 计算出的速度。显式指定新粒距会重新划分网格。
CPU 求解器会约束已识别的启用边区间缝线，并报告尚未覆盖的内部线和跨边缝线组。
通过 `pattern_ops` 启用的边界弹力会在 CPU 仿真中按目标长度和相对强度产生约束，裁片的原始 2D 几何保持不变。
启用 `shirring` 后，Rust 对靠近所选边的区域局部细化，细化带的宽度和网格密度由粒距与已有网格间距决定。切换抽褶时，原顶点保留位置和速度，新增顶点由相邻顶点插值。
静态人台使用载入网格阻挡布料顶点穿越。封闭且面朝向一致的网格也会在仿真前校正初始穿插。原生保存姿态里的内嵌人台仅有未摆姿网格，
因此该路径仍使用近似人台碰撞。
使用 `.clorust` 保存 Rust 工程。PNG 和 JPEG 基础贴图可用于 2D 和 3D 快照。
OBJ、GLB 和 glTF 导出会保留已分配的面料底色和基础 PNG 贴图，并支持单色卡的
`colorways=true`。OBJ 会返回同目录下的 MTL 路径，MTL 引用导出时复制的 PNG 文件。
FBX 静态导出使用 FBX 7700、厘米和 Y 轴向上，保留 UV、法线和面料分配。
`include_avatar=true` 会附加已载入的静态人台网格，`colorways=true` 支持单色卡。
底色及法线 PNG 保存在同目录，返回列表包含指定的 FBX 路径。
原生 CLI 和 MCP 的 `scene_open` 支持以替换或追加模式导入静态 FBX，按毫米和 Y 轴向上
读取实例变换、面料、UV 和文件目录内的底色及法线 PNG 或 JPEG。蒙皮、动画、嵌入贴图及纯 WebAssembly 构建的 FBX 导入仍待支持。
GLB 和 glTF 的 `TEXCOORD_0` 是贴图 UV。使用 `scene_open` 打开 OBJ 时，
会保留网格的 UV、MTL 底色与透明度和可用的基础 PNG，并在 3D 快照中绘制导入网格。
在同一会话中以替换模式载入原生工程后，`scene_save` 可保存原格式副本或写回原裁片的名称，
并可从 `.zprj` 提取或保存其中的 `.zpac`。项目可选择保留或生成多视图预览。
通过 `fabric_set_color` 修改已载入的原生面料后，保存时会同步单色卡的记录和底图去色状态。
载入时会恢复归档内可读取的面料贴图。单色卡中已有、独用的 PNG 槽位可用 `fabric_set_texture`
替换后写回。JPEG 输入会转换为 PNG。原生写回需要原文件仍可访问。
新建场景、裁片几何、缝合、换面料和创建原生贴图槽位等可保存为 `.clorust`，
这些修改的原生写回、部分仿真参数和其他公开 MCP 调用仍待兼容。

如需设置环境变量，在注册命令中加入 `--env NAME=VALUE`，放在 `--` 前。也可以在 MCP 客户端配置中设置同名环境变量。用 `CLO_RUST_OUTPUT_DIR` 指定本机结果目录，服务会为每次执行创建独立子目录。

## MCP 工具

| 工具 | 需要的输入与结果 |
| --- | --- |
| `prewarm_runtime` | 下载、校验和预热本机平台的 GitHub Release，或检查指定二进制。返回版本、SHA-256 及路径。设置 `download_release: true` 可在已有本地二进制时显式选择发行版。 |
| `verify_seam` | Rust 审核保存工程或 CLO 边数据。输出边长差、区间错误、连通对照及可选约束 JSON。 |
| `strain` | 从带毫米原始位置及毫米纸样 UV 的 GLB 计算面积加权逐片主工程应变 P95。 |
| `stress` | 归档同姿势原生应力截图、显示单位和色标。可附 `piece_id,location,stress_kpa` CSV。 |
| `fit_pressure` | 归档同姿势原生压力截图、显示单位和色标。可附 `piece_id,location,pressure_kpa` CSV。 |
| `compare_line_art` | 生成同视角设计线稿和成衣截图的并排证据。报告记录姿势与来源，供审阅细节。 |
| `fold_angles` | Rust 计算裁片内相邻面折角、逐片 P95 及可测边数。 |
| `physics_replay` | 播放求解器已录制的 PNG/JPEG 与实测时间戳，生成离线播放、暂停、逐帧页面。用 `mesh_frames` 传入同拓扑 GLB 和时间戳时，生成含应变及折角的三维 ParaView `.vtk.series`。 |
| `physics_debug_view` | 输出带应变、有效性和折角字段的 VTK，可选同姿势人台静态相交 VTK。用 ParaView 打开。 |
| `export_fit_maps` | 复用 `clo-fit-export` 生成试穿图、应变图、应力图及折角图，并返回缺失状态。 |
| `review_boat_batch` | 对原插件准备好的多姿势目录运行现有 Boat 评估，生成离线报告。 |

`stress` 和 `fit_pressure` 会在原生截图缺失时返回 `partial` 或 `unavailable`，空样本保持为空。动态求解、录制、原生应力及试穿压力采集需要 CLO 或其他有相应功能的求解器。半成品姿势或几何相交不能用于推断接触压力和材料应力。

## 接入 CLO Rust Boat

将已安装插件内的 `skills/clo-rust-boat-fit-review/scripts/run_review.py` 和
`skills/clo-fit-export/scripts/export_fit.py` 分别配置到 `CLO_BOAT_REVIEW_SCRIPT`
与 `CLO_FIT_EXPORT_SCRIPT`，再按它们各自夹带的 requirements 安装绘图依赖到 MCP 的 Python 环境。
`review_boat_batch` 使用 0.4.0 或更新版插件的 `--native` 入口。
在 Boat 已安装 MCP 时，在 Boat 内运行服务并传入已准备的本地文件。当 MCP 运行在 Mac 时，
把 Boat 内预热得到的 Linux 二进制路径交给原插件的 `run_boat.py --remote-native`。
按原技能的数据清单传递必要输入，保留案例来源、视角、姿势、单位及裁片 ID。

## 人台尺寸

- `avatar_measure` 从单独的人台三角网格测量 Y 向身高，以及胸、腰、臀三个闭合水平截面的周长。
- `avatar_resize` 输入 `height_mm`、`chest_mm`、`waist_mm`、`hips_mm`。它输出毫米制 OBJ 和校验 JSON，重新从保存的 OBJ 读回量体，四项误差均不超过 0.5 mm 时返回通过。
- 输入可为 OBJ 或指定 `mesh` 名的 GLB。GLB 先应用保存的姿势与蒙皮。用 `units` 指定源单位 `mm`、`cm` 或 `m`。`sections` 的 `hips`、`waist`、`chest` 都是自模型脚底起算的身高比例。预设 0.54、0.62、0.755 适合本例 MakeHuman，使用其他人台或姿势时须现场校核截面，尤其要排除与手臂连通的胸部截面。
- 目标限定在源网格各项量体的 ±20% 内。按截面做连续的几何变形，输出仅保存三角网格和该姿势；人体表面卷尺规则、重新绑定骨架、皮肤材质和 CLO AVS 不属于该输出格式。
- 新命令需要含 `avatar-measure` 和 `avatar-resize` 的 `pattern-native`。开发环境可将 `CLO_RUST_BINARY` 指向当前检出的构建。已运行的 MCP 连接需要重新连接才会刷新工具列表。

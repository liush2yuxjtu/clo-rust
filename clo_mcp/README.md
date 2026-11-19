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

48 项工具的接口已登记，行为兼容性需要逐项核实。Rust 的文件格式、模拟数值、
桌面操作和 API 覆盖取决于实际实现及测试证据。

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

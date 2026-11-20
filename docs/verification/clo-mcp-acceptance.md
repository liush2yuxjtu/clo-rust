# CLO MCP 48 工具验收记录

## 怎么验证

2026-11-20 复跑了以下三项。仓库根目录的完整命令与证据字段见 [`/verify` 技能](../../.agents/skills/verify/SKILL.md#verify-the-48-public-clo-mcp-tools)。

| 检查 | 实际做法 | 复跑结果 |
| --- | --- | --- |
| 工具契约 | `smoke_compat --verify-tool-contracts` 直接读取参考 MCP 与两个适配器的 `tools/list`，逐项核对固定清单中的名称、描述和输入输出结构。 | 三个入口均通过 48 项检查。 |
| Rust 实调 | `sweep_compat` 在临时场景里向每项工具发送真正的 `tools/call`，逐项记录错误标记及内容类型。 | 48／48，缺失、额外调用、MCP 错误均为 0。 |
| 双端行为示例 | `smoke_compat --compare-strapped-draft-preview` 向两端发送相同的 7 组规格，逐字段比较纸样、缝线等返回值，再发送 `{"spec": []}` 并读回最后规格。 | 7 组相同，两端均拒绝无效输入，最后规格保持原值。此结果覆盖列出的起版用例。 |

现场比较还要用同一份工程或素材，验证返回值、状态读回、可重开的文件和错误处理。全量行为验收仍未完成。本轮 `clo_status` 返回参考端 `down`、Rust 端 `up`，依赖原生桥接的现场用例待复测。

## 48 项逐项记录

「本轮」和「历史」表示已有双端证据的具体场景。「待验」列的是仍需要实测的行为。桌面可访问性查询可独立执行，截图需要屏幕上有 CLO 窗口。

| 工具 | 已有证据 | 待验行为 |
| --- | --- | --- |
| `clo_status` | 本轮分别读到原生 `down` 和 Rust `up` | 双端就绪、停桥和重启状态 |
| `clo_launch` | Rust 实调 | 首次、重复启动与原生桥接恢复 |
| `clo_stop_bridge` | Rust 实调和会话重启案例 | 现场停桥后工具错误和场景保留 |
| `clo_run_python` | Rust 纸样查询和输出；调用跨过截止时间时保留已完成写入，脚本停止 | 现场双端执行、异常和超时后的状态 |
| `clo_api_search` | Rust 搜索和目录交叉验证，纸样查询摘要匹配原生目录留档 | 现场双端名称、文档搜索及限制参数 |
| `clo_api_doc` | Rust 短名和限定名返回同一记录 | 双端名称歧义、缺失项和重载 |
| `scene_info` | 历史 Carlita 对照匹配 10 个工程字段 | 新版本相同工程、空场景和切换 |
| `scene_clear` | Rust 两种清场、材料索引重置和旧 UI 快照拒绝，最后规格仍可读 | 双端 `keep_avatar`、材料和缝组读回 |
| `scene_open` | Rust 原生、DXF、JSON 追加及保存姿态，成功打开使旧 UI 快照失效 | 双端替换、追加、损坏文件和场景读回 |
| `scene_save` | Rust 工程保存重开 | 双端原生工程回开、缩略图和失败路径 |
| `scene_export` | Rust 导出实调及 FBX 回开 | 双端格式、单位、材质、色组和人台 |
| `snapshot` | Rust 默认视角、别名与 PNG，末视角与 API 缩略图逐字节一致 | 双端正侧背面、视角顺序和错误 |
| `snapshot_2d` | Rust 图像调用，脚本和高层工具 PNG 一致 | 双端有效图像及图中纸样轮廓 |
| `avatar_list` | 历史 6 个库文件、12 个别名及场景读回对照 | 新版本同一人台库和追加后的清单 |
| `avatar_load` | Rust 原生库别名优先约定目录，追加、替换及脚本删除保留布料姿态 | 双端别名、文件路径、追加、替换及缺失项 |
| `avatar_measurements` | Rust 与原生保存数据对照的独立记录 | 同一现场人台的刷新和缓存策略 |
| `arrangement_points` | 历史 Carlita 110 个名称及顺序对照 | 双端更换人台、多人台和空场景 |
| `pattern_list` | 历史 42 片的轮廓、设置和子形状对照 | 当前版本详略模式、编辑后索引读回 |
| `pattern_info` | 历史 322 条外边、520 条子形状边及缺失裁片对照 | 当前版本编辑后的点、线和索引 |
| `pattern_create` | Rust 实调及新片追加姿态用例 | 双端点型、重名和无效轮廓 |
| `pattern_delete` | Rust 删除后剩余姿态续算 | 双端批量、缺失目标和引用缝组 |
| `pattern_set_arrangement` | Rust 实调及单片重建 | 双端排布位置、偏移及无效点位 |
| `pattern_edit_points` | Rust 字符串坐标、绝对与相对移动、原生留档 API 返回类型和单片重建 | 双端现场坐标和失败后的状态 |
| `pattern_ops` | Rust 复制、镜像、轮廓编辑和边弹力状态保存续算 | 双端支持的操作、依赖场景变化和拒绝路径 |
| `sew` | Rust 新约束和已保存姿态续算 | 双端端点、方向、缝组与失败后的状态 |
| `seams_list` | 历史 240 个有序缝组、42 片成员关系对照；Rust 原生归档缝组与 API / Python 读回一致 | 现场双端新建、删除及编辑读回 |
| `fabric_library` | Rust 实调，有双端库筛选脚本 | 同一库的全量、筛选、空结果和类型错误 |
| `fabric_list` | 历史 4 套材料及 40 项字段对照 | 当前版本赋材、选中项和编辑后读回 |
| `fabric_apply` | Rust 底层添加与赋材分开，重开保留姿态和 GLB 材质 | 双端库别名、选片、赋材后的行为和找不到材料 |
| `fabric_set_color` | Rust 改色区分不透明与薄透面料，原生归档重开验证后置透明度贴图替换 | 双端颜色值、正反面、贴图渲染与无效索引 |
| `fabric_set_texture` | Rust 单通道清除、工程重开、GLB 及错误后状态 | 双端纹理持久化及原生 CLO 回开 |
| `simulate` | Rust 批量 API 与公开工具重划网格一致，粒距 0 沿用已有网格 | 双端步数、粒距、缝合与接触量测 |
| `simulation_reset` | Rust 恢复排布、保留设置 | 同一变形场景的双端重置和读回 |
| `checkpoint_save` | 本轮 Rust 同名保存验证原生源文件 SHA-256 | 双端保存时机、命名和失败路径 |
| `checkpoint_restore` | Rust 原生名称恢复 36 片及姿态，成功恢复使旧 UI 快照失效 | 原生端现场恢复、双端导出与回开 |
| `checkpoint_list` | 本轮双端比较时间排序和共享原生归档 | 现场保存和恢复之后的列表 |
| `garment_plan_preview` | 本轮双端比较版型、领袖、面料、数值、非法输入和无副作用 | 现场人台量体预览 |
| `garment_build` | Rust 实调和构建、检查点、快照用例 | 双端加载人台、替换和追加、缝合和模拟读回 |
| `garment_rebuild` | 本轮 Rust 会话历史用例 | 双端改规格重建、失败后历史和场景 |
| `garment_last_spec` | 本轮双端空会话和预览后不变 | 完成构建和重建之后的双端历史 |
| `clo_api_catalog` | Rust 查询、分页及导出 API 用例；2 个纸样条目匹配原生目录留档 | 双端实际工作流的可调用目录及翻页 |
| `clo_api_describe` | Rust 文档字段和导出 API 用例 | 双端实用类型、枚举、重载及缺失项 |
| `clo_api_call` | Rust 调用、导出回开及相机变更 | 双端读写、参数类型和错误的状态读回 |
| `clo_api_release` | Rust 重复释放 `ImportExportOption` 句柄返回 1 和 0 | 现场双端释放、失效及重新获取 |
| `clo_control_guide` | 本轮双端指南结构和工具路径 | 路径所述的现场操作读回 |
| `clo_ui_state` | 本轮双端节点树、上下界及 6 个非法输入 | 可见窗口与运行中场景、选区和对话框 |
| `clo_ui_action` | 本轮双端过期快照拒绝 | 双端可执行动作和效果读回 |
| `clo_ui_screenshot` | 历史双端 PNG；本轮 Rust PNG，CLO 无可见窗口 | 同一工作流的新版本截图和坐标对应 |

用 [仓库 `/verify`](../../.agents/skills/verify/SKILL.md#verify-the-48-public-clo-mcp-tools) 复跑。最新报告在工作区 `work/acceptance-48/`，包含 `rust-tool-calls.json`、`bridge-status.json` 和各比较命令的 JSON 与日志。Carlita 保存姿态样例的 SHA-256 为 `4ad351eb392d40f22609961534579b65f38205d520837523e2acea6bd8af14b9`。既有结果只用于定位下一次复测。

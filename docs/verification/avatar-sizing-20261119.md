# 人台改尺验收 · 2026-11-19

验收基线为 8303245661d057f05fbd37fadeb3b4ce61e3f180。候选改动涵盖 Rust 网格截面、MCP 调用和 Boat 长袖参考人体适配。

- 本地 /verify 执行 cargo build --release --locked --offline、cargo test --locked --offline 和 sh verify-avatar.sh。发布构建、14 项 Rust 单元测试及 GLB 蒙皮和碰撞回归通过。
- 通过配置的 Python MCP 客户端开启 stdio、列出工具、调用 avatar_measure 和 avatar_resize，再单独读取生成的 OBJ。MakeHuman 最大误差 0.243 mm，Luka 最大误差 0.0152 mm。包含手臂的过宽胸部截面正确拒绝。
- Boat x86_64 编译 Rust 源码，复测同一个输出 OBJ。另用独立的三角面相交与闭环周长算法复测实际碰撞体，最大误差 0.2432 mm。长袖基准与改尺后组各通过 9 帧检查。Luka 的实测覆盖网格改尺，长袖双臂验收使用 MakeHuman。
- **PASS**。本次提交源码和验收记录，发行包沿用原有发布流程。

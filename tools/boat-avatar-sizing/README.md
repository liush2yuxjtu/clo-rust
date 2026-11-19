# 长袖试穿的人台尺寸适配

适配对象是 CLO Rust Boat 已验证长袖配置使用的 MakeHuman 参考网格。源版本以 Boat 独立 DXF 脚本固定的 Pattern Agent 提交为准。复用原仿真的 CLO DXF、缝线、衣片材料坐标和体型未变时的骨架。对新网格和准备种子使用同一个经核实的变形，保留原图的独立人体碰撞、袖管穿着、布料自交、续算与逐帧验收。

在已有的隔离求解器副本上执行 `enable_profile.py ENGINE`。该操作校对待改的源函数内容，部署一次。用 `sized_body.py ORIGINAL_JSON RESIZED_OBJ RESIZE_RECEIPT RESIZED_JSON` 生成新碰撞体，校对原网格拓扑、MCP 导出的摘要以及独立复算的顶点位置。将新 JSON 放在 ENGINE 内，以 `CLO_SIZED_BODY_ASSET` 指定相对路径，然后使用同一款 DXF 和同一姿势求解。

适配沿用已验证参考人体的拓扑和骨架。CLO Luka 的已保存网格已支持 MCP 量体及改尺。替换长袖碰撞体还需闭合人台和同源袖管骨架证据。

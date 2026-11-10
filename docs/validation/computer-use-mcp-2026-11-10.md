# Computer Use MCP 验证记录

日期：2026-11-10。原生验收在提交前工作区完成，协议扩展版本 AUT1/1。提交整理阶段另完成全工作区回归。未发布、未替换安装中的 Removent。

## 已完成的验证

| 层次 | 检查 | 结果 |
| --- | --- | --- |
| 工作区 | `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过。依赖 `block 0.1.6` 有现存 future-incompatibility 提示 |
| 完整 Rust 工作区 | `cargo test --locked --workspace -- --test-threads=1` | 提交前回归 678 通过、21 忽略，共 48 个 suite；GUI opt-in 验收另列于下方 |
| 源码规范 | `scripts/check.sh` | 布局、25 个 build、12 个 release、17 个 relay（1 项跳过）、40 个 macOS 离线测试及 shell 语法通过 |
| 自动化/协议/核心/网络 | 四个包的 library tests | 146 通过，1 项既有忽略测试；覆盖 viewer 与本地接管、持有状态隔离、恢复 revision、监听启动/心跳/禁用、完整事件掩码检查和输入清理；监听失效时仍可读取，但操作和恢复不会进入原生执行器；进程退出或 PID 身份变化后引用失效 |
| 原生输入构造 | `cargo test -p removent-input --lib` | 19 通过；验证键鼠/滚动及释放事件的来源标记、Unicode key-up、双击/拖动/滚动/修饰键构造；不向 OS 派发输入 |
| host 回归 | `cargo test -p removent-host --lib` | 72 通过，2 项既有忽略测试；新增 Caps Lock、修饰键与滚动的持有状态检查 |
| 剪贴板回归 | `cargo test -p removent-client --lib session::tests::clipboard` | 9 通过 |
| RVP 自动化集成 | `session_loopback automation` | 5 通过，5 项原生 GUI 测试默认忽略并单独运行。真实 TLS pin、配对和准入后，验证回执、去重、冲突拒绝、剪贴板并行、慢流头部不阻塞、未授权拒绝、broker 复用、取消传播，以及 viewer 和 host 本地接管通知 |
| MCP 到 RVP | `cargo test -p removent-mcp --test end_to_end` | 1 通过，1 项 GUI 测试默认忽略并单独运行：实际 stdio 子进程主动连接主机，执行模拟原生操作并去重，返回 image content；viewer/本地通知暂停后状态、持有状态和错误正确，显式恢复更新 revision 后可继续；stdin EOF 关闭连接 |
| MCP 到真实原生 UI | `mcp_native_appkit_end_to_end -- --ignored --nocapture --test-threads=1` | 1 通过，2.41 秒：实际 stdio MCP 主动连接经过 pin 的 RVP 测试主机，原生执行器修改 AppKit 控件、选择文本、输入中文/emoji、点击按钮并返回真实 PNG image content；重复 mutation ID 不重复输入或按钮动作，stdin EOF 清理连接 |
| MCP 协商/生命周期 | `scripts/qa/test_mcp.py` | 2025-06-18、2025-11-25 握手及较新 initialize 版本的兼容协商通过；工具清单、错误参数、连接失败和断开通过 |
| 本机原生只读 | `cargo run -p removent-automation --example probe` | 当前进程屏幕录制、辅助功能及输入监控可用；真实 tap 注册并验证完整掩码，状态为 `ready`，`can_mutate: true`。列出 1 个显示器，实际采集 640×360 PNG，并取得前台应用 AX 树 |
| 原生验收入口 | `xcrun swiftc -typecheck scripts/qa/automation_receiver.swift` | 临时 AppKit fixture 编译检查通过；6 个 opt-in Rust 验收入口通过工作区编译检查 |
| 本机原生操作 | `native_automation_rvp -- --ignored --nocapture --test-threads=1` | 5 通过，10.88 秒；真实 TLS/RVP 与 native executor 驱动临时 AppKit 应用，覆盖以下现场验收 |

本机 probe 仅输出数量和图像尺寸，不输出应用名称、元素文本或截图内容。原生读取验证与模拟执行器的输入测试是不同证据层次。

默认接管测试使用延迟释放的模拟执行器，确认原生动作返回清理完成前不派发 viewer 输入；`control_status` 可绕过 broker 动作租约查询。本地接管通过 host 平台通知入口模拟，覆盖正在执行的动作被取消、viewer 松键不能清除本地持有、显式恢复和旧 revision 拒绝。事件解码与来源过滤使用构造的 CGEvent 和元数据，tap 禁用回调覆盖 NULL event，事件掩码检查使用模拟清单。这些默认测试与以下现场验收是不同层次。

## 解锁后现场验收

此前锁屏时前台预检阻止了输入，本次用户要求继续推进后确认桌面已解锁，串行运行临时窗口验收，5 项 RVP 原生测试及 1 项 MCP 原生测试全部通过：

1. AX focus、set value、button press、UTF-16 文本选择及中文/emoji 替换；检查 AppKit 控件实际值及旧引用失效。
2. 真实 PNG 的坐标映射、双击、Shift 右键、拖动、滚动方向和结束阶段；通过 AppKit 事件回执确认 agent key-up 先于 viewer key-down，修饰键已释放，旧截图坐标被拒绝。
3. 从另一个进程派发未标记的 CGEvent，经过真实 OS 事件 tap 触发本地接管。检查 agent 被取消、不会释放外部仍持有的同一个键、viewer 松键不清除本地持有、外部释放后仍须显式恢复。这是外部合成输入，不是物理 HID 设备验收。
4. 临时窗口调用真实系统安全输入 API，检查本次会话变为 `unavailable/secure_input`、操作及恢复被拒绝、读取保留；关闭安全输入后不会自动恢复。fixture 在失去焦点、退出及超时路径释放自己持有的键与安全输入。
5. 同一个无 AppKit 主事件循环的 host 会话内，两次启动并退出新应用，检查清单、前台目标及 AX 引用随生命周期刷新。
6. 真实 stdio MCP 子进程调用 `removent_connect` 与 `computer`，通过真实 RVP 和原生执行器验证 AppKit 文字/按钮实际结果、相同 operation ID 的去重，以及 PNG 从 host 返回为 MCP image content。此项在同一台 Mac 上运行两个协议端点，不能代替双机验收。

现场验收发现并修复两处问题：后台应用清单/前台状态缓存不刷新，以及 automation 滚动量和共用注入器各取反一次导致方向错误。另修正测试等待：`native_api_returned` 不保证 AppKit 已收到 key-up，测量下一段事件前须等待真实回执。

应用清单现在读取同用户的实时进程及静态应用元数据，引用绑定 PID 和进程启动时间。前台 PID 查询隔离使用公开但已弃用的 Process Manager API；后续应在 host 拥有 AppKit 主事件循环时迁移到现代 API。Apple 文档说明 [NSRunningApplication](https://developer.apple.com/documentation/appkit/nsrunningapplication) 的动态属性与 [runningApplications](https://developer.apple.com/documentation/appkit/nsworkspace/runningapplications) 依赖主事件循环，不能将线程安全误认为后台轮询可刷新。

AX 快照保留 CF 对象去重，避免循环引用消耗节点预算。原生验收始终检查目标窗口处于前台，不编辑用户文件；正常关闭后恢复此前焦点和指针。安装中的 Removent 与正式权限设置均未替换或修改。

## 当前边界

- 尚未在两台真实电脑之间验收 agent 闭环，未实测此次 MCP 路径的 QUIC/WSS relay。
- 物理键鼠接管和真实 Input Monitoring 权限撤销仍未验收。OS 注册、外部合成事件和安全输入切换已实测，但不替代物理设备行为。
- 普通键鼠以外的系统媒体键不在当前监听范围内；本地事件不被拦截，已派发的系统调用可能继续完成。后台前台 PID 查询仍有上述已弃用 API 的维护边界。
- 未打包或验证本次签名产物。构建脚本已纳入 `removent-mcp`，源码和离线脚本检查不等同签名、公证或安装验证。
- 浏览器扩展和 Native Messaging host 尚未实现。当前仅明确报告不可用，不注册浏览器工具。
- 桌面会话可供 MCP 借用，RVP viewer 输入与被控端本地监听均接入暂停逻辑；桌面 UI 尚不能附加显示 MCP 创建的会话。完整目标见设计文档，已实现接口见使用指南。

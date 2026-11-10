# Computer Use MCP

这一版本支持 macOS 主控端和被控端。Agent 可以通过 MCP 发起保存的 Removent 连接，观察远端应用、原生元素和屏幕，再执行原生或坐标操作。浏览器 DOM 桥接尚未实现，不提供浏览器工具；`capabilities.browser.ready` 为 `false`。

## 配置

1. 在 Removent 桌面端连接被控电脑，确认主机证书并保存连接。需要密码认证时，在桌面端保存认证凭据。MCP 复用相同数据目录、设备身份和钥匙串，不接受工具参数中的密码或 OTP。
2. 启动 MCP 后调用 `removent_hosts`，取得 `client_fingerprint` 与目标 `saved_id`。也可在相同数据目录下执行 `removent-cli identity` 取得主控证书指纹。
3. 在被控电脑授权该完整的小写证书指纹：

   ```sh
   removent-cli settings set automation_allowed_peers <64位主控证书指纹>
   ```

   多个指纹用英文逗号连接，不含空格。空字符串撤销所有自动化授权。设置通过现有 daemon reload 生效，现有会话可能断开。被控端仍执行正常身份认证与准入，并需要屏幕录制、辅助功能和输入监控权限。登录窗口服务不提供自动化。输入监控只做权限预检，不自动弹出授权请求；授权或恢复后重新建立会话。

源码启动：

```sh
cargo build -p removent-mcp
./target/debug/removent-mcp
```

MCP 客户端配置示例（使用本次代码构建的 app bundle）：

```json
{
  "mcpServers": {
    "removent": {
      "command": "/Applications/Removent.app/Contents/MacOS/removent-mcp"
    }
  }
}
```

直接运行 Cargo 构建的开发版本默认使用仓库 `userdata`；`scripts/dev.sh` 使用 `userdata/dev`，需要给 MCP 设置相同的 `REMOVENT_DATA_DIR`。发布版本使用用户 Library 数据目录。可通过 MCP 配置的 `env.REMOVENT_DATA_DIR` 指定与桌面端一致的目录。stdout 仅用于 MCP 消息。当前 stdio 使用 MCP initialize 生命周期，可协商 2025-06-18、2025-11-25；较新的 initialize 请求按 SDK 兼容规则协商版本。

## 调用流程

| 工具 | 行为 |
| --- | --- |
| `removent_hosts` | 返回保存的原生主机和本机证书指纹，不返回凭据 |
| `removent_connect` | 输入 `saved_id`，立即返回 `session_id` 与 `connecting`；相同目标复用已有条目 |
| `removent_status` | `connecting`、`connected`、`failed` 或 `disconnected`，含错误、活动操作和远端 `control` 状态 |
| `removent_disconnect` | 取消连接/任务并清理条目；借用的桌面连接继续存在 |
| `removent_cancel` | 取消指定 `operation_id`；已发生的输入不回滚 |
| `computer` | 输入 `session_id`、唯一 `operation_id` 和 `command` |

连接就绪后先调用能力查询：

```json
{"session_id":"返回的会话编号","operation_id":"caps-1","command":{"action":"capabilities"}}
```

先确认 `local_input_monitor.state` 为 `ready`，以及 `can_mutate` 为 `true`。监听尚在 `starting` 时稍后重新查询；`unavailable` 时先处理报告的原因并重连，不尝试反复恢复。`native_ui` 仅表示 AX 读取能力，不代表当前允许变更。

再取得显示器和应用清单，并读取目标树或截图：

```json
{"session_id":"返回的会话编号","operation_id":"state-1","command":{"action":"state"}}
{"session_id":"返回的会话编号","operation_id":"tree-1","command":{"action":"snapshot","target":"frontmost"}}
{"session_id":"返回的会话编号","operation_id":"screen-1","command":{"action":"screenshot","display_id":0,"max_width":1280}}
```

`target` 支持 `frontmost`、清单中的 `app:<pid>` 或运行中的 bundle ID。窗口作为 AX 树节点出现。`display_id: 0` 选择主屏，也可使用清单中的显示器 ID。截图通过 MCP image content 返回，尺寸及 observation 在 structured content 中。

`computer.command` 支持：

- `open_app { bundle_id }`、`snapshot { target }`、`state`、`capabilities`。
- `control_status`、`resume_control { revision }`，查询及显式恢复人工接管后的自动化。
- `screenshot { display_id, max_width }`。
- `click { point, button, count, modifiers, hold_ms }`，按钮为 `left`、`right`、`middle`，修饰键为 `shift`、`control`、`alt`、`meta`。
- `move { point }`、`drag { from, to, duration_ms }`、`scroll { point, dx, dy }`。滚动量为 viewport 像素，正 y 向下滚动。
- `press_key { key, hold_ms }`，例如 `Meta+Shift+A`、`Enter`、`ArrowDown`。
- `type_text { text }`，支持 Unicode、换行和 tab，不修改剪贴板。
- `element_action { reference, name }`，动作必须来自当前快照；`focus { reference }`、`set_value { reference, value }`。
- `select_text { reference, text, prefix, suffix, placement }`，placement 为 `select`、`before`、`after`；重复文本须通过上下文消歧。范围按 UTF-16 计算。

所有坐标 point 都是 `{ "observation": 截图返回的编号, "x": 100, "y": 80 }`。不能直接使用屏幕 points、原始 RVP 视频尺寸或其他截图的坐标。显示器布局改变、引用过期时重新观察。每次新 AX 快照替换此前引用；受保护文本不返回值。

变更操作返回 `status: executed` 和 `acknowledgement: native_api_returned`，应继续取截图或树确认实际 UI 结果。未知结果时不得换新 ID 重复点击或输入；同一连接内以同一个 ID、同样参数重取变更结果。断线后需明确断开旧条目再建立新会话，旧元素和观察编号不能复用。

## 会话共享及当前范围

桌面端已连接时，MCP 会通过当前用户专属的本地 socket 借用该连接，不触发远端第二连接的 Busy。socket 校验对等 UID 和远端指纹，借用句柄绑定随机连接 generation；桌面重连后旧句柄失效。MCP 创建的连接也可被另一个 MCP 进程借用，但连接所有者退出会关闭它。每个本地 broker 同时只派发一个普通操作，`control_status` 可在动作进行期间查询。

通过同一 RVP viewer 发来的鼠标、按键或滚动输入会暂停 agent。host 先取消正在执行的变更动作，等待原生执行器释放按键、修饰键和按钮，再派发人工输入。排队中的旧动作及接管前发出的迟到请求会被拒绝。若原生调用在 2 秒内仍未退出，host 关闭会话，不混合派发两路输入；已执行的动作不能回滚。

被控端还会在已授权的原生自动化会话内启动只读事件监听。本地普通按键、修饰键、鼠标和滚动输入会暂停 agent 并取消剩余动作。Removent 注入的事件携带进程内随机标记，避免自身动作和释放事件触发接管；其他未标记的本地自动化输入也按本地接管处理。监听不读取文本，不保存事件历史，不向 MCP 返回按键码或指针位置。

本地物理输入直接由系统派发，监听不会拦截或等待 agent。已进入系统的输入或 AX 调用仍可能完成，不能保证与物理事件完全没有交叠，也不能回滚。清理时会保留监听器已观察到的本地同键、同按钮和修饰键持有状态，避免 agent 的释放事件覆盖用户仍在按住的输入。

`control_status` 和 `removent_status.control` 分别提供 `viewer_input_held`、`local_input_held`，并通过 `last_input_source` 区分 `viewer`、`local` 或 `monitor`。任一来源仍有输入未释放时不能恢复；一方的释放不会清除另一方的状态。

| `local_input_monitor` 状态/原因 | 处理方式 |
| --- | --- |
| `starting` | 正在创建监听并采样当前持有状态，稍后查询 |
| `ready` | 监听运行中，仍须检查控制模式与 `can_resume` |
| `unavailable` / `input_monitoring_permission` | 在被控端授予当前 host 进程输入监控权限，再重连 |
| `unavailable` / `secure_input` | 系统安全输入期间禁止自动化变更；结束安全输入后重连 |
| `unavailable` / `tap_disabled`、`monitor_unresponsive`、`startup_timeout` | 本次监听存在空档，检查被控端状态后重连，不能仅调用恢复 |
| `unavailable` / `tap_mask_incomplete`、`tap_identity_ambiguous`、`tap_inventory_unavailable` | 无法确认完整键鼠监听已注册，检查权限及系统状态后重连 |
| `unavailable` / 其他原因 | 监听启动或运行失败，处理被控端故障后重连 |

监听启动有 2 秒期限；运行时每 100ms 检查权限、安全输入、事件 tap 和心跳。超过 500ms 没有心跳会使控制状态失效并取消活动动作，迟到的心跳不能恢复本会话。监听失效时截图、状态和有权限的 AX 读取仍可调用，变更与恢复返回错误。这里的定时检查不是硬实时保证。

人工输入结束后不会自动恢复。先读取 `control_status`，待 `input_held: false`、`can_resume: true`，再使用该次返回的 revision 显式恢复：

```json
{"session_id":"返回的会话编号","operation_id":"control-1","command":{"action":"control_status"}}
{"session_id":"返回的会话编号","operation_id":"resume-1","command":{"action":"resume_control","revision":2}}
```

示例中的 `2` 必须替换为当前 revision。恢复要求没有未释放的人工输入，且最后一次人工输入距今至少 500ms。若收到 `manual_busy` 或 `control_changed`，重新观察控制状态，不循环抢占正在使用 viewer 的用户。MCP 在成功恢复后记住新 revision；也可在 `computer` 顶层显式传入 `control_revision`。每次接管或恢复都会作废旧截图坐标和 AX 引用，继续操作前重新截图或读取树。相同 operation ID 始终返回原操作结果，不能用它重新执行已被拒绝的动作。

桌面 UI 尚不能附加显示 MCP 先建立的连接。共享视频订阅、窗口独立截图/裁切、图树组合观察、树差量和条件等待仍待实现。当前状态查询只给连接总体阶段，不包含完整认证进度。首次证书确认和一次性凭据输入由桌面端完成，MCP 只连接保存并完成本地信任确认的目标。

原生 UI 是否可操作取决于应用的 Accessibility 支持。AX 快照最多 256 个去重节点、20 层和约 5 秒遍历预算，返回 `truncated`；单次跨进程属性调用也有期限。节点在应用提供时包含 `identifier`，实际操作仍使用带生命周期的 `reference`。快捷键使用 macOS 符号虚拟键映射，Unicode 正文应使用 `type_text`。当前不是完整本机 Computer Use API 的替代实现，也不改变 agent 已有本机工具的控制目标。

## 验证入口

```sh
cargo test -p removent-mcp --test end_to_end
cargo test -p removent-client --test session_loopback automation
cargo test -p removent-client --lib session::tests::clipboard
cargo test -p removent-automation -p removent-proto -p removent-net -p removent-core --lib
python3 scripts/qa/test_mcp.py
cargo run -p removent-automation --example probe
# 仅在已解锁桌面、具备权限且暂不进行其他输入时运行，五个窗口测试串行执行。
cargo test -p removent-client --test session_loopback native_automation_rvp -- --ignored --nocapture --test-threads=1
# 上一条结束后再运行完整 stdio MCP -> RVP -> AppKit 验收。
cargo test -p removent-mcp --test end_to_end mcp_native_appkit_end_to_end -- --ignored --nocapture --test-threads=1
```

前几项覆盖协议、实际 RVP 通道和模拟执行器；`probe` 只读取本机能力、应用数量、截图和前台应用树，不注入输入、不输出画面或树内容。RVP 原生测试编译临时 AppKit 窗口，通过真实 RVP 和原生执行器检查 AX 操作、Unicode 输入、截图坐标、点击/拖动/滚动、人工接管后的释放与引用失效；还覆盖新应用的启动/退出、外部进程合成事件经过真实 OS 监听，以及安全输入启用后阻断变更。外部合成事件不等于物理键鼠。测试窗口关闭后释放自己持有的键及安全输入，恢复此前焦点和指针；前台应用改变时停止输入。锁屏时在创建窗口前失败，不把该测试算作通过。

实际验收状态见 [验证记录](../validation/computer-use-mcp-2026-11-10.md)。真实双机直连、QUIC/WSS relay 仍需要另行验收。

最后一个 MCP 测试主动连接临时 RVP 主机，在同一临时 AppKit fixture 中检查实际文字、按钮结果、重试去重和真实 PNG image content，关闭 stdin 后检查连接退出。两组 GUI 验收需要串行执行，不能同时争用前台或输入。

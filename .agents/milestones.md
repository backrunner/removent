# 里程碑计划

版本：v1.0（2026-08-22） · 上游：[requirements.md](./requirements.md)

排期以"周"为相对单位（单人全职当量；并行度自行折算）。每个里程碑有可运行的验收物，禁止跨级堆功能。

## M0 · 工程地基（第 1 周）

- Cargo workspace 按 architecture.md §2 建齐空 crate + 依赖方向约束（cargo-deny 校验）。
- CI：fmt / clippy(-D warnings) / test / audit / deny（GitHub Actions，见 release.md 流水线雏形）。
- GPUI + gpui-component 跑通 Hello 窗口与暗色主题；`userdata/` 目录解析与 flock 落地。
- **验收**：`cargo run -p removent-app` 出窗口；CI 全绿。

## M1 · 连接层打通（第 2–3 周）→ 对应 FR-01~07

- mDNS 广播/浏览；quinn mutual-TLS endpoint；SPAKE2 配对全流程（含 peers.json 读写）；SessionRequest 准入三模式（含被控端准入弹窗 GPUI prompt，FR-51）。
- CLI 冒烟工具 `removent-cli ping <peer>`：发现→配对→控制流 echo，打印 rtt。
- 断线重连状态机 v1（resume_token 快速恢复路径）。
- **验收**：两台真机完成首次配对 <60s；重复连接免确认；拔网线 30s 内恢复会话。

## M2 · 视频通路（第 4–6 周）→ FR-10/11/17

- SCK 采集（排除自身窗口）→ VT 硬编 HEVC/H264 → RVP 视频流 → VT 硬解 → LatestFrameSlot → GPUI RenderImage 渲染。
- 视口缩放三模式；全屏；关键帧请求；config_changed 热重置解码器。
- 性能埋点：encode_ms/decode_ms/render_fps 进 HUD 雏形。
- **验收**：1080p30 稳定 10 分钟无泄漏（macOS Instruments 无持续增长）；端到端延迟实测 ≤80ms。

## M3 · 输入与剪贴板文本（第 7–8 周）→ FR-16/19/30/31/32/33/34(P0 部分)

- 鼠标/键盘事件采集转发 + CGEvent 注入；逃逸键策略；观察模式；光标同步渲染。
- 剪贴板双向纯文本同步（changeCount 监听、防回环、聚焦拉取策略）。
- 菜单栏常驻入口 v1（FR-50）：独立 Swift tray（RemoventTray）+ removentd daemon——状态图标 + 服务开关 + 会话列表 + 准入审批/PIN 展示。
- **验收**：远端 vim 连续输入无丢键；拖拽选择流畅；双向复制粘贴中文无乱码；无回环风暴。

## M4 · 低延迟音频（第 9 周）→ FR-20/21/22/25

- SCK 系统音频 → Opus 48k/10ms → 抖动缓冲(自适应 20–60ms) → cpal 播放；DTX；音画同步校正。
- 音频开关记忆与会话级音量。
- **验收**：播放音乐场景口型/节拍可感知偏差 ≤±40ms；端到端音频延迟 ≤60ms 实测报告；静音时码率趋零。

## M5 · 自适应与多显示器 + 双向语音（第 10–12 周）→ FR-12/14/23 + NFR-01/03/04 全量达标

- StatsReport 驱动的调档算法落地（滞回、手动档位钳制）；显示器列表与切换；DisplayListUpdate 热处理。
- 双向语音（mic 采集→反向 Opus 流）；权限中途撤销的降级处理。
- **验收**：1440p60 达标；限流到 5Mbps 时自动降档且文本仍可读（1Mbps 极限档验收）；插拔显示器不掉会话。

## M6 · 文件传输与产品化（第 13–16 周）→ FR-04/35/40/41/50/52/53 + 发版就绪

- 双栏文件面板 + 拖拽发送 + 断点续传（cache/transfers）；剪贴板文件引用转传输。
- 富文本/图片剪贴板；launchd 自启与无头值守（removentd LaunchAgent，tray 内 toggle）；被控提示条；信任设备管理 UI。
- 打包：.app bundle、Info.plist 权限文案、hardened runtime、codesign+notarize、GitHub Releases 流水线 + 自动更新（release.md 全流程跑通一次真实发布）。
- i18n 全量、明暗主题切换、设置页收尾。
- **验收**：v0.1.0 公开发布；从 release 页下载安装→配对→完整会话→自动更新升级，全程无文档外操作。

## M7 · Backlog（按需立项）

会话录制、在线聊天、同屏多路视频、相对指针模式、HDR、隐私屏、远程命令级文件管理、AV1 编码、Windows/Linux 移植评估。

---

## 附录 A · 延迟预算表（NFR-01 的分解，M2 起逐项实测）

| 环节 | 预算 | 说明 |
| --- | --- | --- |
| SCK 捕获回调 | ≤16ms | 与帧间隔重合，取帧内偏移均值 |
| 入队+编码 | ≤8ms | Apple Silicon 硬编 RealTime |
| 打包+QUIC 发送 | ≤1ms | LAN 单跳 |
| 网络传输 | ≤1ms | 千兆有线/Wi-Fi6 同广播域 |
| 接收解析+解码 | ≤6ms | VT 硬解 |
| 帧槽交换+GPUI 上传渲染 | ≤16ms | 一个 vsync |
| 视频抖动余量 | ≤8ms | |
| **合计 motion-to-photon** | **≤55ms 典型 / 80ms 上限** | |

音频链路预算（NFR-02）：采集缓冲 10ms + Opus 编码 <2ms + 网络 <2ms + 抖动缓冲 20–40ms + 输出缓冲 10ms ≈ **43–64ms**。

## 附录 B · 里程碑依赖关系

```
M0 ─ M1 ─ M2 ─ M3 ─ M4 ─ M5 ─ M6 ──▶ (v0.1.0)
      │    └───┬───┘
      │     （M3/M4 可双线并行）
      └────────────────────── release.md 流水线自 M0 起渐进搭建，M6 收口
```

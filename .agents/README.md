# Removent 设计文档

Removent 是一个类 NoMachine 的远程桌面工具，**完全用 Rust + GPUI 实现**，客户端与服务端一体。
定位：**仅限局域网使用**（不做公网中继 / 配对码穿透），追求高帧率、低延迟、低带宽下仍清晰。

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [requirements.md](./requirements.md) | 需求文档：目标、功能需求（FR）、非功能需求（NFR）、与 NoMachine/RustDesk 能力对标矩阵、权限矩阵、明确不做的事 |
| [architecture.md](./architecture.md) | 技术架构与模块设计：Cargo workspace 划分、数据流管线、线程模型、进程形态、存储布局 |
| [protocol.md](./protocol.md) | 自定义协议 RVP（Removent Protocol）：发现、配对、会话控制、媒体帧格式、剪贴板、版本演进规则 |
| [research.md](./research.md) | 技术选型调研：各依赖库现状 / license 评估 / 关键 API 细节 / 风险清单 |
| [ui-design.md](./ui-design.md) | UI 设计基调「Quiet Control」：设计原则、设计 token、核心界面规格、组件库选型（gpui-component） |
| [milestones.md](./milestones.md) | 里程碑 M0–M7：范围、验收标准、依赖关系、延迟预算附录 |
| [release.md](./release.md) | 发版渠道（GitHub Releases）：签名公证流水线、latest.json 更新元数据、客户端自动更新状态机 |

## 核心决策一览（Decision Log 摘要）

| # | 决策 | 结论 | 理由 |
| --- | --- | --- | --- |
| D1 | UI 框架 | GPUI + gpui-component | 用户指定 GPUI；gpui-component 提供 60+ 成熟组件与主题系统，Apache-2.0 |
| D2 | 传输层 | QUIC（quinn crate）之上自建 RVP 协议 | 免去手写加密/可靠重传/拥塞控制；多路复用天然隔离控制流与媒体流；LAN 场景延迟极小 |
| D3 | 发现 | mDNS（mdns-sd crate），服务类型 `_removent._udp` | 纯 Rust、无 Bonjour 依赖、跨平台一致 |
| D4 | 视频编码 | VideoToolbox 硬编：HEVC Main 优先，H.264 High 回退 | Apple Silicon 硬件能效最好；HEVC 低码率清晰度显著优于 H.264 |
| D5 | 音频 | ScreenCaptureKit 系统音频采集 + Opus 48kHz/10ms 帧，端到端 ≤60ms | 满足"低延迟透传"；DTX 省带宽 |
| D6 | 输入注入 | CGEventPost（需辅助功能权限） | macOS 唯一通用注入途径；GPUI 侧捕获事件转发 |
| D7 | 序列化 | 控制消息 postcard + 显式版本协商；媒体帧手写二进制头 | 媒体路径零拷贝友好；控制消息紧凑 |
| D8 | 安全 | 双向 TLS（自签证书互 pin）+ SPAKE2 PIN 配对 + 会话准入提示 | 局域网≠可信网络，默认加密与授权 |
| D9 | 用户数据 | 统一存放 `userdata/` 目录（便携式设计） | 见 architecture.md 存储章节；可用 `REMOVENT_DATA_DIR` 覆盖 |
| D10 | 平台 | 首发仅 macOS ≥13.0 (Ventura)，arm64 优先 | SCK 系统音频采集需要 13+；后续再评估 Windows/Linux |
| D11 | License 合规 | 对 RustDesk 做 clean-room：不读其源码，只依据公开行为与通用协议知识自行设计 | RustDesk 为 AGPL-3.0，避免代码污染 |
| D12 | 发版渠道 | GitHub Releases（开源），latest.json + 三重校验自动更新 | 见 release.md |
| D13 | 进程形态 | 服务端独立 daemon（removentd）+ Swift 菜单栏 tray（RemoventTray）管理，UDS+JSON Lines IPC；GPUI app 纯客户端 | 无头值守/开机自启由 daemon 承担；tray 承接准入审批与 PIN 展示；GPUI 无内建 tray |

## 阅读顺序建议

新成员按 requirements → architecture → protocol → ui-design → milestones 顺序阅读；
实现某个具体子系统前先查 research.md 中对应小节确认选型未变。

## 文档约定

- 需求编号 `FR-nn`（功能）/ `NFR-nn`（非功能）/ `A-nn`（架构决策，见各文档内联）。
- 所有常量（端口、服务名、魔数等）以 protocol.md 为唯一权威定义，其他文档引用不再重复给出可变值。

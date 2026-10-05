# UI 设计基调与规范 —「Quiet Control」

版本：v1.3（2026-09-30） · 上游：[requirements.md](requirements.md) · 实现依赖：gpui 0.2.x + gpui-component 0.5.x

## 1. 基调：Quiet Control（安静的控制台）

远程桌面的主角永远是**那块远端画面**。我们的 UI 目标是"平时感觉不到，需要时立刻出现"：
像专业监看设备——信息密度可控、状态一目了然、操作路径极短。

v1.3 按当前 Apple HIG 对齐导航和内容层级：使用系统字体、中性内容表面、明确的侧栏选择状态、
统一控件尺寸和克制的强调色。内容分组不添加装饰性渐变或投影；浮层使用深度表达遮挡关系。
主程序仍由 GPUI 绘制，这些表面不是 AppKit 的原生 Liquid Glass 材质。

### 六条设计原则

1. **内容即王座**：会话态下画面占据 ≥88% 视觉面积；所有 chrome 悬浮、可自动隐藏；任何遮挡都可一键收纳。
2. **一处一义**：每个功能只有一个入口。会话工具栏 ≤7 个图标：全屏 / 显示器 / 音频 / 剪贴板 / 文件 / 画质 / 断开。
3. **键盘操作可发现**：设置用 `Cmd+,`，添加连接用 `Cmd+L`，设备搜索用 `Cmd+F`；按钮保留 Tab 焦点和键盘激活。会话逃逸键 `Ctrl+Cmd+Esc` 永不转发；命令面板尚属规划。
4. **状态可见但不打扰**：延迟/帧率/码率聚合为一个右下角小徽标（绿/黄/红三态），hover 展开 HUD 详情；异常才主动 toast。
5. **保留平台惯例**：跨平台保持功能和语义一致；macOS 的导航、窗口行为、系统字体与控件层级遵循 Apple HIG，不要求不同平台像素级一致。
6. **暗色为默认**：目标用户长时间注视画面，默认深色主题；亮色完整支持、跟随系统切换。

## 2. 设计 Token

### 2.1 颜色（语义化，对齐 gpui-component ThemeColor / shadcn 命名）

| Token | Dark | Light | 用途 |
| --- | --- | --- | --- |
| `background` | `#1E1E20` | `#F5F5F7` | 窗口内容底色 |
| `surface` | `#2A2A2C` | `#F5F5F7` | 次级表面 |
| `sidebar` | `#252527` | `#ECECEE` | 导航区域 |
| `group_box` | `#2C2C2E` | `#FFFFFF` | 内容分组 |
| `overlay` | `#2C2C2EF5` | `#FFFFFFF2` | 浮层表面 |
| `border` | `#FFFFFF16` | `#1C213014` | 低对比分隔线/描边 |
| `text.primary` | `#F5F5F7` | `#202127` | 主文本 |
| `text.secondary` | `#A8ABB3` | `#686B74` | 次级文本 |
| `accent` | `#82B5FF` | `#0865DB` | 焦点环/强调文字 |
| `primary` | `#306ACE` | `#0865DB` | 强调按钮底色，白色文字 |
| `success` | `#30D158` | `#1F7A38` | 已连接/健康（深色=systemGreen；浅色用加深档，兼作小号文字需 ≥4.5:1） |
| `warning` | `#FF9F0A` | `#C93400` | 重连中/降档（同上，浅色加深档） |
| `danger` | `#FF453A` | `#D70015` | 断开/拒绝/高延迟（同上，浅色加深档） |
| `scrim` | `#00000099` | `#00000040` | 全屏模态遮罩（与 overlay 区分，避免浅色下洗白背景） |

健康三态映射：绿=rtt<20ms 且 loss≈0；黄=降档中或 rtt≥20ms；红=重连/卡顿。

### 2.2 字体与栅格
- 栈：系统字体优先（mac: SF Pro），回退 Inter；等宽用 SF Mono/JetBrains Mono（HUD 数字）。
- 字阶：11(辅助)/13(正文)/15(标题小)/20(标题)/28(数字大字)；行高 1.45。
- 间距 4px 栅格；内容分组圆角 10、控件 8、浮层独立控制；图标 16/20/22 按上下文选用，24px SVG 统一 2px 描边并全部内嵌。
- 分段控件只表达相关选项的选择状态，同组等宽且保持同一行；更新渠道标签仅为 `Stable` / `Beta`，说明放在旁边的提示文字。

### 2.3 动效
- 只有两类动画：透明度淡入(120ms) 与位移滑出(160ms, ease-out)；工具条 auto-hide 延迟 800ms 无操作后收起。不做装饰性动画。

## 3. 组件库选型（结论）

**采用 [gpui-component](https://github.com/longbridge/gpui-component)（Apache-2.0，crates.io 0.5.x）**：

- 覆盖本项目所需全部基础件：Button/Input/Switch/Select/Tabs/Tooltip/Toast/Dialog/Prompt/ContextMenu/DockArea/VirtualList/Table/TitleBar/Slider 等 60+；
- 内建 `Theme`/`ThemeColor` 语义 token 系统 → 直接承载 §2.1 的 token 表；
- 内建 rust-i18n 支持 → FR-61 中英双语；
- 生产验证（Longbridge Pro 商用产品）、社区活跃（12k+ stars）；
- 风险与对策：pre-1.0 API 变动 → 锁定 git rev 或精确 semver，升级独立 PR；缺失组件（如频谱/波形）自绘 Element 补齐。

不引入其 webview feature 与 editor 等无关重量组件（tree-sitter 等按 feature 裁剪）。

## 4. 关键界面规格

### 4.1 Home（主窗口 1040×700，最小 860×600，可缩放）
```
┌ Removent ─────────────── [添加连接] [设置] ┐
│ 设备侧栏 228/256px │ 连接入口 / 选中设备详情 │
│ 搜索、已保存、附近 │ 主操作与说明           │
│                   │ 共享本机、权限、指纹   │
│ 本机名称 / 共享状态│                      │
├───────────────────┴──────────────────────┤
│ 连接状态                                  │
└───────────────────────────────────────────┘
```
空态显示“连接到设备”、说明与一个强调的添加连接按钮；下方为共享本机的分组表单，包含
分享开关、权限清单和可复制指纹。缺失权限提供有圆角底色的“打开系统设置”按钮。
设备详情页与设置页使用圆角 10 的中性内容分组和细分隔线，首页不使用宣传式渐变卡片。

### 4.2 Viewer 会话窗
- 默认进入即"沉浸态"：纯画面 + 右下角健康徽标；鼠标移到顶缘唤出悬浮工具条，hover 在工具条上保持不隐藏，移开 800ms 后收起；点击画面其他区域立即收起。
- 对端名由 TitleBar 承担，工具条内不再重复显示，只留图标按钮（当前实现：全屏 / 断开；其余随功能落地补齐）。
- 画面层为常驻深色面，其上的文字/图标用固定浅色，不随主题 token 切换；断线遮罩用 scrim。
- （规划）工具条 ≤7 个图标：全屏 / 显示器 / 音频 / 剪贴板 / 文件 / 画质 / 断开；二级面板：显示器选择（缩略图卡片）、画质（四档位 + 自动开关）、音频（输出音量/双向麦克风开关）、文件传输侧板（DockArea 承载）。
- （规划）观察模式徽标常驻左上（"仅观看"，点击切控制）。
- （规划）重连中：全屏半透明遮罩 + 居中转圈与倒计时文案（protocol §7.4 窗口期），失败才落错误页。

### 4.3 配对弹窗（双端）
弹窗圆角 14。大号 6 位 PIN（28px 等宽数字，分组显示 `123 456`）+ 右对齐按钮组（次按钮在左、主按钮在右）。被控端同款弹窗复用于准入确认（30s 无响应自动拒绝；超时后 daemon 广播 `admissionResolved`，各端弹窗随之关闭）。
（规划）双侧指纹短串并排比对行、「仅本次 / 始终信任」二按钮、300s 倒计时进度条、准入弹窗能力勾选组。

### 4.4 Host 面板（菜单栏展开 + 主窗口设置页）
菜单栏（独立 Swift tray `RemoventTray`，经 UDS IPC 连 removentd）：状态行带图标（绿/灰圆点 + 服务状态文案）、「打开 Removent 主窗口」入口、服务开关、当前会话卡（对方名/时长/编码，带图标）、配对 PIN 展示、开机自启 toggle、打开数据目录。准入/PIN 弹窗由主 app 窗口优先处理——主 app 运行时托盘延迟 8s 兜底（未收到 `admissionResolved` 广播才补弹 NSAlert），主 app 未运行时立即弹。
设置打开后复用主窗口侧栏，显示通用 / 外观 / 安全 / 共享 / 设备发现 / iCloud / 系统；
右侧只呈现当前分类标题和分组表单，底部有“返回设备”。分类切换不清除原有设备选择。
不再用会换行的横向分段按钮承担不同设置分类的导航。iCloud 的云图标与其他符号一起内嵌。

## 5. 文案与 i18n
- 语气：直接、无感叹号；错误文案必须含"下一步动作"（例："屏幕录制权限已关闭 → 打开系统设置"）。
- UI 文案经 rust-i18n key 化，中文与英文同步维护；产品名、协议名和更新渠道名保留原名。

## 6. 当前 HIG 依据与验证范围

2026-09-30 查阅 Apple 在线文档：[Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos)、[Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars)、[Materials](https://developer.apple.com/design/human-interface-guidelines/materials)、[Buttons](https://developer.apple.com/design/human-interface-guidelines/buttons)、[Segmented controls](https://developer.apple.com/design/human-interface-guidelines/segmented-controls)。

导航和控件应与内容形成清晰层级，强调按钮数量保持克制；Liquid Glass 用于导航/控件层，
不应用于整片内容区域。当前 GPUI 实现对齐结构与视觉规范，原生材质和完整辅助功能支持需单独验证，
不将浅色/深色截图或功能测试称为完整 HIG 合规验收。

# Rust 依赖升级与中文排版验证（2026-10-11）

## 范围

升级 14 个 Rust workspace 成员及共享依赖，适配不兼容 API；更新 Cargo.lock。
保留既有网络恢复和设备列表界面改动。依赖版本通过 crates.io 实时查询，采用
稳定发行版；已由上游引入的预发行版本保留其约束，不主动用预发行版替换稳定版。

升级后成功的在线 `cargo upgrade --incompatible allow --ignore-rust-version --dry-run` 审计结果：
80 项已满足最新版本、9 项本地依赖，只有直接使用的 ed25519-dalek 仍有可选主版本升级。
收尾时再次查询 crates.io 遇到响应超时；版本清单依据本轮前一次成功的在线审计和锁文件。

## 主要版本与迁移

| 依赖 | 旧要求 | 当前锁定版本 | 适配 |
| --- | --- | --- | --- |
| GPUI / gpui-component | 0.2 / 0.5 | gpui-pre 0.3.8 / 0.7.1 | 平台入口、焦点、窗口事件、主题、按钮与输入框 API |
| screencapturekit | 0.3 | 11.0.0 | 原生样本缓冲、时间戳、回调和可失败的流初始化 |
| apple-cf / videotoolbox | 0.9 / 0.18 | 0.11.0 / 0.21.0 | 重新应用 iOS SDK 与平台符号限制补丁 |
| cpal | 0.15 | 0.18.2 | 采样率、设备描述及流配置 |
| mdns-sd | 0.13 | 0.21.5 | ResolvedService、ScopedIp；保留 IPv6 scope ID，按 DNS-SD TXT 字节上限截短超长名称 |
| rand | 0.8 | 0.10.3 | 新 RNG trait；设备密钥仍使用 OS 随机源 |
| rcgen / x509-cert | 0.13 / 0.2 | 0.14.10 / 0.3.0 | 签名密钥字段与证书访问器 |
| der / pkcs8 | 0.7 / 0.10 | 0.8.2 / 0.11.0 | PEM/DER trait 版本匹配 |
| totp-rs | 5 | 6.0.0 | Builder、Secret 与 Token API；原认证参数保持一致 |
| reqwest | 0.12 | 0.13.5 | rustls 功能名称；保持证书验证 |
| rust-i18n / toml | 3 / 0.8 | 4.2.4 / 1.1.8 | 新版本序列化与本地化 |
| aes / des / md-5 / sha2 | 0.8 / 0.8 / 0.10 / 0.10 | 0.9.3 / 0.9.0 / 0.11.0 / 0.11.0 | cipher trait、定长块；更新器采用流式哈希 |
| opus / flume / num-bigint | 0.3 / 0.11 / 0.4 | 0.4.0 / 0.12.0 / 0.5.1 | 更新清单与锁文件 |

rubato 的共享版本要求更新到 5.0，目前没有 workspace 成员启用它。
锁文件里仍有旧版本的传递依赖，它们受各上游包的兼容范围约束。
workspace 的 Rust 版本声明提高到 1.92：新版 GPUI 使用 Rust 1.91 稳定的字符边界 API，
Linux 传递依赖 oo7 0.6.0 则声明需要 1.92。审查完整 Cargo metadata 后同步提高声明。
本轮实际编译和测试使用 Rust 1.99，没有单独执行 1.92 的最低版本验证。

### ed25519-dalek 3 的解析阻碍

保留直接依赖 ed25519-dalek 2.2.0。尝试 3.0.0 时，Cargo 无法解析曲线依赖：
最新版 IronRDP 的 sspi 0.21.3 在 Apple 平台精确约束 curve25519-dalek 5.0.0-rc.1
与 ed25519-dalek 3.0.0-rc.1；picky 7.0.0-rc.25 同样精确约束曲线库 5.0.0-rc.1，
而 ed25519-dalek 3.0.0 需要稳定版 5。
不修改第三方密码学库的精确版本约束。待 IronRDP/SSPI/Picky 协调后再升级此项。

## 中文排版与局域网空状态

- GPUI 原生文本测量、字形换行和多行省略应用 Unicode 行断属性。禁止闭标点位于行首、
  开标点位于行尾；包含开括号后空格、引号、成对省略号与破折号。极窄宽度允许不可分
  标点组合溢出，避免通过拆开标点破坏中文排版。
- 不向文案添加硬编码换行或不可见字符。局部 GPUI 补丁的来源、范围和移除条件见
  `vendor/gpui/REMOVENT-PATCH.md`。
- 局域网空状态使用 32×32 图标容器和 20×20 SVG；关闭、发现开启、搜索无结果分别使用
  wifi-off、wifi、search。标题与图标垂直居中，说明另起一块并使用卡片完整宽度。
- 禁则写入根 AGENTS.md 与 UI 设计规范第 5 节，保留中文信息不使用圆点分隔的规范。
- 升级后的输入框程序化赋值不会触发 Change；模拟用户输入的测试改用 replace_all，
  并增加提交前的目标/凭证范围校验，防止旧目标密码被用于新目标。
- macOS 所有 Rust 可执行文件及测试加入系统 Swift runtime 的 rpath，避免新捕获桥接库的
  `libswift_Concurrency.dylib` 加载失败。
- 原生复查补齐 gpui-platform 的 font-kit、Wayland、X11 功能，恢复系统字体与原有平台支持；
  同时适配新版按钮自定义背景默认透明度，保持强调按钮正常状态的实色底色。
- 列表以非选中内容对齐：标题、搜索框、卡片及行图标共用左右 16px 基线；选中底色左右
  各向外扩展 8px。所有交互状态保留同一边框和内边距，文字与图标位置不变。
- 后续协议标签调整：已保存与局域网发现共用胶囊协议标签，地址另行布局，间距 6px。
  地址占剩余宽度并允许省略，协议标签保持完整。桌面构建、Clippy、格式与布局检查通过，
  原生深浅色窗口确认胶囊样式，浅色选中前后尺寸与位置一致。
- 后续发现快捷操作：关闭卡片新增「开启发现」，一次开启 Removent、VNC、RDP 并通过
  既有持久化/实时策略更新流程开始搜索；保存失败保留关闭状态及按钮，成功更新底部状态。
  新增 UI 点击、保存失败与重试回归；`cargo test --locked -p removent-app discovery -- --test-threads=1`
  的 8 项测试通过，桌面构建、Clippy、格式与布局检查通过。原生深浅色卡片复查完成，
  独立数据目录实际点击后发现 3 个设备条目，持久化的三个发现开关均已开启；本机共享及
  VNC 服务保持关闭。测试后停止预览进程并恢复其发现关闭状态。

## 本地验证

- `cargo fmt --all -- --check`、`git diff --check`、`python3 scripts/check_layout.py`。
- `cargo test --locked --workspace -- --test-threads=1`：最新提交前复查通过，37 个测试套件，452 项通过、6 项按既有配置忽略、0 项失败。
- `cargo clippy --locked --workspace --all-targets -- -D warnings`：通过。
- `cargo build --locked --workspace`：通过。
- 最后一次列表和按钮样式调整后重新执行桌面测试：78 项通过、1 项忽略、0 项失败；
  workspace 全目标 Clippy 与桌面构建再次通过。
- 中文回归覆盖 1–240 px、5 组标点文案、测量/字形/多行省略，以及显式换行保留。
- iOS arm64 真机与模拟器 Rust 静态库均编译成功；两个平台的 unsigned Xcode Debug
  构建和最终链接均成功。验证中发现并修复 apple-cf 分布式通知的 iOS 未定义符号。
- Mac 解锁后完成独立数据目录的原生窗口检查：深浅色、常规窗口和全屏、已保存选择、
  本机胶囊标签、标题右侧操作及更多菜单、搜索无结果、发现关闭和开启空状态，亦看到实际
  局域网发现条目。没有发起远程连接或开启共享。
- 最后一次列表调整后复查深浅色内容基线与向外扩展的选中背景；原生截图在交互后出现旧帧，
  通过窗口缩放后重新捕获确认更新布局。未将此截图工具现象归因为产品重绘问题。
- 中文窄宽度由自动化测试覆盖；没有完成原生最小宽度窗口检查。
- 复查结束后停止独立预览进程；生产数据和设置未改动。
- 未执行远程 CI、Windows/Linux 运行验证、物理设备安装或发布。

## 安全扫描与剩余上游告警

`cargo audit --deny warnings` 及使用本轮已更新数据库的 `--no-fetch` 复查均未通过：
1 项 RSA 漏洞和 6 项停止维护告警。
RSA 0.10.0-rc.18 来自 IronRDP 的 SSPI/Picky/WinSCard 依赖链；
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071) 当前没有可升级修复版。
其余为 atomic-polyfill 1.0.3、instant 0.1.13、paste 1.0.15、rustls-pemfile 2.2.0、
rustybuzz 0.20.1、ttf-parser 0.25.1 的维护状态告警；均由上游依赖引入，其中
rustls-pemfile 只用于 IronRDP 测试服务器。未添加忽略规则，也未把升级描述为安全扫描通过。

构建仍报告旧 block 0.1.6 的 future-incompatibility；iOS 上 videotoolbox 的动态
符号封装有三个 unused/dead-code 警告。它们不影响上述编译和最终链接。

## 依据

- [Unicode UAX #14](https://www.unicode.org/reports/tr14/index.html)
- [gpui-component 0.7.1 发行说明](https://github.com/longbridge/gpui-kit/releases/tag/v0.7.1)
- [reqwest 当前文档](https://docs.rs/reqwest/latest/reqwest/)

完整运行日志位于本机 `/tmp/removent-upgrade-*.log`；这些临时文件不随仓库提交。

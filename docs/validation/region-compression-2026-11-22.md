# 分区域压缩实验：同带宽下的中文文字与运动画面

发现的首秒补块突发已在后续原型中修复。后续报告包含相同输入和链路下的逐帧对照、实际 TCP 回环以及文字恢复时间。[视频优先的补块调度](region-pacing-2026-11-22.md)

在 720p30 的合成编辑器旁播放小窗动画时，稳定文字块经无损修复后，更接近源像素。与实际总字节数匹配的纯 HEVC 相比，文字区域 PSNR 从 38.74 提高到 43.07 dB，运动区从 31.81 降到 31.62 dB。抽查的 15 个文字样本，两种输出的 OCR 逐项相同。补块集中在启动阶段，限速模型显示它会延迟视频交付。目前保留原生单路视频，用离线实验继续评估有带宽预算的文字修复。

## 采集端现在可以测什么

macOS 使用 ScreenCaptureKit 的 Host 可设置 `REMOVENT_REGION_DIAGNOSTICS=1`，在 info 日志中查看 `capture region diagnostics`。每个采集回调在覆盖旧帧前，把变化矩形映射到 64×64 像素网格。同一样本内重叠的矩形只计一次。每个五秒窗口输出有变化的格数、每秒至少变化四次的格数、最活跃的格、缺少可用元数据的帧数及分析耗时。空闲回调单独计数，分辨率或采集代次变化时重新统计。默认关闭时跳过元数据读取；开启统计也无需读回屏幕像素。[采集接线](../../packages/media-capture/src/sck.rs) [网格统计](../../packages/media-capture/src/sck/regions.rs)

诊断测量的是系统报告的重绘范围。离线原型则逐块比较 BGRA 像素来决定是否发送补块。两者独立，可以分别检查采集端的变化提示和编码后的实际收益。

## 实验条件

- Apple M5 Max，macOS 27.2.1，release 构建，使用项目的 VideoToolbox HEVC 编码器、解码器和现有量化上限。每段 1280×720、30 fps、120 个源时间点（4 秒）。输入为合成内容。
- 同一确定性序列运行三次策略。baseline 使用目标 2,000 kbps 的纯 HEVC，hybrid 在相同设置的视频上叠加补块，matched 单独调节纯 HEVC 的目标码率，尽可能匹配 hybrid 实际产生的总字节。码率按源时间计算，包含视频负载及每帧 27 字节 RVP 头、完整补块批次，不含网络协议开销。
- 每个 64×64 块连续四次观测像素完全相同后，检测亮度锐边。每个时间点至多新增八块，使用 zlib 压缩整批 BGRA 像素和操作记录，并带 24 字节批头。收端验证视频引用和时间戳，解压后合成到显示副本。块内容变化时立即撤回旧补块。全屏运动时缓存为空，可直接使用视频解码输出。[编码基准](../../packages/media-codec/examples/region_compression.rs) [补块收发](../../packages/media-codec/examples/region_compression/tiles.rs)
- 补块选择只看像素。用于评分的文字区和运动区由夹具独立标注。混合场景的小窗占 20% 面积，第 24 帧（从 0 起算）替换一行中文。其他场景为全屏文稿滚动、全屏视差动画和全静止文稿。每个源时间点都对当时显示缓冲中的图像评分，包括沿用上一帧的情况。[实验输入](../../packages/media-codec/examples/region_compression/fixture.rs)

## 四秒结果

| 场景 | 纯 HEVC 实际 kbps | 加补块实际 kbps | 匹配对照实际 kbps | 实验观测 |
| --- | ---: | ---: | ---: | --- |
| 编辑器加小窗 | 1,540 | 2,075 | 2,069 | 安装 168 块，撤回 6 块 |
| 全屏文字滚动 | 1,628 | 1,628 | 1,628 | 无补块，输出像素和视频字节一致 |
| 全屏游戏样式动画 | 1,466 | 1,466 | 1,466 | 无补块，输出像素和视频字节一致 |
| 全静止文稿 | 270 | 922 | 739 | 视频只发首帧，实验时钟另触发 26 次补块更新；纯 HEVC 未匹配到同样总量 |

混合场景两组等量对照的总量相差 0.32%。文字区域的横向亮度边缘像素上，RGB 通道平均绝对误差从 2.891 降到 0.719（通道范围 0–255）。此指标与选块都使用亮度边缘，另用文字区域全像素 PSNR 和独立 OCR 交叉检查。第 23、24、119 帧各取五行，包括被替换的中文行和 10/12 px 小字。原图和三组输出对人工转录均为 427 字符中 7 个编辑，识别文本逐样本一致。第 24 帧在三组解码结果中都能读到新句。[原始数据](../benchmarks/2026-11-22-region-compression/codec.csv) [OCR 明细](../benchmarks/2026-11-22-region-compression/ocr.json) [原像素裁切对照](../benchmarks/2026-11-22-region-compression/mixed-text-crops.png)

同样的基础视频加上 267,670 字节补块，四秒内多用了 34.8% 的字节。这些补块全部集中在首秒。把同一场景延长到 20 秒，文字只在原来的时间编辑一次，补块字节仍为 267,670，相对基础视频的额外开销降到 6.2%。期间持续有小窗运动。该比例取决于静态区域保持多久及编辑频率。[逐帧字节记录](../benchmarks/2026-11-22-region-compression/mixed-by-frame.csv) [20 秒对照](../benchmarks/2026-11-22-region-compression/mixed-20s.csv)

## 时延与验收边界

以 2,000 kbps 恒定有效容量、单条先进先出队列回放逐帧字节记录，假设各批数据在源时间戳立即入队。混合场景从入队到该次更新全部字节出队的 p95，基础 HEVC 为 777 ms，加补块为 1,404 ms，字节匹配的 HEVC 为 1,111 ms。最大的单批补块是 15,865 字节，单独占用该链路约 63 ms。这个模型包含首帧突发，省略了 RTT、丢包、真实拥塞控制和主机丢弃过时帧的行为。它说明按格数限制补块仍会产生突发。下一轮应使用实时可用字节预算，在视频积压时推迟可选修复，并保证旧块撤回和新视频同步生效。[限速回放](../../scripts/fixtures/region_delivery.py) [计算结果](../benchmarks/2026-11-22-region-compression/delivery-model.csv)

另做三次混合场景短测，关闭回放写盘。全部字节数和质量指标复现。串行编解码及合成耗时的 p95 取三次中位数，补块组 5.325 ms，等字节纯 HEVC 组 4.806 ms。计时排除生成夹具和图像评分，系统同时存在其他负载，单次尾部耗时会波动。该原型使用 CPU 可访问的 BGRA，生产原生 surface 的 GPU 分析、上传和合成成本仍待测。全静止场景由逻辑时钟继续观察最后画面，实际 Host 需要独立的缓存刷新调度才能在 ScreenCaptureKit 空闲时完成这些更新。[三次复测](../benchmarks/2026-11-22-region-compression/mixed-repeat.csv)

本机回放器按源时间戳把真实解码结果送入 Metal 窗口。本轮 macOS 会话锁屏，混合样本的 120 个 drawable 均无有效呈现时间，因此显示间隔 p95/p99 留空。跨设备、QUIC 传输及真实游戏输入到显示的延迟也待测。[显示回放器](../../scripts/fixtures/region_replay.swift) [本机回放记录](../benchmarks/2026-11-22-region-compression/presentation.jsonl)

## 复现

在 macOS 仓库根目录执行。回放导出会产生数 GB 原始 BGRA，可省略环境变量只生成编码和质量指标。OCR 脚本使用 Pillow 与系统 Vision，显示回放需在可见的图形会话中运行。

```sh
rtk cargo build --locked --release -p removent-media-codec --example region_compression
REMOVENT_REGION_REPLAY_DIR=target/region-experiment/replay rtk proxy target/release/examples/region_compression all 2000 120 30 1280 720
rtk proxy target/release/examples/region_compression mixed 2000 600 30 1280 720
rtk proxy python3 scripts/qa/region_compression_ocr.py target/region-experiment/replay target/region-experiment/ocr
rtk proxy python3 scripts/fixtures/region_delivery.py 2000 target/region-experiment/replay/*.csv
rtk swiftc -O scripts/fixtures/region_replay.swift -o /tmp/removent-region-replay
rtk proxy /tmp/removent-region-replay 1280 720 target/region-experiment/replay/mixed-hybrid.bgra
```

# image_compresser

用 Rust 编写的批量图片压缩器：递归扫描**输入目录**，把受支持的图片统一转码为 **WebP**（有损，可调质量），并输出到镜像输入目录结构的**输出目录**。

产物文件名保留原文件名与扩展名，再拼接 `.webp`，例如：

```
输入目录/a/b/photo.png  →  输出目录/a/b/photo.png.webp
```

## 特性

- 目录结构镜像：输出目录完整复刻输入目录的相对路径
- 统一输出 WebP，质量 1-100 可调（对应 `thumbQuality`，默认 80）
- 可选"仅当原图更宽才等比缩小"（`--resize` + `--width`，阈值默认 512）
- GIF 取**首帧**转静态 WebP
- 按扩展名白名单识别；扩展名与内容不符时自动按文件内容签名回退识别
- 解码默认不设内存上限（适配超大素材）；扩展名"损坏但可被其它解码器解析"的文件也能转码
- 并发 worker（默认按 CPU 核数）；进度按批输出，可安全中断续跑（默认跳过已存在产物，`--overwrite` 覆盖）
- 输入输出为同一目录时报错；输出目录位于输入目录内部时自动排除，避免递归重复转码
- 损坏/不支持文件不会中断整体，汇总统计并计入退出码
- 支持 `--output-format json` 输出 NDJSON（每行一个事件），便于脚本/上游程序消费

## 编译与运行

需要 Rust 工具链（cargo）。

```bash
# 编译
cargo build --release

# 直接运行
cargo run --release -- -i 输入目录 -o 输出目录

# 产物位于
target/release/image_compresser.exe
```

## 用法

```bash
image_compresser -i <输入目录> -o <输出目录> [选项]
```

示例：

```bash
# 最简：全部转 WebP，质量 80
image_compresser -i ./pics -o ./pics_webp

# 超过 512px 宽的等比缩到 512，质量 75，8 个并发
image_compresser -i ./pics -o ./pics_webp --resize --width 512 -q 75 -j 8

# 已存在也重新生成、关闭进度条、看每步日志
image_compresser -i ./pics -o ./pics_webp --overwrite --batch 0 -v

# 只看会转哪些，不写盘
image_compresser -i ./pics -o ./pics_webp --dry-run

# 机器可读：stdout 只输出 NDJSON 事件
image_compresser -i ./pics -o ./pics_webp --output-format json
```

## 全部参数

| 参数 | 含义 | 默认 | 说明 |
|---|---|---|---|
| `-i, --input <DIR>` | 输入目录 | 必填 | 递归扫描；必须存在且为目录 |
| `-o, --output <DIR>` | 输出目录 | 必填 | 镜像输入目录结构；不存在会自动创建 |
| `--resize` | 启用按宽度缩放 | 关闭 | 仅当原图宽度**超过** `--width` 时才等比缩小；不超过则原尺寸输出 |
| `--width <px>` | 缩放宽度阈值 | `512` | 仅在 `--resize` 时生效 |
| `-q, --quality <1-100>` | WebP 编码质量 | `80` | 越低体积越小、画质越差 |
| `-j, --workers <N>` | 并发 worker 数 | `0`=自动 | `0` 按 CPU 核数；超大图多、内存紧张时建议调小（如 `4`） |
| `--batch <N>` | 进度日志分批条数(仅 text) | `100` | 每处理 N 条打印一次 `[进度]`；`0`=关闭 |
| `--overwrite` | 覆盖已存在的目标文件 | 关闭 | 默认跳过已存在产物（幂等、可中断续跑） |
| `--dry-run` | 只打印待转换清单 | 关闭 | 不创建目录、不写文件 |
| `--output-format <text\|json>` | 输出格式 | `text` | `text`=人类可读；`json`=每行一个 JSON 事件（NDJSON，见下节） |
| `-v, --verbose` | 每步详细信息 | 关闭 | 仅 `text` 模式生效；打印到 stderr |
| `-h, --help` | 帮助 | — | |
| `-V, --version` | 版本号 | — | |

## 支持的输入格式（扩展名白名单）

```
png, jpg, jpeg, gif, bmp, tif, tiff, ico, tga,
pnm, pbm, pgm, ppm, pam, qoi, hdr, dds, webp
```

- 白名单之外的扩展名：**不转换**，计入"跳过(非图片扩展)"，可配合 `-v` 查看明细
- `.webp` 输入同样会重新编码，产物为 `xxx.webp.webp`
- GIF：只取**首帧**，输出为静态 WebP
- 扩展名与内容不符（例如 JPEG 内容存成 `.png`）：先按扩展名解码，失败后按文件内容签名重试
- 内容损坏无法解码：计入"失败"，退出码非 0，但不中断整体

## 输出与汇总

每次运行打印：输入/输出目录、当前设置、扫描统计、汇总（成功/跳过/失败数量、原始 vs WebP 体积变化、用时）。失败明细与 `-v` 的逐条日志输出到 stderr。

### 退出码

| 码 | 含义 |
|---|---|
| `0` | 正常结束（可能有跳过，但没有失败项） |
| `1` | 存在解码/编码/写入失败的文件 |
| `2` | 参数或运行期错误（如输入目录不存在、输入输出相同） |

## 参数与参考文档对照

本工具为命令行精简实现，对应参考文档中的相关字段如下（应用环境相关的 `thumbRestart`/`cacheUseSubprocess`/`gifRealtime`/worker 重启等概念已按约定精简舍弃）：

| 本工具 | 参考字段 |
|---|---|
| `--width` | `thumbWidth`（默认 512） |
| `-q/--quality` | `thumbQuality`（默认 80） |
| `-j/--workers` | `thumbWorkers`（`0`=自动） |
| `--batch` | `scanBatch`/`thumbBatch`（默认 100，本工具仅用于进度分批打印） |
| `--overwrite`/默认跳过 | 输出已存在策略（幂等续跑） |
| `--resize` | 缩放开关（仅超阈值才缩小） |

## JSON 输出（NDJSON，`--output-format json`）

该模式下 **stdout 只输出 NDJSON**（每行一个事件），便于管道/脚本逐行消费；人类可读的汇总、进度、`-v` 明细均不进入 stdout。`rel` 用 `/` 分隔、含 JSON 转义；`ok:true` 行的 `width/height` 为**产物(缩放后)**尺寸。

事件覆盖**每个扫描到的文件**，因此可严格对账：`success + skipped + failed == scan.total`。

### 事件

扫描结束（开始干活前）：

```json
{"event":"scan","total":9888}
```

每处理一个文件（成功带产物宽高；跳过/失败用 `reason`）：

```json
{"event":"file","ok":true,"rel":"sub/a.png","width":3648,"height":5472}
{"event":"file","ok":false,"rel":"sub/a.png","reason":"decode"}
{"event":"file","ok":false,"rel":"readme.txt","reason":"unsupported"}
{"event":"file","ok":false,"rel":"sub/a.png","reason":"exists"}
```

结束汇总：

```json
{"event":"done","success":9800,"skipped":50,"failed":38}
```

### `reason` 取值

| 值 | 含义 | 计入 |
|---|---|---|
| `read` | 源文件读取失败 | failed |
| `decode` | 解码失败（含损坏/无法解析） | failed |
| `encode` | WebP 编码失败 | failed |
| `write` | 写入输出失败 | failed |
| `exists` | 输出已存在且未开 `--overwrite` | skipped |
| `unsupported` | 扩展名不在白名单，不转换 | skipped |

`--dry-run` + `--output-format json`：同样输出 `scan`/逐文件/`done`，可转换的文件为 `ok:true`（不写盘、无 `width/height`），用于预览。

## 测试

```bash
cargo test
```

覆盖：目录镜像结构与产物命名、重跑跳过/`--overwrite`、GIF 首帧尺寸、`--resize` 缩放与不缩放路径、损坏文件退出码、`--dry-run` 不写盘、输出目录嵌套时防递归、扩展名与内容不符回退识别、JSON 输出（NDJSON 对账/坏文件 reason 与退出码/JSON dry-run）。

## 注意事项

- 解码不设内存上限：超大 PNG 单张可能占用很大内存，且按 `-j` 并发叠加，内存紧张时降低 worker 数
- 有损 WebP 转码不可逆；如需保留原图请保留输入目录

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use clap::{Parser, ValueEnum};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;

mod convert;
mod scan;

use convert::{ConvConfig, FailReason, Status};
use scan::{Entry, Kind, ScanCounts};

#[derive(Parser, Debug)]
#[command(
    name = "image_compresser",
    version,
    about = "递归扫描输入目录，把支持的图片(png/jpg/gif/webp/bmp/tiff 等)转码为 WebP，\
             输出目录镜像输入目录结构，产物文件名保留原扩展名并追加 .webp，如 xxx.png.webp"
)]
struct Cli {
    #[arg(short, long, value_name = "DIR")]
    input: PathBuf,

    #[arg(short, long, value_name = "DIR")]
    output: PathBuf,

    #[arg(long, help = "启用按宽度缩放：仅当原图宽度超过 --width 时才等比缩小")]
    resize: bool,

    #[arg(long, default_value_t = 512, value_parser = clap::value_parser!(u32).range(1..), help = "缩放宽度阈值(px)，仅在 --resize 时生效")]
    width: u32,

    #[arg(short = 'q', long, default_value_t = 80, value_parser = clap::value_parser!(u8).range(1..=100), help = "WebP 编码质量 1-100")]
    quality: u8,

    #[arg(
        short = 'j',
        long,
        default_value_t = 0,
        help = "并发 worker 数，0=自动(按 CPU 核数)"
    )]
    workers: usize,

    #[arg(
        long,
        default_value_t = 100,
        help = "进度日志分批条数(仅 text 模式)，0=关闭进度输出"
    )]
    batch: u64,

    #[arg(long, help = "输出文件已存在时覆盖；默认跳过")]
    overwrite: bool,

    #[arg(long, help = "仅扫描并打印将要转换的文件，不写入")]
    dry_run: bool,

    #[arg(short, long, help = "输出每步详细信息(仅 text 模式)")]
    verbose: bool,

    #[arg(
        long,
        value_enum,
        default_value = "text",
        help = "输出格式：text=人类可读；json=每行一个 JSON 事件(NDJSON)"
    )]
    output_format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

struct Summary {
    scan: ScanCounts,
    done: u64,
    skipped_exists: u64,
    failed: u64,
    in_bytes: u64,
    out_bytes: u64,
    elapsed_ms: u128,
}

enum Task {
    Done {
        width: u32,
        height: u32,
        in_bytes: u64,
        out_bytes: u64,
    },
    Exists,
    Unsupported,
    Failed {
        reason: FailReason,
        err: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("错误: {msg}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<u8, String> {
    let input = fs::canonicalize(&cli.input)
        .map_err(|_| format!("输入目录不存在或无法访问: {}", cli.input.display()))?;
    if !input.is_dir() {
        return Err(format!("输入路径不是目录: {}", input.display()));
    }

    let output_root = cli.output.clone();
    let out_canon = fs::canonicalize(&output_root).ok();
    if out_canon.as_deref() == Some(input.as_path()) {
        return Err("输入目录与输出目录不能相同".into());
    }
    let exclude = out_canon.as_deref().filter(|c| c.starts_with(&input));

    let (entries, scan_counts) = scan::scan(&input, &output_root, exclude, cli.verbose);

    let workers = if cli.workers > 0 {
        cli.workers
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    };
    let json = cli.output_format == OutputFormat::Json;

    if scan_counts.walk_errors > 0 {
        eprintln!("警告: 目录遍历失败 {} 处", scan_counts.walk_errors);
    }

    let cfg = ConvConfig {
        resize: cli.resize,
        width: cli.width,
        quality: cli.quality,
        overwrite: cli.overwrite,
    };
    let image_count = entries.iter().filter(|e| e.kind == Kind::Image).count();

    if cli.dry_run {
        if json {
            emit_scan(scan_counts.total_files);
            dry_run_json(&entries, cli.overwrite);
        } else {
            dry_run_text(&entries, cli.overwrite);
        }
        return Ok(0);
    }

    if !json {
        println!("输入目录: {}", input.display());
        println!("输出目录: {}", output_root.display());
        println!(
            "设置: 质量={} | resize={}(阈值{}px) | workers={} | 进度批次={} | overwrite={}",
            cli.quality, cli.resize, cli.width, workers, cli.batch, cli.overwrite
        );
        println!(
            "扫描: 共 {} 个文件, 支持格式 {} 个(扩展名: {}), 跳过(非图片扩展) {} 个",
            scan_counts.total_files,
            scan_counts.supported,
            scan::supported_ext_list(),
            scan_counts.skipped_ext
        );
    }

    if image_count == 0 {
        if !json {
            println!("没有可转换的文件");
        }
        if json {
            emit_scan(scan_counts.total_files);
            for e in &entries {
                emit_json_skip(e, "unsupported");
            }
            emit_done(0, entries.len() as u64, 0);
        }
        return Ok(0);
    }

    let pool = ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(|e| format!("创建 worker 线程池失败: {e}"))?;

    let start = Instant::now();
    if json {
        emit_scan(scan_counts.total_files);
    }
    let processed = AtomicU64::new(0);
    let total_attempt = image_count as u64;
    let verbose = cli.verbose;
    let batch = cli.batch;

    let results: Vec<Task> = pool.install(|| {
        entries
            .par_iter()
            .map(|e: &Entry| {
                let task = process_entry(e, &cfg);
                if json {
                    emit_json_task(e, &task);
                } else {
                    emit_text_task(e, &task, verbose);
                    if !matches!(task, Task::Unsupported) {
                        let cur = processed.fetch_add(1, Ordering::Relaxed) + 1;
                        if batch > 0 && cur.is_multiple_of(batch) {
                            eprintln!("[进度] 已处理 {cur}/{total_attempt}");
                        }
                    }
                }
                task
            })
            .collect()
    });

    let mut done = 0u64;
    let mut skipped_exists = 0u64;
    let mut skipped_unsupported = 0u64;
    let mut failed = 0u64;
    let mut in_bytes = 0u64;
    let mut out_bytes = 0u64;
    for task in results {
        match task {
            Task::Done {
                width: _,
                height: _,
                in_bytes: i,
                out_bytes: o,
            } => {
                done += 1;
                in_bytes += i;
                out_bytes += o;
            }
            Task::Exists => skipped_exists += 1,
            Task::Unsupported => skipped_unsupported += 1,
            Task::Failed { .. } => failed += 1,
        }
    }

    if json {
        emit_done(done, skipped_exists + skipped_unsupported, failed);
    } else {
        let summary = Summary {
            scan: scan_counts,
            done,
            skipped_exists,
            failed,
            in_bytes,
            out_bytes,
            elapsed_ms: start.elapsed().as_millis(),
        };
        print_summary(&summary);
        if done > 0 {
            println!("输出目录: {}", output_root.display());
        }
    }

    Ok(if failed > 0 { 1 } else { 0 })
}

fn process_entry(e: &Entry, cfg: &ConvConfig) -> Task {
    match e.kind {
        Kind::Unsupported => Task::Unsupported,
        Kind::Image => {
            let out = e.out.as_ref().expect("image entry has output path");
            let r = convert::convert_one(&e.src, out, cfg);
            match r.status {
                Status::Done => Task::Done {
                    width: r.width,
                    height: r.height,
                    in_bytes: r.in_bytes,
                    out_bytes: r.out_bytes,
                },
                Status::Exists => Task::Exists,
                Status::Failed => Task::Failed {
                    reason: r.fail_reason.unwrap_or(FailReason::Decode),
                    err: r.err.unwrap_or_default(),
                },
            }
        }
    }
}

fn dry_run_text(entries: &[Entry], overwrite: bool) {
    let images: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Image).collect();
    if images.is_empty() {
        println!("没有可转换的文件");
        return;
    }
    for e in images {
        let out = e.out.as_ref().unwrap();
        if !overwrite && out.exists() {
            println!("[将跳过-已存在] {}  =>  {}", e.src.display(), out.display());
        } else {
            println!("{}  =>  {}", e.src.display(), out.display());
        }
    }
}

fn dry_run_json(entries: &[Entry], overwrite: bool) {
    let mut success = 0u64;
    let mut skipped = 0u64;
    for e in entries {
        match e.kind {
            Kind::Unsupported => {
                emit_json_skip(e, "unsupported");
                skipped += 1;
            }
            Kind::Image => {
                let out = e.out.as_ref().unwrap();
                if !overwrite && out.exists() {
                    emit_json_skip(e, "exists");
                    skipped += 1;
                } else {
                    emit_json_ok_nodims(e);
                    success += 1;
                }
            }
        }
    }
    emit_done(success, skipped, 0);
}

fn emit_text_task(e: &Entry, task: &Task, verbose: bool) {
    match task {
        Task::Done {
            in_bytes,
            out_bytes,
            ..
        } => {
            if verbose {
                let out = e.out.as_ref().unwrap();
                eprintln!(
                    "[转换] {} -> {} ({}B -> {}B)",
                    e.src.display(),
                    out.display(),
                    in_bytes,
                    out_bytes
                );
            }
        }
        Task::Exists => {
            if verbose {
                let out = e.out.as_ref().unwrap();
                eprintln!("[跳过] 输出已存在: {}", out.display());
            }
        }
        Task::Unsupported => {}
        Task::Failed { err, .. } => {
            eprintln!("[失败] {}: {}", e.src.display(), err);
        }
    }
}

fn emit_json_task(e: &Entry, task: &Task) {
    match task {
        Task::Done { width, height, .. } => emit_json_ok(&e.rel, *width, *height),
        Task::Exists => emit_json_skip(e, "exists"),
        Task::Unsupported => emit_json_skip(e, "unsupported"),
        Task::Failed { reason, .. } => emit_json_skip(e, reason.as_str()),
    }
}

fn emit_scan(total: usize) {
    println!("{{\"event\":\"scan\",\"total\":{total}}}");
}

fn emit_json_ok(rel: &Path, width: u32, height: u32) {
    println!(
        "{{\"event\":\"file\",\"ok\":true,\"rel\":\"{}\",\"width\":{width},\"height\":{height}}}",
        json_escape(&rel_slash(rel))
    );
}

fn emit_json_ok_nodims(e: &Entry) {
    println!(
        "{{\"event\":\"file\",\"ok\":true,\"rel\":\"{}\"}}",
        json_escape(&rel_slash(&e.rel))
    );
}

fn emit_json_skip(e: &Entry, reason: &str) {
    println!(
        "{{\"event\":\"file\",\"ok\":false,\"rel\":\"{}\",\"reason\":\"{}\"}}",
        json_escape(&rel_slash(&e.rel)),
        reason
    );
}

fn emit_done(success: u64, skipped: u64, failed: u64) {
    println!(
        "{{\"event\":\"done\",\"success\":{success},\"skipped\":{skipped},\"failed\":{failed}}}"
    );
}

fn rel_slash(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn print_summary(s: &Summary) {
    println!("------ 汇总 ------");
    println!("成功转换: {} 张", s.done);
    println!("跳过(输出已存在): {} 张", s.skipped_exists);
    println!("失败: {} 张", s.failed);
    println!("跳过(非图片扩展): {} 个文件", s.scan.skipped_ext);
    if s.scan.walk_errors > 0 {
        println!("目录遍历失败: {} 处", s.scan.walk_errors);
    }
    if s.done > 0 && s.in_bytes > 0 {
        let saved = if s.in_bytes >= s.out_bytes {
            (s.in_bytes - s.out_bytes) as f64 / s.in_bytes as f64 * 100.0
        } else {
            -((s.out_bytes - s.in_bytes) as f64 / s.in_bytes as f64 * 100.0)
        };
        println!(
            "原始大小: {} ({} KB)  ->  WebP 输出: {} ({} KB)  (变化 {:.1}%)",
            s.in_bytes,
            s.in_bytes / 1024,
            s.out_bytes,
            s.out_bytes / 1024,
            saved
        );
    }
    println!("用时: {:.2}s", s.elapsed_ms as f64 / 1000.0);
}

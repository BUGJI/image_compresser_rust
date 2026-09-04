use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use clap::Parser;
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;

mod convert;
mod scan;

use convert::{ConvConfig, Outcome};
use scan::{Job, ScanCounts};

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

    #[arg(long, default_value_t = 100, help = "进度日志分批条数，0=关闭进度输出")]
    batch: u64,

    #[arg(long, help = "输出文件已存在时覆盖；默认跳过")]
    overwrite: bool,

    #[arg(long, help = "仅扫描并打印将要转换的文件，不写入")]
    dry_run: bool,

    #[arg(short, long, help = "输出每步详细信息")]
    verbose: bool,
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

    let (jobs, scan_counts) = scan::scan(&input, &output_root, exclude, cli.verbose);

    let workers = if cli.workers > 0 {
        cli.workers
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    };

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
    if scan_counts.walk_errors > 0 {
        eprintln!("警告: 目录遍历失败 {} 处", scan_counts.walk_errors);
    }

    if cli.dry_run {
        if jobs.is_empty() {
            println!("没有可转换的文件");
        }
        for job in &jobs {
            if !cli.overwrite && job.out.exists() {
                println!(
                    "[将跳过-已存在] {}  =>  {}",
                    job.src.display(),
                    job.out.display()
                );
            } else {
                println!("{}  =>  {}", job.src.display(), job.out.display());
            }
        }
        return Ok(0);
    }

    if jobs.is_empty() {
        println!("没有可转换的文件");
        return Ok(0);
    }

    let pool = ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(|e| format!("创建 worker 线程池失败: {e}"))?;

    let start = Instant::now();
    let processed = AtomicU64::new(0);
    let total = jobs.len() as u64;
    let cfg = ConvConfig {
        resize: cli.resize,
        width: cli.width,
        quality: cli.quality,
        overwrite: cli.overwrite,
    };
    let verbose = cli.verbose;
    let batch = cli.batch;

    let results: Vec<(Outcome, Option<(u64, u64)>)> = pool.install(|| {
        jobs.par_iter()
            .map(|job: &Job| {
                let (outcome, sizes) = convert::convert_one(&job.src, &job.out, &cfg, verbose);
                let cur = processed.fetch_add(1, Ordering::Relaxed) + 1;
                if batch > 0 && cur.is_multiple_of(batch) {
                    eprintln!("[进度] 已处理 {cur}/{total}");
                }
                (outcome, sizes)
            })
            .collect()
    });

    let mut done = 0u64;
    let mut skipped_exists = 0u64;
    let mut failed = 0u64;
    let mut in_bytes = 0u64;
    let mut out_bytes = 0u64;
    for (outcome, sizes) in results {
        match outcome {
            Outcome::Done => {
                done += 1;
                if let Some((i, o)) = sizes {
                    in_bytes += i;
                    out_bytes += o;
                }
            }
            Outcome::SkippedExists => skipped_exists += 1,
            Outcome::Failed => failed += 1,
        }
    }

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

    if summary.done > 0 {
        println!("输出目录: {}", output_root.display());
    }
    Ok(if summary.failed > 0 { 1 } else { 0 })
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

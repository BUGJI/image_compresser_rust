use std::ffi::OsString;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::convert::image_format_of;

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanCounts {
    pub total_files: usize,
    pub supported: usize,
    pub skipped_ext: usize,
    pub walk_errors: usize,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub src: PathBuf,
    pub out: PathBuf,
}

pub fn out_path_for(output_root: &Path, rel: &Path) -> PathBuf {
    let mut name: OsString = rel.as_os_str().to_owned();
    name.push(".webp");
    output_root.join(name)
}

pub fn scan(
    input: &Path,
    output_root: &Path,
    exclude: Option<&Path>,
    verbose: bool,
) -> (Vec<Job>, ScanCounts) {
    let mut counts = ScanCounts::default();
    let mut jobs = Vec::new();

    let iter = WalkDir::new(input)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| exclude.is_none_or(|x| !e.path().starts_with(x)));

    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                counts.walk_errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        counts.total_files += 1;

        let path = entry.path();
        let rel = path.strip_prefix(input).unwrap_or(path);
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if image_format_of(&ext).is_none() {
            counts.skipped_ext += 1;
            if verbose {
                eprintln!("[忽略] 非支持的图片扩展名: {}", path.display());
            }
            continue;
        }

        counts.supported += 1;
        jobs.push(Job {
            src: path.to_path_buf(),
            out: out_path_for(output_root, rel),
        });
    }

    (jobs, counts)
}

pub fn supported_ext_list() -> String {
    let exts = [
        "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "ico", "tga", "pnm", "pbm", "pgm",
        "ppm", "pam", "qoi", "hdr", "dds", "webp",
    ];
    exts.join(", ")
}

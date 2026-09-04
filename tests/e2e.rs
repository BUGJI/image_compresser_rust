use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use image::codecs::gif::GifEncoder;
use image::{Frame, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_image_compresser"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn png(path: &Path, w: u32, h: u32, c: [u8; 4]) {
    RgbaImage::from_pixel(w, h, Rgba(c)).save(path).unwrap();
}

fn jpg(path: &Path, w: u32, h: u32, c: [u8; 3]) {
    RgbImage::from_pixel(w, h, Rgb(c)).save(path).unwrap();
}

fn gif(path: &Path, w: u32, h: u32) {
    let f1 = RgbaImage::from_pixel(w, h, Rgba([255, 0, 0, 255]));
    let f2 = RgbaImage::from_pixel(w, h, Rgba([0, 0, 255, 255]));
    let file = fs::File::create(path).unwrap();
    let mut enc = GifEncoder::new(file);
    let frames = vec![Frame::new(f1), Frame::new(f2)];
    enc.encode_frames(frames).unwrap();
}

fn webp(path: &Path, w: u32, h: u32, c: [u8; 3]) {
    let img = RgbImage::from_pixel(w, h, Rgb(c));
    let enc = webp::Encoder::from_rgb(img.as_raw(), w, h);
    fs::write(path, enc.encode(90.0).as_ref()).unwrap();
}

fn webp_dims(data: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(data).unwrap();
    (img.width(), img.height())
}

fn collect_relative_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, &mut v);
    v
}

fn build_tree(dir: &Path) -> PathBuf {
    let photos = dir.join("photos");
    let sub = photos.join("sub");
    let deep = sub.join("deep");
    fs::create_dir_all(&deep).unwrap();
    let note = photos.join("note");
    fs::create_dir_all(&note).unwrap();
    png(&photos.join("a.png"), 10, 10, [200, 50, 50, 255]);
    jpg(&sub.join("b.jpg"), 8, 6, [50, 100, 200]);
    gif(&deep.join("c.gif"), 20, 10);
    webp(&sub.join("x.webp"), 5, 5, [10, 20, 30]);
    fs::write(note.join("readme.txt"), "hello").unwrap();
    photos
}

#[test]
fn mirror_structure_and_rerun_policy() {
    let tmp = TempDir::new().unwrap();
    let inp = build_tree(tmp.path());

    let out_dir = tmp.path().join("out");
    let o = run(&["-i", inp.to_str().unwrap(), "-o", out_dir.to_str().unwrap()]);
    assert!(o.status.success());

    let expect = [
        "a.png.webp",
        "sub/b.jpg.webp",
        "sub/deep/c.gif.webp",
        "sub/x.webp.webp",
    ];
    for rel in expect {
        let p = out_dir.join(rel);
        assert!(p.exists(), "missing {}", p.display());
    }
    assert!(
        !out_dir.join("note/readme.txt.webp").exists(),
        "txt should not be converted"
    );
    for f in collect_relative_files(&out_dir) {
        assert!(
            f.extension().is_some_and(|e| e == "webp"),
            "unexpected {}",
            f.display()
        );
    }

    let gif_bytes = fs::read(out_dir.join("sub/deep/c.gif.webp")).unwrap();
    assert_eq!(
        webp_dims(&gif_bytes),
        (20, 10),
        "gif should be first-frame static"
    );
    let png_bytes = fs::read(out_dir.join("a.png.webp")).unwrap();
    assert_eq!(webp_dims(&png_bytes), (10, 10));

    let before = fs::read(out_dir.join("a.png.webp")).unwrap();

    let o2 = run(&["-i", inp.to_str().unwrap(), "-o", out_dir.to_str().unwrap()]);
    assert!(o2.status.success());
    let s2 = String::from_utf8_lossy(&o2.stdout).into_owned();
    assert!(s2.contains("成功转换: 0 张"), "rerun should skip all: {s2}");
    assert!(
        s2.contains("跳过(输出已存在): 4 张"),
        "rerun should skip 4: {s2}"
    );
    assert!(s2.contains("跳过(非图片扩展): 1 个文件"));
    assert_eq!(fs::read(out_dir.join("a.png.webp")).unwrap(), before);

    let o3 = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out_dir.to_str().unwrap(),
        "--overwrite",
    ]);
    assert!(o3.status.success());
    let s3 = String::from_utf8_lossy(&o3.stdout).into_owned();
    assert!(
        s3.contains("成功转换: 4 张"),
        "overwrite should convert 4: {s3}"
    );
}

#[test]
fn resize_only_when_wider_and_flag_gated() {
    let tmp = TempDir::new().unwrap();
    let inp = tmp.path().join("photos");
    fs::create_dir_all(&inp).unwrap();
    png(&inp.join("wide.png"), 2000, 1000, [255, 255, 255, 255]);
    png(&inp.join("small.png"), 100, 100, [255, 255, 255, 255]);

    let out_plain = tmp.path().join("plain");
    let o1 = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out_plain.to_str().unwrap(),
    ]);
    assert!(o1.status.success());
    let wide = fs::read(out_plain.join("wide.png.webp")).unwrap();
    assert_eq!(webp_dims(&wide), (2000, 1000), "no resize without flag");

    let out_res = tmp.path().join("resized");
    let o2 = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out_res.to_str().unwrap(),
        "--resize",
        "--width",
        "512",
    ]);
    assert!(o2.status.success());
    let wide = fs::read(out_res.join("wide.png.webp")).unwrap();
    assert_eq!(webp_dims(&wide), (512, 256), "wider image shrunk to 512");
    let small = fs::read(out_res.join("small.png.webp")).unwrap();
    assert_eq!(webp_dims(&small), (100, 100), "narrower image untouched");
}

#[test]
fn corrupt_file_fails_and_exits_nonzero() {
    let tmp = TempDir::new().unwrap();
    let inp = tmp.path().join("in");
    fs::create_dir_all(&inp).unwrap();
    fs::write(inp.join("ok.png"), b"this is definitely not an image").unwrap();
    let out = tmp.path().join("out");
    let o = run(&["-i", inp.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(s.contains("失败: 1 张"), "{s}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("解码错误"));
}

#[test]
fn dry_run_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let inp = build_tree(tmp.path());
    let out = tmp.path().join("out");
    let o = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(o.status.success());
    assert!(!out.exists(), "dry run must not create output dir");
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(s.contains("a.png.webp") && s.contains("=>"));
}

#[test]
fn nested_output_dir_not_rescanned() {
    let tmp = TempDir::new().unwrap();
    let inp = tmp.path().join("photos");
    fs::create_dir_all(&inp).unwrap();
    png(&inp.join("a.png"), 200, 200, [1, 2, 3, 255]);

    let out1 = tmp.path().join("o1");
    let o1 = run(&["-i", inp.to_str().unwrap(), "-o", out1.to_str().unwrap()]);
    assert!(o1.status.success());

    let out2 = inp.join("nested");
    let r2 = run(&["-i", inp.to_str().unwrap(), "-o", out2.to_str().unwrap()]);
    assert!(r2.status.success());
    let r3 = run(&["-i", inp.to_str().unwrap(), "-o", out2.to_str().unwrap()]);
    assert!(r3.status.success());
    let s = String::from_utf8_lossy(&r3.stdout).into_owned();
    assert!(
        s.contains("跳过(输出已存在): 1 张"),
        "no runaway re-encode: {s}"
    );
    let files = collect_relative_files(&out2);
    assert_eq!(files.len(), 1, "only one output file expected: {files:?}");
}

#[test]
fn mislabeled_extension_decoded_by_content() {
    let tmp = TempDir::new().unwrap();
    let inp = tmp.path().join("in");
    fs::create_dir_all(&inp).unwrap();
    let rgb = RgbImage::from_pixel(12, 9, Rgb([40, 60, 80]));
    let mut buf = Vec::new();
    rgb.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .unwrap();
    fs::write(inp.join("actually_jpeg.png"), &buf).unwrap();
    let out = tmp.path().join("out");
    let o = run(&["-i", inp.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(o.status.success());
    let p = out.join("actually_jpeg.png.webp");
    assert!(
        p.exists(),
        "mislabeled jpeg should still convert: {}",
        p.display()
    );
    let data = fs::read(p).unwrap();
    assert_eq!(webp_dims(&data), (12, 9));
}

#[test]
fn json_output_ndjson_and_reconcilable() {
    let tmp = TempDir::new().unwrap();
    let inp = build_tree(tmp.path());
    let out = tmp.path().join("out");
    let o = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--output-format",
        "json",
    ]);
    assert!(o.status.success());

    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        !stdout.contains('\\'),
        "stdout must be pure ndjson, got: {stdout}"
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some(r#"{"event":"scan","total":5}"#)
    );

    let file_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.contains("\"event\":\"file\""))
        .copied()
        .collect();
    assert_eq!(
        file_lines.len(),
        5,
        "every scanned file must emit one event"
    );
    assert!(
        stdout.contains("\"rel\":\"sub/deep/c.gif\""),
        "rel must use forward slashes"
    );
    assert!(stdout.contains("\"rel\":\"note/readme.txt\""));

    let ok_true = file_lines
        .iter()
        .filter(|l| l.contains("\"ok\":true"))
        .count();
    assert_eq!(ok_true, 4);
    assert!(
        stdout.contains("\"width\":10,\"height\":10"),
        "ok event must carry output dims"
    );
    let unsupported = file_lines
        .iter()
        .filter(|l| l.contains("\"reason\":\"unsupported\""))
        .count();
    assert_eq!(unsupported, 1);
    assert_eq!(
        lines.last().copied(),
        Some(r#"{"event":"done","success":4,"skipped":1,"failed":0}"#)
    );

    let o2 = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--output-format",
        "json",
    ]);
    assert!(o2.status.success());
    let s2 = String::from_utf8_lossy(&o2.stdout).into_owned();
    assert_eq!(
        s2.matches("\"reason\":\"exists\"").count(),
        4,
        "rerun marks 4 exists"
    );
    assert!(
        s2.trim_end()
            .ends_with(r#"{"event":"done","success":0,"skipped":5,"failed":0}"#),
        "{s2}"
    );
}

#[test]
fn json_output_corrupt_reports_decode_and_exit_code() {
    let tmp = TempDir::new().unwrap();
    let inp = tmp.path().join("in");
    fs::create_dir_all(&inp).unwrap();
    fs::write(inp.join("bad.png"), b"not a real image").unwrap();
    png(&inp.join("good.png"), 6, 4, [9, 9, 9, 255]);
    let out = tmp.path().join("out");
    let o = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--output-format",
        "json",
    ]);
    assert_eq!(o.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        stdout.contains(r#"{"event":"file","ok":false,"rel":"bad.png","reason":"decode"}"#),
        "{stdout}"
    );
    assert!(
        stdout
            .trim_end()
            .ends_with(r#"{"event":"done","success":1,"skipped":0,"failed":1}"#)
    );
}

#[test]
fn json_output_dry_run() {
    let tmp = TempDir::new().unwrap();
    let inp = build_tree(tmp.path());
    let out = tmp.path().join("out");
    let o = run(&[
        "-i",
        inp.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--output-format",
        "json",
        "--dry-run",
    ]);
    assert!(o.status.success());
    assert!(!out.exists());
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(stdout.contains("\"ok\":true") && stdout.contains("\"reason\":\"unsupported\""));
    assert!(
        stdout
            .trim_end()
            .ends_with(r#"{"event":"done","success":4,"skipped":1,"failed":0}"#)
    );
}

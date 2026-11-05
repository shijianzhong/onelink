//! M3 端到端验收（不是常规单测）：真实起两个 iroh 服务，用一个 temp HOME 目录，
//! 传输 200MB 文件，验证接收端落盘、blake3 一致，并打印耗时/吞吐。
//!
//! 用法：`cargo run --example e2e_file`
//! 成功输出 `PASS: ...` 并返回 0；任一步失败输出 `FAIL: ...` 并返回 1。
//! 峰值内存用外部工具测：`/usr/bin/time -l target/debug/examples/e2e_file`。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use onelink::net::{UiEvent, shutdown_iroh_service, spawn_iroh_service};

/// 传输大小：200MB（按计划要求，不用小文件替代）。
const FILE_SIZE: u64 = 200 * 1024 * 1024;
/// 写源文件时每次写入的字节数（1MB 缓冲，避免堆里攒大文件）。
const WRITE_CHUNK: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(180);

fn make_source(path: &Path) -> blake3::Hash {
    use std::io::Write;

    let mut file = std::fs::File::create(path).expect("create source file");
    let mut hasher = blake3::Hasher::new();
    let chunk = vec![0x5A_u8; WRITE_CHUNK];
    let mut written = 0_u64;
    while written < FILE_SIZE {
        let n = (FILE_SIZE - written).min(WRITE_CHUNK as u64) as usize;
        file.write_all(&chunk[..n]).expect("write source file");
        hasher.update(&chunk[..n]);
        written += n as u64;
    }
    file.flush().expect("flush source file");
    hasher.finalize()
}

fn hash_file(path: &Path) -> std::io::Result<blake3::Hash> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0_u8; WRITE_CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buf[..n]);
    }
}

fn fail(msg: impl std::fmt::Display) -> ! {
    println!("FAIL: {msg}");
    std::process::exit(1);
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // 独立 HOME，接收目录落到临时 ~/Downloads/OneLink，不污染真实家目录。
    let dir = std::env::temp_dir().join(format!("onelink-e2e-file-{}", std::process::id()));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("create temp home");
    // SAFETY: e2e 是独立进程，此刻尚未启动任何线程/服务（tracing、临时目录都用绝对路径）。
    unsafe { std::env::set_var("HOME", &home) };

    let src = dir.join("source.bin");
    let started = Instant::now();
    let expected = make_source(&src);
    println!(
        "[e2e] source ready: {} MB in {:?}",
        FILE_SIZE / (1024 * 1024),
        started.elapsed()
    );

    let a = spawn_iroh_service().expect("spawn service A");
    let b = spawn_iroh_service().expect("spawn service B");
    println!("[e2e] two services up");
    println!("[e2e] A={} B={}", a.node_id, b.node_id);

    let send_start = Instant::now();
    let xfer_id = a.send_file_to(b.addr.clone(), src.clone());
    println!("[e2e] A -> B xfer_id={xfer_id}");

    let deadline = send_start + TIMEOUT;
    let mut sender_done = false;
    let mut receiver_path: Option<PathBuf> = None;
    let mut failure: Option<String> = None;
    let mut progress_events = 0_u32;

    while Instant::now() < deadline {
        while let Some(ev) = a.tx.try_recv() {
            match ev {
                UiEvent::TransferProgress { xfer_id: id, .. } if id == xfer_id => {
                    progress_events += 1;
                }
                UiEvent::TransferCompleted { xfer_id: id, .. } if id == xfer_id => {
                    sender_done = true;
                }
                UiEvent::TransferFailed {
                    xfer_id: id,
                    reason,
                    ..
                }
                | UiEvent::TransferRejected {
                    xfer_id: id,
                    reason,
                    ..
                } if id == xfer_id => failure = Some(reason),
                _ => {}
            }
        }
        while let Some(ev) = b.tx.try_recv() {
            match ev {
                UiEvent::TransferOffer { xfer_id: id, .. } if id == xfer_id => {
                    println!("[e2e] B got offer id={id}, auto-accept");
                    b.accept_file(id);
                }
                UiEvent::TransferProgress { xfer_id: id, .. } if id == xfer_id => {
                    progress_events += 1;
                }
                UiEvent::TransferCompleted {
                    xfer_id: id, path, ..
                } if id == xfer_id => {
                    receiver_path = path;
                }
                UiEvent::TransferFailed {
                    xfer_id: id,
                    reason,
                    ..
                }
                | UiEvent::TransferRejected {
                    xfer_id: id,
                    reason,
                    ..
                } if id == xfer_id => failure = Some(reason),
                _ => {}
            }
        }
        if failure.is_some() || (sender_done && receiver_path.is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let elapsed = send_start.elapsed();

    shutdown_iroh_service(&a);
    shutdown_iroh_service(&b);

    if let Some(reason) = failure {
        cleanup(&dir);
        fail(format!("transfer failed: {reason}"));
    }
    if !sender_done {
        cleanup(&dir);
        fail("sender never got TransferCompleted");
    }
    let Some(path) = receiver_path else {
        cleanup(&dir);
        fail("receiver never got TransferCompleted");
    };

    // 落盘校验：存在、大小一致、blake3 一致、无 .part 残留。
    let meta = std::fs::metadata(&path).unwrap_or_else(|e| {
        cleanup(&dir);
        fail(format!("received file missing at {}: {e}", path.display()));
    });
    if meta.len() != FILE_SIZE {
        cleanup(&dir);
        fail(format!(
            "received size {} != expected {FILE_SIZE}",
            meta.len()
        ));
    }
    let actual = hash_file(&path).unwrap_or_else(|e| {
        cleanup(&dir);
        fail(format!("hash received file: {e}"));
    });
    if actual != expected {
        cleanup(&dir);
        fail(format!("hash mismatch: expected {expected}, got {actual}"));
    }
    if let Some(parent) = path.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("part") {
                cleanup(&dir);
                fail(format!(".part left behind: {}", entry.path().display()));
            }
        }
    }

    let secs = elapsed.as_secs_f64();
    let mb_per_s = FILE_SIZE as f64 / (1024.0 * 1024.0) / secs;
    println!(
        "PASS: 200MB in {secs:.2}s ({mb_per_s:.1} MB/s) hash ok (progress events {progress_events})"
    );
    println!("[e2e] received at {}", path.display());

    cleanup(&dir);
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

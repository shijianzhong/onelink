//! M3 Task 2：接收落盘策略、哈希与进度节流（纯逻辑，无网络）。

use onelink::xfer::{
    ProgressThrottle, default_recv_dir, file_hash, sanitize_file_name, unique_path,
};
use std::time::{Duration, Instant};

#[test]
fn sanitize_strips_traversal_and_separators() {
    assert_eq!(
        sanitize_file_name("../../../../.ssh/authorized_keys"),
        "authorized_keys"
    );
    assert_eq!(sanitize_file_name("/etc/passwd"), "passwd");
    assert_eq!(sanitize_file_name("C:\\Windows\\evil.exe"), "evil.exe");
    assert_eq!(sanitize_file_name("bad\u{0}name.txt"), "badname.txt");
    assert_eq!(sanitize_file_name("   "), "unnamed");
    assert_eq!(sanitize_file_name(".."), "unnamed");
    assert_eq!(sanitize_file_name(&"x".repeat(300)).chars().count(), 200);
}

#[test]
fn unique_path_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("合同.pdf");
    std::fs::write(&base, b"original").unwrap();

    let next = unique_path(dir.path(), "合同.pdf");
    assert_eq!(next.file_name().unwrap().to_string_lossy(), "合同 (1).pdf");
    assert_eq!(std::fs::read(&base).unwrap(), b"original");

    std::fs::write(&next, b"second").unwrap();
    let third = unique_path(dir.path(), "合同.pdf");
    assert_eq!(third.file_name().unwrap().to_string_lossy(), "合同 (2).pdf");
}

#[test]
fn default_recv_dir_sits_under_downloads() {
    let dir = default_recv_dir();
    assert!(dir.ends_with("Downloads/OneLink"), "dir was {dir:?}");
}

#[tokio::test]
async fn file_hash_matches_blake3_of_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.bin");
    std::fs::write(&p, b"hello onelink").unwrap();
    assert_eq!(
        file_hash(&p).await.unwrap(),
        blake3::hash(b"hello onelink").to_hex().to_string()
    );
}

#[tokio::test]
async fn file_hash_streams_large_file_without_loading_it() {
    // 5MB 文件分块哈希，结果与整块哈希一致；顺带确认 64KB 缓冲路径能跨块。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.bin");
    let data = vec![0x5A_u8; 5 * 1024 * 1024];
    std::fs::write(&p, &data).unwrap();
    assert_eq!(
        file_hash(&p).await.unwrap(),
        blake3::hash(&data).to_hex().to_string()
    );
}

#[test]
fn throttle_emits_on_percent_step_or_200ms() {
    let mut t = ProgressThrottle::new();
    let t0 = Instant::now();

    assert!(t.should_emit(0, 1000, t0), "首次必发");
    assert!(!t.should_emit(1, 1000, t0), "不足 1% 且不足 200ms");
    assert!(t.should_emit(10, 1000, t0), "跨 1%");
    assert!(!t.should_emit(11, 1000, t0));
    assert!(
        t.should_emit(12, 1000, t0 + Duration::from_millis(250)),
        "200ms 到"
    );
    assert!(
        t.should_emit(1000, 1000, t0 + Duration::from_millis(260)),
        "完成必发"
    );
}

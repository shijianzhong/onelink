//! 退出路径：服务线程能被动停止并在 2 秒内收尾。

use std::time::{Duration, Instant};

use onelink::net::{shutdown_iroh_service, spawn_iroh_service};

#[test]
fn shutdown_stops_service_thread_within_two_seconds() {
    let handle = spawn_iroh_service().expect("spawn iroh service");
    // 等 endpoint + mDNS 起来，避免测的是启动中的线程
    std::thread::sleep(Duration::from_millis(600));
    assert!(handle.is_running(), "service thread should be running");

    let started = Instant::now();
    shutdown_iroh_service(&handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "shutdown took {elapsed:?}"
    );
    assert!(!handle.is_running(), "service thread still alive");
}

//! 退出路径：服务线程能被动停止、有宽限期兜底，且在异常情况下也不会挂住。

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

/// 资源刚就绪就退出：不能因为 mDNS/relay 还在跑而挂住。
#[test]
fn shutdown_right_after_spawn_does_not_hang() {
    let handle = spawn_iroh_service().expect("spawn iroh service");
    let started = Instant::now();
    shutdown_iroh_service(&handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "shutdown took {elapsed:?}"
    );
}

/// 宽限期一到就放弃优雅关闭：即使服务线程还在收尾，shutdown 也必须立刻返回。
#[test]
fn shutdown_returns_immediately_when_grace_expires() {
    let handle = spawn_iroh_service().expect("spawn iroh service");
    std::thread::sleep(Duration::from_millis(600));

    let started = Instant::now();
    let finished = handle.shutdown(Duration::ZERO);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "zero-grace shutdown waited {elapsed:?}"
    );
    // 零宽限下大概率还没收尾；关键是不阻塞，而不是结果一定为 true。
    let _ = finished;
}

/// 幂等：重复关闭直接返回，不重复 join、不 panic。
#[test]
fn shutdown_is_idempotent() {
    let handle = spawn_iroh_service().expect("spawn iroh service");
    std::thread::sleep(Duration::from_millis(600));
    shutdown_iroh_service(&handle);

    let started = Instant::now();
    shutdown_iroh_service(&handle);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "second shutdown waited {elapsed:?}"
    );
}

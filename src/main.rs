use std::sync::Arc;

use gpui_kit::*;
use onelink::app::App;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("onelink starting");
    // 启动 iroh 服务（endpoint + mDNS 发现 + 握手），事件队列交给 UI 事件泵。
    let service =
        Arc::new(onelink::net::spawn_iroh_service().expect("failed to spawn iroh service"));
    let events = Arc::clone(&service.tx);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let events = Arc::clone(&events);
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            window.set_window_title("OneLink");
            cx.new(|cx| App::new(cx, events))
        })
        .expect("failed to open window");
    });
    // 窗口全部关闭后 GPUI 的 run 返回：停服务线程、close endpoint。
    tracing::info!("ui exited, shutting down iroh service");
    onelink::net::shutdown_iroh_service(&service);
    tracing::info!("onelink exited cleanly");
}

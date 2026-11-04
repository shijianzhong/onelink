use gpui_kit::*;
use onelink::app::App;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("onelink starting");
    // 启动 iroh 服务（endpoint + mDNS 发现）。句柄在 Task 6 接入 UI。
    let _service = onelink::net::spawn_iroh_service().expect("failed to spawn iroh service");
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            window.set_window_title("OneLink");
            cx.new(|cx| App::new(cx))
        })
        .expect("failed to open window");
    });
}

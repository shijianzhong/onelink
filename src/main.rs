use std::sync::Arc;

use gpui_kit::*;
use onelink::app::App;
use onelink::ui::theme::size as ui_size;

/// 主窗口初始 bounds：按 IM 的紧凑尺度居中放在主显示器可用区域内。
fn initial_bounds(cx: &gpui_kit::App) -> gpui_kit::WindowBounds {
    let window_size = gpui_kit::Size {
        width: px(ui_size::WINDOW_W),
        height: px(ui_size::WINDOW_H),
    };
    let origin = cx
        .primary_display()
        .map(|display| {
            let area = display.visible_bounds();
            gpui_kit::Point {
                x: area.origin.x + (area.size.width - window_size.width) / 2.0,
                y: area.origin.y + (area.size.height - window_size.height) / 2.0,
            }
        })
        .unwrap_or_default();
    gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
        origin,
        size: window_size,
    })
}

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("onelink starting");
    // 启动 iroh 服务（endpoint + mDNS 发现 + 握手），事件队列交给 UI 事件泵。
    let service =
        Arc::new(onelink::net::spawn_iroh_service().expect("failed to spawn iroh service"));
    let shutdown_handle = Arc::clone(&service);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let service = Arc::clone(&service);
        let options = WindowOptions {
            window_bounds: Some(initial_bounds(&*cx)),
            // 再缩小就会把 300px 侧栏之外的聊天区挤到气泡最大宽度以下。
            window_min_size: Some(gpui_kit::Size {
                width: px(ui_size::WINDOW_MIN_W),
                height: px(ui_size::WINDOW_MIN_H),
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, move |window, cx| {
            window.set_window_title("OneLink");
            cx.new(|cx| App::new(window, cx, Arc::clone(&service)))
        })
        .expect("failed to open window");
    });
    // 窗口全部关闭后 GPUI 的 run 返回：停服务线程、close endpoint。
    tracing::info!("ui exited, shutting down iroh service");
    onelink::net::shutdown_iroh_service(&shutdown_handle);
    tracing::info!("onelink exited cleanly");
}

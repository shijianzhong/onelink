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
    // 打开本地库（损坏会自动备份重建）；昵称用于握手，空则回退主机名。
    let (store, loaded) = onelink::store::Store::open(&onelink::paths::db_path());
    let nickname = loaded.self_nickname.clone().unwrap_or_default();
    // 启动 iroh 服务（endpoint + mDNS 发现 + 握手），事件队列交给 UI 事件泵。
    // 身份密钥持久化：node id 跨重启不变，对端不会把本机当成一台新设备。
    let service = Arc::new(
        onelink::net::spawn_iroh_service_persistent(nickname, &onelink::paths::endpoint_key_path())
            .expect("failed to spawn iroh service"),
    );
    let shutdown_handle = Arc::clone(&service);
    gpui_kit::application()
        // macOS 默认"关掉最后一个窗口但进程继续跑"，会留下没有窗口的僵尸进程；
        // 单窗口 IM 客户端关窗即退出（与 README 的手工验证一致）。
        .with_quit_mode(gpui_kit::QuitMode::LastWindowClosed)
        .run(move |cx| {
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
                let service = Arc::clone(&service);
                cx.new(|cx| App::new(window, cx, service, store, loaded))
            })
            .expect("failed to open window");
            #[cfg(target_os = "macos")]
            disable_close_fade();
        });
    // 退出阶段：服务线程最多等一个短宽限，超时就放弃优雅关闭，不把退出路径挂住。
    onelink::net::shutdown_iroh_service(&shutdown_handle);
    tracing::info!("onelink exited cleanly");
}

/// macOS 关窗默认带 ~250–300ms 的淡出动画：点红叉后窗口会先"顿"在屏幕上再消失。
/// 把窗口的 animationBehavior 设为 None 后窗口直接消失（实测关窗可见时长 330ms → 30ms）。
///
/// gpui-pre 不暴露 NSWindow，这里用最小 FFI 直接问 AppKit 要窗口
/// （不引入 objc/cocoa 这类已弃用的绑定 crate）。
#[cfg(target_os = "macos")]
fn disable_close_fade() {
    use std::ffi::c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *const c_void;
        fn objc_msgSend();
    }

    // NSWindowAnimationBehaviorNone == 2。
    const NONE: usize = 2;
    let send0: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let send_usize0: unsafe extern "C" fn(*mut c_void, *const c_void) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let send_usize: unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe {
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let app = send0(cls, sel_registerName(c"sharedApplication".as_ptr()));
        let windows = send0(app, sel_registerName(c"windows".as_ptr()));
        let count = send_usize0(windows, sel_registerName(c"count".as_ptr()));
        let index_sel = sel_registerName(c"objectAtIndex:".as_ptr());
        let behavior_sel = sel_registerName(c"setAnimationBehavior:".as_ptr());
        for i in 0..count {
            let window = send_usize(windows, index_sel, i);
            let _ = send_usize(window, behavior_sel, NONE);
        }
    }
}

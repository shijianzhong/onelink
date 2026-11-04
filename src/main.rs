use gpui_kit::*;
use onelink::app::App;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("onelink starting");
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            window.set_window_title("OneLink");
            cx.new(|cx| App::new(cx))
        })
        .expect("failed to open window");
    });
}

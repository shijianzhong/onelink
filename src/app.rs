use gpui_kit::*;

/// OneLink 根组件。M1 阶段仅渲染占位文本，设备列表在 Task 6 接入。
pub struct App;

impl App {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for App {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("OneLink")
    }
}

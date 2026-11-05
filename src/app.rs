//! 根组件：标题 + 设备列表，并运行事件泵把网络事件喂给设备列表。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AsyncApp, Context, Entity, IntoElement, ParentElement, Render, Styled, WeakEntity, Window, div,
    prelude::*, px,
};

use crate::net::{EventQueue, UiEvent};
use crate::ui::device_list::DeviceList;

/// 事件泵轮询间隔。
const PUMP_INTERVAL: Duration = Duration::from_millis(300);

pub struct App {
    device_list: Entity<DeviceList>,
}

impl App {
    pub fn new(cx: &mut Context<Self>, events: Arc<EventQueue>) -> Self {
        let device_list = cx.new(DeviceList::new);

        let pump_events = Arc::clone(&events);
        cx.spawn(async move |this: WeakEntity<App>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                let mut batch = Vec::new();
                while let Some(ev) = pump_events.try_recv() {
                    batch.push(ev);
                }
                if batch.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |app, cx| app.apply_batch(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self { device_list }
    }

    /// 把一批网络事件应用到设备列表（事件泵与测试共用）。
    pub fn apply_batch(&mut self, batch: Vec<UiEvent>, cx: &mut Context<Self>) {
        if batch.is_empty() {
            return;
        }
        for ev in batch {
            self.device_list.update(cx, |list, cx| list.apply(ev, cx));
        }
        cx.notify();
    }
}

impl Render for App {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(18.))
                    .child("OneLink"),
            )
            .child(self.device_list.clone())
    }
}

//! OneLink —— 局域网聊天 + 文件互传客户端。
//!
//! 二进制入口见 `main.rs`；本 lib 暴露可测试的模块（模型、UI、网络服务）。

pub mod app;
pub mod avatar;
pub mod chat;
pub mod model;
pub mod net;
pub mod paths;
pub mod proto;
pub mod store;
pub mod ui;
pub mod xfer;

# OneLink

局域网内的点对点聊天 / 文件互传桌面客户端。Rust + [iroh](https://github.com/n0-computer/iroh)（QUIC P2P，
mDNS 局域网发现，官方公共 relay 仅作跨网兜底）+ [GPUI Kit](https://github.com/longbridge/gpui-kit)。

不依赖中央服务器：同网段直连，断外网仍可用；跨网时由 iroh relay 转发（默认开启，后续版本可在设置里关闭）。

## 当前进度（M2：一对一聊天）

M1（骨架 + 设备发现）已完成：窗口、mDNS 自动发现、握手换设备名、在线/离线状态、优雅退出。

M2 新增聊天：

- 点左侧设备行 → 右侧打开该设备的聊天面板（切换设备不串消息）
- 输入框 + "发送"按钮：消息经真实 QUIC 直连送到对端，对端面板出现同一条消息（对方发的左对齐、自己的右对齐）
- 对方收到后回 ACK，本端记为送达；超时（5s）或对端已退出时记 `chat send failed` 日志，UI 不卡不 panic
- 未选中设备收到消息 → 该设备行显示红色未读数，点开即清零
- 回车（Enter）直接发送；空输入不发；超过 64KB 的文本本地拒绝，不发半截
- **消息只存内存**：关闭程序即清空（历史/断点续传在 M4，SQLite）

尚未包含（M3/M4）：文件传输、传输进度、设置页、历史消息。

## 聊天（M2）

两个实例互见后（列表里出现 `● 对方设备名`）：

1. 点该设备行 → 右侧出现聊天面板（标题显示对端 node id 前 8 位）
2. 输入内容，按 **Enter** 或点"发送"（Shift+Enter 暂不处理，留给将来的多行输入）
3. 本端气泡立即出现；对端 1 秒内出现同一条消息并自动回 ACK
4. 关掉对端再发送 → 本端日志 `chat send failed ... reason="..."`，界面照常可用

聊天不经过任何服务器：同网段走 QUIC 直连，跨网时由 iroh 官方 relay 转发加密数据（relay 只见密文，不懂内容）。

## 构建与运行

需要 nightly 工具链（`rust-toolchain.toml` 已锁定，`rustup` 会自动安装）：

```bash
cargo run
```

首次运行 macOS 会询问"是否允许 OneLink 接受网络连接"——选允许，否则 mDNS 发现和入站连接会被拦。
Windows 首次运行请在防火墙弹窗中勾选"专用网络"。

看日志：

```bash
RUST_LOG=onelink=info cargo run
```

启动成功的关键两行：

```
iroh endpoint bound node_id="..." our_name="..."
mDNS service registered
```

## 双机验证

1. 两台电脑接同一路由器 / 同一 Wi-Fi（同网段），各 `cargo run`
2. 几秒内两台窗口里应互现一行：`● 对方设备名  <node_id 前 8 位>`
3. 关闭其中一台（或直接 kill 进程）→ 另一台在 mDNS 过期后把该行变成 `○` 灰色，设备不消失
4. 关闭窗口 → 进程退出，日志末尾出现 `onelink exited cleanly`，无 panic

本机自己不会出现在自己的列表里。

## 测试与检查

```bash
cargo test                              # 40 个测试：协议帧 / 服务端分帧 / 发送通道 / 会话模型 / 注册表 / 事件队列 / 握手 / UI / 接线 / 退出
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

真实网络的端到端验收（起两个 iroh 服务，走 mDNS 发现 → 握手 → 收发一条消息，不依赖 GUI）：

```bash
cargo run --example e2e_chat     # 成功打印 PASS: peer received in xxx ms
```

UI 测试跑在 GPUI 的 headless 窗口里（`#[gpui_kit::test]` + `tests/common/mod.rs` fixture）。

## 目录

```
src/main.rs            窗口入口：启动 iroh 服务 → 打开窗口 → 退出时 shutdown
src/app.rs             根组件：左设备列表 + 右聊天面板 + 300ms 事件泵 + 子组件事件订阅
src/net.rs             iroh endpoint / mDNS 发现 / frame 分派 / 上行命令通道 / 有界事件队列 / 优雅退出
src/proto.rs           线协议帧（Hello/HelloAck/Chat/ChatAck）与编解码、64KB 上限
src/chat.rs            会话模型：按 peer 存消息、未读计数、当前选中
src/model.rs           设备注册表（去重、在线状态、排序）
src/ui/device_list.rs  设备列表渲染（状态点 / 未读数 / 点击抛 Selected 事件）
src/ui/chat_panel.rs   聊天面板（消息气泡 / 输入框 / 发送事件）
examples/e2e_chat.rs   端到端验收脚本（真实 QUIC 收发）
tests/                 协议、网络、会话、UI、接线、退出
```

## 已知问题

- mDNS 过期依赖系统通告周期，进程被强杀后对方最多需要几十秒才变灰
- 同一台机器跑两个实例时设备名相同（都取 hostname），只能靠 node id 区分
- 发送失败在面板上显示红字提示（5s 超时 / 对端不可达），下一次成功送达或再次发送时自动清除；失败发生在非当前会话时只在日志可见
- 单条消息上限 64KB（超限本地拒绝，不发半截）
- `gpui-kit` 0.7.3 的 `test-support` 会把可选依赖 `gpui-fast-platform` 拉进解析，与
  `gpui-pre-macos` 的 cocoa 版本冲突；`Cargo.toml` 里已用 `cocoa = "=0.26.0"` 钉住版本

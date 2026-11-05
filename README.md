# OneLink

局域网内的点对点聊天 / 文件互传桌面客户端。Rust + [iroh](https://github.com/n0-computer/iroh)（QUIC P2P，
mDNS 局域网发现，官方公共 relay 仅作跨网兜底）+ [GPUI Kit](https://github.com/longbridge/gpui-kit)。

不依赖中央服务器：同网段直连，断外网仍可用；跨网时由 iroh relay 转发（默认开启，后续版本可在设置里关闭）。

## 当前进度

**M1（骨架 + 设备发现）已完成**：窗口、mDNS 自动发现、握手换设备名、在线/离线状态、优雅退出。

**M2（一对一聊天）已完成**：

- 点左侧设备行 → 右侧打开该设备的聊天面板（切换设备不串消息）
- 输入框 + "发送"按钮：消息经真实 QUIC 直连送到对端，对端面板出现同一条消息（对方发的左对齐、自己的右对齐）
- 对方收到后回 ACK，本端记为送达；超时（5s）或对端已退出时记 `chat send failed` 日志，UI 不卡不 panic
- 未选中设备收到消息 → 该设备行显示红色未读数，点开即清零
- 回车（Enter）直接发送；空输入不发；超过 64KB 的文本本地拒绝，不发半截
- **消息只存内存**：关闭程序即清空

**M3（文件传输）已完成的路径**：

- 聊天输入框左侧 📎 → 系统文件选择框 → 向当前会话对端发起文件请求（一次一个文件）
- 接收方在会话里看到一条传输行（文件名 + 大小 + "接受/拒绝"），点接受后才开始传数据
- 传输中显示进度条与百分比（发送端与接收端都会更新）；完成显示"完成"，失败/拒绝显示红色原因
- 文件按块流式读写，不把整文件读进内存；发送端算 blake3，接收端边收边算并比对，不一致即失败且不落成品
- 接收目录 `~/Downloads/OneLink/`，文件名做路径穿越清洗，重名自动错开（`name (1).ext`），传输中写 `.part`，成功才改名
- 发送完成后 UI 行置"完成"要等对端回 `FileAck`，不是"发出即成功"

M3 尚未做（M3.5 / M4）：拖拽发送、多文件队列、断点续传、取消传输、大小上限与接收目录设置页、
SQLite 历史消息、应用打包。**当前不是"M3 全部功能完成"的状态。**

## 聊天（M2）

两个实例互见后（列表里出现 `● 对方设备名`）：

1. 点该设备行 → 右侧出现聊天面板（标题显示对端 node id 前 8 位）
2. 输入内容，按 **Enter** 或点"发送"（Shift+Enter 暂不处理，留给将来的多行输入）
3. 本端气泡立即出现；对端 1 秒内出现同一条消息并自动回 ACK
4. 关掉对端再发送 → 本端日志 `chat send failed ... reason="..."`，界面照常可用

聊天不经过任何服务器：同网段走 QUIC 直连，跨网时由 iroh 官方 relay 转发加密数据（relay 只见密文，不懂内容）。

## 文件传输（M3）

1. 打开与对端的会话，点输入框左侧 **📎**，选一个文件
2. 本端出现传输行"等待对方接受…"；对端会话出现同一文件行，带 **接受 / 拒绝**
3. 对端点"接受" → 两侧出现进度条（约 200ms 一次刷新）；点"拒绝" → 两侧都以红色原因结束
4. 完成后本端行显示"完成"，对端文件落在 `~/Downloads/OneLink/`；同名文件自动改名，不覆盖已有文件

约定：

- 传输控制帧与文件字节走两条独立 ALPN（`onelink-v1` 控制 JSON、`onelink-v1-file` 数据块），
  数据块格式 `[u32 len][bytes]` + `[0]` 结束 + `FileDone{blake3}` + `FileAck`；**不是**设计文档早期设想的 iroh blobs
- 校验失败会删除 `.part`、不留下成品文件，发送端收到失败事件
- 接收确认默认**手动**（不自动接收），未接受/未拒绝的请求目前没有超时回收

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
cargo test                              # 62 个测试：协议帧 / 分块编解码 / 发送与接收 / 落盘清洗 / 会话模型 / 注册表 / 事件队列 / 握手 / UI / App 接线 / 退出
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

真实网络的端到端验收（起两个 iroh 服务，不依赖 GUI）：

```bash
cargo run --example e2e_chat     # 成功打印 PASS: peer received in xxx ms
cargo run --example e2e_file     # 200MB 文件经真实 QUIC 传输 + blake3 校验
```

`e2e_file` 使用临时 HOME（接收目录不落到真实 `~/Downloads`），成功输出形如
`PASS: 200MB in 18.44s (10.8 MB/s) hash ok (progress events 177)`。
debug 构建下 blake3 是 CPU 瓶颈（两侧各哈希 200MB，`time -l` 显示 user 41.7s > real 23.7s），
吞吐不代表 release 性能；峰值内存可用 `/usr/bin/time -l target/debug/examples/e2e_file` 观察
（实测 maximum RSS ≈ 54MB、peak memory footprint ≈ 33MB，与 200MB 文件大小无关，说明是流式的）。

UI 测试跑在 GPUI 的 headless 窗口里（`#[gpui_kit::test]` + `tests/common/mod.rs` fixture）。

## 目录

```
src/main.rs            窗口入口：启动 iroh 服务 → 打开窗口 → 退出时 shutdown
src/app.rs             根组件：左设备列表 + 右聊天面板 + 300ms 事件泵 + 传输事件按会话落地
src/net.rs             iroh endpoint / mDNS 发现 / frame 分派 / 文件收发 / 上行命令通道 / 有界事件队列
src/proto.rs           控制帧（Hello/HelloAck/Chat/ChatAck/FileOffer/FileAccept/FileReject/FileDone/FileAck）
src/xfer.rs            文件分块编解码、blake3、文件名清洗、重名错开、接收目录、进度节流
src/chat.rs            会话模型：按 peer 存消息、未读计数、当前选中
src/model.rs           设备注册表（去重、在线状态、排序）
src/ui/device_list.rs  设备列表渲染（状态点 / 未读数 / 点击抛 Selected 事件）
src/ui/chat_panel.rs   聊天面板（消息气泡 / 输入框 / 发送与 📎 事件 / 传输区块）
src/ui/transfer_row.rs 传输行渲染（进度条 / 接受拒绝按钮 / 失败原因）
examples/e2e_chat.rs   聊天端到端验收（真实 QUIC 收发）
examples/e2e_file.rs   文件传输端到端验收（200MB + blake3 校验）
tests/                 协议、网络、文件收发、会话、UI、接线、退出
```

## 已知问题

- mDNS 过期依赖系统通告周期，进程被强杀后对方最多需要几十秒才变灰
- 同一台机器跑两个实例时设备名相同（都取 hostname），只能靠 node id 区分
- 发送失败在面板上显示红字提示（5s 超时 / 对端不可达），下一次成功送达时自动清除；失败发生在非当前会话时只在日志可见
- 单条消息上限 64KB（超限本地拒绝，不发半截）
- 文件传输：一次只能选一个文件（无队列）；请求没有超时回收，对方不响应就会一直挂着；不支持取消/续传
- 拖拽发送待 M3.5：GPUI 0.3.9 没有元素级文件拖放 API（`on_drop` 是对象销毁回调，`FileDropEvent` 只是平台输入）
- 文件请求只按 `xfer_id`（每个服务自己的自增序号）记账，多对端并发时理论上可能撞号
- e2e 退出时 iroh 日志会打 `Endpoint dropped without calling Endpoint::close`（shutdown 直接结束服务线程，未优雅关闭 QUIC，功能与退出行为不受影响）
- 设计口径偏差：早期设计文档写过 iroh blobs 与 LanLink 命名，当前实现是自定义 ALPN 分块传输、产品名统一为 OneLink
- `gpui-kit` 0.7.3 的 `test-support` 会把可选依赖 `gpui-fast-platform` 拉进解析，与
  `gpui-pre-macos` 的 cocoa 版本冲突；`Cargo.toml` 里已用 `cocoa = "=0.26.0"` 钉住版本

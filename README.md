# OneLink

局域网内的点对点聊天 / 文件互传桌面客户端。Rust + [iroh](https://github.com/n0-computer/iroh)（QUIC P2P，
mDNS 局域网发现，官方公共 relay 仅作跨网兜底）+ [GPUI Kit](https://github.com/longbridge/gpui-kit)。

不依赖中央服务器：同网段直连，断外网仍可用；跨网时由 iroh relay 转发（默认开启，后续版本可在设置里关闭）。

## 当前进度（M1：骨架 + 设备发现）

- 打开窗口，左侧显示设备列表
- 同一局域网内自动发现其它 OneLink 实例（mDNS，`onelink-v1` 服务名）
- 通过握手交换设备名（iOS/macOS 会显示 `xxx.local`）
- 在线 `●` / 离线 `○`（设备下线后保留在列表里灰显，不消失）
- 重复发现不会产生重复行（按 node id 去重）
- 关闭窗口时优雅退出：停止服务线程、close endpoint

尚未包含（M2/M3）：聊天消息、文件传输、传输进度、设置页。

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
cargo test                              # 18 个测试：注册表 / 事件队列 / 握手 / 设备列表 UI / 退出
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

UI 测试跑在 GPUI 的 headless 窗口里（`#[gpui_kit::test]` + `tests/common/mod.rs` fixture）。

## 目录

```
src/main.rs           窗口入口：启动 iroh 服务 → 打开窗口 → 退出时 shutdown
src/app.rs            根组件：标题 + 设备列表 + 300ms 事件泵
src/net.rs            iroh endpoint / mDNS 发现 / 握手 / 有界事件队列 / 优雅退出
src/model.rs          设备注册表（去重、在线状态、排序）
src/ui/device_list.rs 设备列表渲染
tests/                注册表、事件桥、握手、UI、退出
```

## 已知问题

- mDNS 过期依赖系统通告周期，进程被强杀后对方最多需要几十秒才变灰
- 同一台机器跑两个实例时设备名相同（都取 hostname），只能靠 node id 区分
- `gpui-kit` 0.7.3 的 `test-support` 会把可选依赖 `gpui-fast-platform` 拉进解析，与
  `gpui-pre-macos` 的 cocoa 版本冲突；`Cargo.toml` 里已用 `cocoa = "=0.26.0"` 钉住版本

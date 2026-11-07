//! 浅色 IM 主题 token。
//!
//! 颜色、圆角、间距、字号与关键尺寸集中在这里，组件只引用语义名，避免魔法数字
//! 散落各处。配色取微信桌面端浅色模式的克制灰阶 + 微信绿强调色；间距/圆角档位
//! 参考 rune 的 `tok` 模块（4/8/12/24 与 6/8/12 三档），保持整套界面同一节奏。

/// 颜色 token（0xRRGGBB，配合 `rgb()` 使用）。
pub mod color {
    /// 窗口与聊天区底色。
    pub const BG: u32 = 0xF2F2F2;
    /// 左侧栏底色，比聊天区深一档以便区分区域。
    pub const SIDEBAR: u32 = 0xEAEAEA;
    /// 卡片 / 对方气泡 / 输入卡片底色。
    pub const SURFACE: u32 = 0xFFFFFF;
    /// 列表行 hover 底色。
    pub const HOVER: u32 = 0xE0E0E0;
    /// 列表行选中底色（比 hover 再深一档）。
    pub const SELECTED: u32 = 0xD6D6D6;
    /// 自己发出的消息气泡（微信绿）。
    pub const BUBBLE_MINE: u32 = 0x95EC69;
    /// 对方消息气泡描边（白底气泡在浅灰聊天区需要一点边界）。
    pub const BUBBLE_PEER_BORDER: u32 = 0xE6E6E6;
    /// 主操作色：在线状态点、发送按钮、传输进度条。
    pub const ACCENT: u32 = 0x07C160;
    /// 输入框聚焦描边（微信绿 40% 叠白）：能感知聚焦，但不抢眼。
    pub const FOCUS_BORDER: u32 = 0x9CE6BF;
    /// 主文字。
    pub const TEXT: u32 = 0x1A1A1A;
    /// 次级文字（状态、副标题）。
    pub const TEXT_SECONDARY: u32 = 0x595959;
    /// 弱化文字（时间戳、短 id、占位提示）。
    pub const TEXT_MUTED: u32 = 0x9A9A9A;
    /// 分隔线与卡片描边。
    pub const DIVIDER: u32 = 0xDCDCDC;
    /// 进度条 / 滑块轨道。
    pub const TRACK: u32 = 0xE5E5E5;
    /// 错误文字。
    pub const DANGER: u32 = 0xC0392B;
    /// 错误提示条底色。
    pub const DANGER_BG: u32 = 0xFDECEA;
    /// 未读角标底色。
    pub const UNREAD: u32 = 0xF04A4A;
    /// 头像占位底色。
    pub const AVATAR: u32 = 0xDCDCDC;
    /// 置灰的不可用控件（发送按钮空态）。
    pub const DISABLED: u32 = 0xD9D9D9;
}

/// 圆角档位。
pub mod radius {
    pub const SM: f32 = 6.;
    pub const MD: f32 = 8.;
    pub const LG: f32 = 12.;
}

/// 间距档位。
pub mod space {
    pub const S1: f32 = 4.;
    pub const S2: f32 = 8.;
    pub const S3: f32 = 12.;
    pub const S6: f32 = 24.;
}

/// 字号档位（消息正文比次要信息大一档，保证层级）。
pub mod text {
    pub const CAPTION: f32 = 11.;
    pub const BODY: f32 = 13.;
    pub const MESSAGE: f32 = 14.;
    pub const TITLE: f32 = 15.;
    pub const BRAND: f32 = 17.;
}

/// 关键尺寸。
pub mod size {
    /// 主窗口默认宽高（对齐主流 IM 的紧凑窗口，而非 GPUI 默认的 1536×1095）。
    pub const WINDOW_W: f32 = 960.;
    pub const WINDOW_H: f32 = 680.;
    /// 窗口最小宽高：再小聊天区就会被侧栏和气泡最大宽度挤扁。
    pub const WINDOW_MIN_W: f32 = 860.;
    pub const WINDOW_MIN_H: f32 = 600.;
    /// 左侧栏宽度。
    pub const SIDEBAR_W: f32 = 300.;
    /// 气泡最大宽度（约为聊天区常见宽度的 60%）。
    pub const BUBBLE_MAX_W: f32 = 460.;
    /// 消息行高。
    pub const MESSAGE_LINE_H: f32 = 20.;
    /// 输入框高度：约 5 行正文，明显大于此前的单行输入。
    pub const COMPOSER_INPUT_H: f32 = 112.;
    /// 附件按钮尺寸。
    pub const ATTACH_W: f32 = 36.;
    pub const ATTACH_H: f32 = 32.;
    /// 发送按钮尺寸。
    pub const SEND_W: f32 = 76.;
    pub const SEND_H: f32 = 32.;
    /// 设备行高度。
    pub const DEVICE_ROW_H: f32 = 64.;
    /// 设备头像边长。
    pub const AVATAR: f32 = 36.;
}

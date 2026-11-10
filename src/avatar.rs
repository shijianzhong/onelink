//! 头像编解码：任何图片先缩成小 PNG 再上线，避免大图进入握手与内存。
//!
//! 线上格式是 base64(PNG)，由 `Frame::Hello`/`HelloAck` 的 `avatar` 字段携带。
//! 三处硬约束都在这里：
//! - 原始文件上限 [`MAX_SOURCE_BYTES`]：超了不读、直接报错；
//! - 解码后最长边 [`MAX_SIDE`]：缩略图固定尺寸，一帧只有几 KB；
//! - 上线 PNG 上限 [`MAX_PNG_BYTES`]：保证 base64 后仍远低于 64 KiB 帧上限。
//!
//! 对端送来的数据一律经 [`sanitize`] 校验（尺寸 + 能否解码）并规整成正方形，
//! 校验不过就当没有头像，UI 回退首字头像——不渲染空白框、不 panic。

use std::io::Cursor;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

/// 原始图片文件上限（字节）。超过直接拒绝，不读进内存。
pub const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// 缩略图最长边（像素）。设备列表头像按 36pt 渲染，96px 足够覆盖 2x 屏。
pub const MAX_SIDE: u32 = 96;
/// 上线 PNG 字节上限：base64 后约 21 KiB，留在 64 KiB 帧上限内还有余量。
pub const MAX_PNG_BYTES: usize = 16 * 1024;
/// 接收侧允许的解压后最长边（防御"小文件解码成巨图"的解压炸弹）。
pub const MAX_RECEIVE_SIDE: u32 = 256;

/// 从文件路径生成可上线的头像：缩放到 [`MAX_SIDE`] 内、编码 PNG、转 base64。
///
/// 失败信息直接给用户看（不含路径以外的内部细节），调用方据此回退。
pub fn encode_path(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取头像文件失败：{e}"))?
        .len();
    if size > MAX_SOURCE_BYTES {
        return Err(format!(
            "图片太大（{} MiB），请选择 {} MiB 以内的图片",
            size / 1024 / 1024,
            MAX_SOURCE_BYTES / 1024 / 1024
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取头像文件失败：{e}"))?;
    encode_bytes(&bytes)
}

/// 从原始图片字节生成可上线的头像（PNG/JPEG；其余格式按解码失败处理）。
pub fn encode_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("图片太大，请选择 4 MiB 以内的图片".to_string());
    }
    let img = image::load_from_memory(bytes)
        .map_err(|_| "无法识别的图片格式（支持 PNG/JPEG）".to_string())?;
    // 逐级降尺寸：绝大多数图片第一档就够，极端噪点图才需要更小。
    for side in [MAX_SIDE, 64, 48] {
        let scaled = fit_square(&img, side);
        let png = to_png(&scaled)?;
        if png.len() <= MAX_PNG_BYTES {
            return Ok(B64.encode(&png));
        }
    }
    Err("图片压缩后仍然太大，请换一张更简单的图片".to_string())
}

/// 校验对端送来的头像：base64 → PNG 字节；任何一步不过都返回 `None`。
pub fn sanitize(b64: &str) -> Option<Vec<u8>> {
    let png = B64.decode(b64.trim()).ok()?;
    if png.is_empty() || png.len() > MAX_PNG_BYTES {
        return None;
    }
    let reader = image::ImageReader::new(Cursor::new(&png))
        .with_guessed_format()
        .ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > MAX_RECEIVE_SIDE || h > MAX_RECEIVE_SIDE {
        return None;
    }
    // 尺寸过关后再真解码一次：确保 UI 侧一定拿得到可用像素。
    let img = image::load_from_memory(&png).ok()?;
    // 统一规整成正方形：旧版/异常来源可能送来非正方形图，若原样交给渲染侧，
    // 按正方形画就会拉伸变形。这里复用同一套居中裁切，保证"头像字节恒为正方形"。
    to_png(&fit_square(&img, MAX_SIDE)).ok()
}

/// 把图片居中裁成正方形（cover）：以中心为基准取最大内接正方形，
/// 不拉伸、不留白。三处头像（设置页预览、侧栏本机、通讯录对端）
/// 拿到的字节都是这里产出的正方形，渲染侧不需要各自再缩放。
fn center_square(img: &image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    if w == h {
        return img.clone();
    }
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    img.crop_imm(x, y, side, side)
}

/// 居中裁成正方形后等比缩放到 `side × side` 以内。
fn fit_square(img: &image::DynamicImage, side: u32) -> image::DynamicImage {
    let squared = center_square(img);
    let (w, h) = (squared.width(), squared.height());
    if w <= side && h <= side {
        return squared;
    }
    let scale = side as f32 / w as f32;
    let n = ((w as f32 * scale).round() as u32).max(1);
    squared.resize_exact(n, n, image::imageops::FilterType::Lanczos3)
}

fn to_png(img: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("头像编码失败：{e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        let img =
            image::RgbaImage::from_pixel(w, h, image::Rgba([color[0], color[1], color[2], 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn large_image_is_downscaled_and_under_wire_cap() {
        let src = png_bytes(1200, 800, [10, 120, 200]);
        let b64 = encode_bytes(&src).expect("encode");
        let png = sanitize(&b64).expect("sanitized");
        assert!(png.len() <= MAX_PNG_BYTES, "png was {} bytes", png.len());
        let img = image::load_from_memory(&png).unwrap();
        assert!(img.width() <= MAX_SIDE && img.height() <= MAX_SIDE);
    }

    #[test]
    fn not_an_image_is_rejected() {
        assert!(encode_bytes(b"definitely not an image").is_err());
        assert!(sanitize("bm90IGFuIGltYWdl").is_none());
    }

    #[test]
    fn oversize_source_is_rejected_before_decoding() {
        let huge = vec![0u8; (MAX_SOURCE_BYTES + 1) as usize];
        assert!(encode_bytes(&huge).is_err());
    }

    #[test]
    fn oversized_received_dimensions_are_rejected() {
        // 512×512 超过接收上限 256：即使能解码也不收（防解压炸弹）。
        let b64 = B64.encode(png_bytes(512, 512, [0, 0, 0]));
        assert!(sanitize(&b64).is_none());
    }

    #[test]
    fn roundtrip_produces_renderable_png() {
        let b64 = encode_bytes(&png_bytes(64, 64, [200, 40, 40])).unwrap();
        let png = sanitize(&b64).expect("valid");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    /// 左右两色的非正方形图：用来判断"居中裁切"还是"贴边裁切"。
    fn split_png(w: u32, h: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for (x, _, pix) in img.enumerate_pixels_mut() {
            let c = if x < w / 2 { left } else { right };
            *pix = image::Rgba([c[0], c[1], c[2], 255]);
        }
        to_png(&image::DynamicImage::ImageRgba8(img)).unwrap()
    }

    #[test]
    fn non_square_is_center_cropped_not_stretched() {
        // 200×100，左半红右半蓝。居中裁正方形取 x∈[50,150)，输出应是左红右蓝；
        // 若按"贴左裁切"会整块变红，若拉伸则两色都出现但尺寸比例会错。
        let src = split_png(200, 100, [200, 0, 0], [0, 0, 200]);
        let b64 = encode_bytes(&src).expect("encode");
        let png = sanitize(&b64).expect("sanitize");
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (96, 96), "must be square");
        let left = img.get_pixel(2, 48).0;
        let right = img.get_pixel(93, 48).0;
        assert!(left[0] > 150 && left[2] < 60, "left was {left:?}");
        assert!(right[2] > 150 && right[0] < 60, "right was {right:?}");
    }

    #[test]
    fn extreme_aspect_ratio_keeps_center_stripe() {
        // 400×40，只有中间 40px 是绿色条：居中裁切应整块取到绿色，
        // 贴边裁切会拿到纯红。
        let mut img = image::RgbaImage::from_pixel(400, 40, image::Rgba([200, 0, 0, 255]));
        for y in 0..40 {
            for x in 180..220 {
                img.put_pixel(x, y, image::Rgba([0, 200, 0, 255]));
            }
        }
        let src = to_png(&image::DynamicImage::ImageRgba8(img)).unwrap();
        let b64 = encode_bytes(&src).expect("encode");
        let png = sanitize(&b64).expect("sanitize");
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (40, 40), "must be square");
        let center = img.get_pixel(20, 20).0;
        assert!(center[1] > 150 && center[0] < 60, "center was {center:?}");
    }

    #[test]
    fn sanitize_squares_incoming_non_square_png() {
        // 即便是直接塞进来的非正方形 PNG（旧版/异常来源），sanitize 也要规整成正方形。
        let b64 = B64.encode(split_png(120, 60, [10, 10, 10], [240, 240, 240]));
        let png = sanitize(&b64).expect("valid");
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), img.height());
        assert!(img.width() <= MAX_SIDE);
    }
}

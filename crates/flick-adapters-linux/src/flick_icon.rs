//! Flick 应用图标（SVG 源码）。
//!
//! 图标 SVG 直接以常量形式内嵌进二进制，改图标只需改这里。启动时由
//! [`crate::portal_identity::ensure_icons`] 把它光栅化成多种尺寸的 PNG，
//! 安装到 `$XDG_DATA_HOME/icons/hicolor/<N>x<N>/apps/<app_id>.png`，供
//! `.desktop` 的 `Icon=` 使用。
//!
//! 源图是 1024×1024 的矢量图（深色圆角方块 + 紫蓝闪电 + 放大镜）。

/// Flick 图标的 SVG 源码（1024×1024，矢量）。
pub(crate) const FLICK_ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#202136"/>
      <stop offset="1" stop-color="#111322"/>
    </linearGradient>
    <linearGradient id="bolt" x1="0.18" y1="0.05" x2="0.78" y2="0.95">
      <stop offset="0" stop-color="#b34cff"/>
      <stop offset="0.48" stop-color="#6e69ff"/>
      <stop offset="1" stop-color="#38c8ff"/>
    </linearGradient>
    <linearGradient id="glass" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="1" stop-color="#dce8ff"/>
    </linearGradient>
    <radialGradient id="glow" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#70caff" stop-opacity=".34"/>
      <stop offset="1" stop-color="#70caff" stop-opacity="0"/>
    </radialGradient>
    <filter id="softGlow" x="-40%" y="-40%" width="180%" height="180%">
      <feGaussianBlur stdDeviation="26"/>
    </filter>
  </defs>

  <rect x="112" y="112" width="800" height="800" rx="182" fill="url(#tile)"/>
  <ellipse cx="590" cy="590" rx="210" ry="190" fill="url(#glow)" filter="url(#softGlow)"/>

  <!-- Lightning silhouette adjusted to match the wider original PNG -->
  <path d="M 516 226
           L 625 226
           C 650 226 656 241 645 260
           L 568 403
           C 559 419 567 430 584 430
           L 678 434
           C 705 435 713 456 694 477
           L 395 765
           C 373 787 344 778 345 752
           C 345 746 348 739 352 730
           L 447 559
           C 457 540 450 535 431 535
           L 289 535
           C 263 535 256 516 273 494
           L 470 248
           C 482 232 496 226 516 226 Z"
        fill="url(#bolt)"/>

  <circle cx="632" cy="580" r="125" fill="none" stroke="url(#glass)" stroke-width="47"/>
  <path d="M 721 670 L 777 726"
        fill="none" stroke="url(#glass)" stroke-width="47"
        stroke-linecap="round"/>
</svg>"##;

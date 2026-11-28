# Flick

Flick 是一个面向 **Wayland** 的桌面启动器 / 快速搜索面板：按下快捷键唤起一个搜索框，输入即搜索（应用、文件、内置动作与插件提供的动作），回车执行。使用 Rust + [gpui](https://github.com/zed-industries/zed)绘制，插件系统可扩展动作与 Surface。**不支持 X11**

> 当前验证环境：**CachyOS / Wayland / GNOME 51 / KDE / niri**，基本界面显示正常，但 Portal `BindShortcuts`
> 在 niri 环境下需要在 niri 配置中手动绑定 `flick toggle`（见下文）。（窗口优先使用 Wayland layer-shell；合成器不支持时回退为最大化透明窗口）。

## 功能与仓库结构（简）

| 目录 | 说明 |
| --- | --- |
| `crates/flick-daemon` | 守护进程：托盘、全局快捷键、IPC、窗口宿主 |
| `crates/flick-cli` | 命令行客户端（二进制名 `flick`） |
| `crates/flick-domain` | 领域类型（动作、路由、设置、插件清单等） |
| `crates/flick-application` | 协调器、面板状态机、端口定义 |
| `crates/flick-runtime` / `flick-plugin` | 插件注册、动作执行、依赖校验 |
| `crates/flick-builtins` | 内置插件（echo / host / apps） |
| `crates/flick-adapters-linux` | Linux 适配（Portal 快捷键、托盘、host_info、显示器探测） |
| `crates/flick-shell-gpui` / `flick-ui` | gpui 外壳与 UI 组件 |
| `crates/flick-settings` / `flick-data` | 设置系统与持久化 |

## 依赖（CachyOS / Arch）

CachyOS 与 Arch 的包管理一致，以下命令可直接使用 `pacman`。

### 构建依赖

```bash
sudo pacman -S --needed base-devel rust pkg-config libxkbcommon freetype2
```

- `rust` 需要 **≥ 1.85**（本项目使用 Rust 2024 edition）；用 `rustup` 也可。
- `base-devel` 提供 `gcc` / `make` / `pkg-config` 等编译工具链。

### 运行时依赖

```bash
sudo pacman -S --needed wayland libxkbcommon fontconfig \
  vulkan-icd-loader mesa dbus \
  xdg-desktop-portal xdg-desktop-portal-gnome
```

- `mesa`（或 `nvidia-utils`）：GPU 驱动，gpui 通过 Vulkan/GL 渲染。
- `fontconfig`：启动器用 `fc-match` 解析界面字体。
- `xdg-desktop-portal` + `xdg-desktop-portal-gnome`：全局快捷键（GlobalShortcuts）走 Portal。
- GNOME 下托盘图标需要 **AppIndicator 扩展**：`sudo pacman -S --needed gnome-shell-extension-appindicator`
  （否则托盘不显示，可用 `FLICK_DISABLE_TRAY=1` 显式关闭托盘；面板仍可通过快捷键 / `flick show` 唤起）。

## 构建（release）

```bash
git clone <本仓库地址> flick
cd flick

# 编译守护进程与 CLI
cargo build --release --bin flick-daemon --bin flick
```

产物：

- `target/release/flick-daemon`
- `target/release/flick`

## 安装

```bash
# 1) 安装二进制（systemd 服务默认从 ~/.cargo/bin 启动）
install -Dm755 target/release/flick-daemon ~/.cargo/bin/flick-daemon
install -Dm755 target/release/flick        ~/.cargo/bin/flick

# 2) 安装 systemd 用户服务
mkdir -p ~/.config/systemd/user
cp crates/flick-daemon/resources/flick.service ~/.config/systemd/user/

# 3) 把图形会话的环境变量导入 systemd 用户管理器（每次登录会话执行一次）
dbus-update-activation-environment --systemd \
  WAYLAND_DISPLAY XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS \
  XDG_CURRENT_DESKTOP XDG_SESSION_TYPE

# 4) 启用并启动
systemctl --user daemon-reload
systemctl --user enable --now flick.service
```

> 若你更希望装到其他路径，改 `~/.cargo/bin` 即可，但同时要改服务文件里的 `ExecStart`。

## 运行与自检

```bash
flick status           # 守护进程状态
flick --json doctor    # 会话 / Portal / IPC 诊断（排障第一步）
flick show             # 直接显示搜索面板（绕过 Portal 快捷键）
flick hide             # 隐藏
flick toggle           # 切换可见性（绑定桌面自定义快捷键的 IPC 回退方案）
flick shortcuts --help # 全局快捷键配置
flick settings --help  # 设置项列出 / 读取 / 写入 / 重置
```

默认快捷键由 Portal 的 `show-search` 绑定；首次使用可用：

```bash
flick shortcuts configure --show-search "Super+Space"
```

### niri 下的快捷键

niri 中 Portal `BindShortcuts` 通常不可用，启动日志会出现 `Portal BindShortcuts failed`；托盘与搜索面板本身正常。把按键交给 niri 处理即可：

```kdl
binds {
    Alt+Space { spawn "flick" "toggle"; }
}
```

改完执行 `niri msg action reload-config`（或重启 niri）。`flick toggle` 走 IPC，不依赖 Portal；也可用 `flick show` / `flick hide` 分别只显示或隐藏。

环境变量：

- `FLICK_DISABLE_TRAY=1`：禁用系统托盘（无 AppIndicator 扩展的桌面建议设置）。

## 许可证

GPL-3.0-or-later，见 [LICENSE](LICENSE)。

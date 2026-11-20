# 👻 GhostDock

**GhostDock** là ứng dụng điều khiển cửa sổ Windows và ẩn `Alt + Tab` / `Taskbar` siêu nhẹ được viết bằng **Rust** (`eframe`/`egui` + Win32 API thuần). Ứng dụng giúp giữ cho các phần mềm chạy nền (trình duyệt tự động hóa, AI Agent, công cụ treo máy, render, terminal...) luôn ở trạng thái **hoạt động 100% hiệu năng (không bị Windows đóng băng hay ngừng cấp GPU/CPU khi thu nhỏ)** mà vẫn hoàn toàn biến mất khỏi thanh `Taskbar` và danh sách `Alt + Tab`.

---

## ✨ Tính năng chính (Key Features)

### 1. Năm chế độ ẩn cửa sổ thông minh (5 Non-Throttling Stealth Modes — v0.2.0)
Khác với thao tác Minimize thông thường (khiến Windows đặt cờ `IsIconic = 1` và ngừng vẽ giao diện / giảm xung tiến trình), **GhostDock** giữ cửa sổ ở trạng thái `WS_VISIBLE` không thu nhỏ nhưng loại bỏ hoàn toàn sự vướng víu trên màn hình:

- **`1px` (`CornerPeek` — Mặc định, Desktop-free & OBS OK):** Gỡ nút Taskbar qua COM `ITaskbarList::DeleteTab`, gắn cờ `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT` (click xuyên qua) và neo gọn 1 pixel ở góc dưới bên phải màn hình mà không bật `WS_EX_TOOLWINDOW`. Cửa sổ biến mất hoàn toàn khỏi cả màn hình Desktop trống, thanh Taskbar và `Alt + Tab`, nhưng **OBS Studio / TikTok Live Studio vẫn nhìn thấy và chọn được trong danh sách Window Capture**.
- **`Ghost` (`LayeredGhost` — Alpha 0 Invisible):** Đặt lớp trong suốt `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE` với `LWA_ALPHA = 0`, gỡ nút Taskbar và ghim xuống `HWND_BOTTOM`. Tàng hình 100% và cho phép click xuyên qua ngay cả trên màn hình Desktop trống, tương thích với các trình quay `BitBlt` / `PrintWindow`.
- **`Live` (`TaskbarCapturable` — WGC OK):** Giữ nguyên kích thước khung hình thực phía sau các cửa sổ đang làm việc (`HWND_BOTTOM` + `WS_EX_NOACTIVATE` + `ITaskbarList::DeleteTab`), tương thích 100% với **Windows Graphics Capture (WGC)**, `BitBlt` và bộ chọn nguồn quay của OBS / TikTok Live Studio.
- **`Stealth` (`StealthBackground` — Strict ToolWindow):** Chế độ ẩn nghiêm ngặt gắn thêm cờ `WS_EX_TOOLWINDOW` + `WS_EX_NOACTIVATE` + `ITaskbarList::DeleteTab` + `HWND_BOTTOM`.
- **`Monitor 2` (`SecondaryMonitor`):** Tự động di chuyển cửa sổ sang màn hình phụ hoặc màn hình ảo (`IddCx`) ở trạng thái mở hoàn toàn không bị che khuất (un-occluded), đồng thời ẩn nút Taskbar và khóa chiếm focus.

### 2. Động cơ bảo vệ thời gian thực `SetWinEventHook` (`WinEventGuard` — Mới trong v0.2.0)
- Chạy luồng giám sát sự kiện hệ thống thời gian thực (`EVENT_SYSTEM_FOREGROUND`, `EVENT_SYSTEM_MINIMIZESTART..END`, `EVENT_OBJECT_SHOW..REORDER`, `EVENT_OBJECT_STATECHANGE..LOCATIONCHANGE`, `TaskbarCreated`, `WM_DISPLAYCHANGE`):
  - **Chống đóng băng khi bấm `Win + D` (Show Desktop):** Tự động đánh thức tức thì các cửa sổ đang ẩn bằng `ShowWindowAsync(SW_SHOWNOACTIVATE)` ngay khi Windows cố thu nhỏ chúng, giữ luồng render/stream không bao giờ bị đứng hình.
  - **Chống cướp Focus (`Focus-Cascade Bounce`):** Tự động trả focus về cửa sổ làm việc trước đó thông qua `AttachThreadInput` nếu cửa sổ ẩn cố tự nhảy lên tiền cảnh.
  - **Khóa cứng Z-Order & Tự động thích ứng đa màn hình / Explorer Restart:** Giữ cửa sổ ẩn luôn nằm dưới các cửa sổ bình thường (`HWND_BOTTOM`), tự tính lại tọa độ khi đổi độ phân giải/DPI và tự ẩn lại nút Taskbar nếu `explorer.exe` khởi động lại.

### 3. Chống giảm hiệu năng trình duyệt chạy ngầm (`No-Occ` / Anti-Occlusion)
- Nút **`No-Occ`** cho phép khởi chạy lại nhanh các trình duyệt/ứng dụng nhân Chromium & Electron kèm bộ cờ vô hiệu hóa cơ chế bóp băng thông cửa sổ bị che khuất:
  `--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding`
- Giúp WebGL, Canvas, WebRTC, hoặc các tác vụ tự động hóa trên trình duyệt luôn chạy ở tốc độ tối đa ngay cả khi bị cửa sổ khác đè lên.

### 4. Khay hệ thống & Phím tắt toàn cục (System Tray & Global Hotkey)
- Chạy ngầm gọn nhẹ dưới khay hệ thống (System Tray) với giao diện Dark Mode chuẩn DWM (`#0B0B0C`).
- Bấm tổ hợp phím **`Ctrl + Shift + Space`** ở bất kỳ đâu (hoặc nhấp chuột trái vào biểu tượng dưới khay hệ thống) để bật/tắt tức thì bảng điều khiển GhostDock.

### 5. Cơ chế chống mồ côi cửa sổ (Zero-Orphan Automatic Restore)
- Lưu trữ chính xác tọa độ gốc (`RECT`), trạng thái phóng to (`IsZoomed`) và cờ hiển thị (`GWL_EXSTYLE`) của từng cửa sổ.
- Khi bấm **`Restore All`** hoặc thoát chương trình (**`Quit (Restore All)`**), toàn bộ cửa sổ đang ẩn sẽ tự động trở về nguyên trạng ban đầu 100%, không bao giờ để sót cửa sổ bị ẩn ngầm.

### 6. Gom nhóm theo tiến trình & Hỗ trợ nâng quyền Admin (Process Grouping & Elevation)
- Tự động gom các cửa sổ theo tiến trình sở hữu (`.exe` / `PID`) kèm bộ đệm metadata tối ưu (`ProcessMetaCache`) không gây tốn CPU.
- Hỗ trợ lọc/tìm kiếm tức thì theo tên cửa sổ hoặc tên file thực thi, ẩn/hiện hàng loạt cửa sổ của cùng một ứng dụng chỉ với 1 cú nhấp (`Hide all` / `Show all`).
- Tự động phát hiện các cửa sổ chạy quyền Administrator (`ADMIN` badge) và cung cấp nút **`Elevate`** nâng quyền UAC trực tiếp, đồng bộ khóa đơn phiên (`Single-Instance Lock`) an toàn qua biên giới quyền UIPI.

---

## 📦 Tải về & Sử dụng (Download & Quick Start)

### Cách 1: Tải bản thực thi đóng gói sẵn (Khuyên dùng cho người dùng cuối)
1. Truy cập mục **[Releases](../../releases/latest)** ở cột bên phải của repository.
2. Tải gói **`GhostDock-windows-x64.zip`** (hoặc `ghost_dock.exe`).
3. Giải nén và chạy trực tiếp `ghost_dock.exe` (Portable 1 file duy nhất, không cần cài đặt).

### Cách 2: Tự biên dịch từ mã nguồn (Build from Source)
Yêu cầu: **Rust Toolchain** (`rustc 1.85+` hỗ trợ Edition 2024) trên Windows 10/11 (x64).

```powershell
# 1. Clone repository
git clone https://github.com/kumalalalala/ghost_dock.git
cd ghost_dock

# 2. Biên dịch bản Release tối ưu (LTO + Strip)
cargo build --release

# 3. Chạy thử nghiệm kiểm tra tích hợp Win32
cargo test

# File thực thi được tạo tại: target\release\ghost_dock.exe
```

---

## 🗂️ Cấu trúc mã nguồn (Project Architecture)

```text
ghost_dock/
├── Cargo.toml                 # Khai báo dependencies (eframe, windows, tray-icon, global-hotkey)
├── build.rs                   # Tự động sinh multi-res ICO và nhúng Windows Manifest (PerMonitorV2 DPI)
├── src/
│   ├── main.rs                # Điểm vào ứng dụng, kiểm tra Single-Instance & khởi tạo viewport
│   ├── lib.rs                 # Khai báo module thư viện
│   ├── app.rs                 # Giao diện bảng điều khiển egui và vòng lặp đồng bộ trạng thái
│   ├── icon.rs                # Bộ dựng biểu tượng vector GhostDock bằng thuật toán SDF thuần
│   ├── single_instance.rs     # Khóa đơn phiên Named Mutex + Event đánh thức cửa sổ xuyên UIPI
│   ├── tray.rs                # Quản lý System Tray menu và phím tắt toàn cục Ctrl+Shift+Space
│   ├── ui/
│   │   ├── mod.rs
│   │   └── theme.rs           # Hệ thống màu sắc Obsidian Dark, nút pill chuyển động mượt
│   └── win32/
│       ├── mod.rs             # Định nghĩa 5 chế độ HideStrategy, WindowState, ProcessGroup
│       ├── controller.rs      # Lõi điều khiển HWND, ẩn/khôi phục cửa sổ, DWM dark mode, No-Occlusion
│       ├── guard.rs           # Động cơ SetWinEventHook thời gian thực chống Win+D, focus cascade & Explorer restart
│       ├── enumerator.rs      # Liệt kê cửa sổ chuẩn Alt+Tab với ProcessMetaCache theo PID
│       └── monitors.rs        # Nhận diện đa màn hình & tính toán vị trí Corner 1px / Monitor 2
└── tests/
    └── win32_stealth_realtest.rs # Bộ kiểm thử tự động kiểm chứng chu kỳ ẩn/khôi phục cửa sổ thật
```

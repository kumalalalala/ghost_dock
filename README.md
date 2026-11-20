# 👻 GhostDock

**GhostDock** là ứng dụng điều khiển cửa sổ Windows và ẩn `Alt + Tab` / `Taskbar` siêu nhẹ được viết bằng **Rust** (`eframe`/`egui` + Win32 API thuần). Ứng dụng giúp giữ cho các phần mềm chạy nền (trình duyệt tự động hóa, AI Agent, công cụ treo máy, render, terminal...) luôn ở trạng thái **hoạt động 100% hiệu năng (không bị Windows đóng băng hay ngừng cấp GPU/CPU khi thu nhỏ)** mà vẫn hoàn toàn biến mất khỏi thanh `Taskbar` và danh sách `Alt + Tab`.

---

## ✨ Tính năng chính (Key Features)

### 1. Ba chế độ ẩn cửa sổ thông minh (3 Non-Throttling Stealth Modes)
Khác với thao tác Minimize thông thường (khiến Windows đặt cờ `IsIconic = 1` và ngừng vẽ giao diện / giảm xung tiến trình), **GhostDock** giữ cửa sổ ở trạng thái `WS_VISIBLE` không thu nhỏ nhưng loại bỏ hoàn toàn sự vướng víu trên màn hình:

- **`Stealth` (Stealth Background):** Gỡ cửa sổ khỏi `Alt + Tab` và `Taskbar` (chuyển kiểu sang `WS_EX_TOOLWINDOW`), đồng thời đưa cửa sổ xuống lớp dưới cùng (`HWND_BOTTOM`). Cửa sổ biến mất khỏi khu vực làm việc nhưng tiến trình vẫn chạy như đang mở bình thường.
- **`Corner 1px` (Corner Peek):** Ẩn khỏi `Alt + Tab` & `Taskbar` đồng thời neo cửa sổ về góc 1 pixel ở mép màn hình, đảm bảo bộ lập lịch DWM của Windows luôn ghi nhận cửa sổ nằm trong vùng hiển thị.
- **`Monitor 2` (Secondary / Virtual Monitor):** Tự động di chuyển cửa sổ sang màn hình phụ hoặc màn hình ảo (`IddCx`) ở trạng thái mở hoàn toàn không bị che khuất (un-occluded), đồng thời ẩn khỏi `Alt + Tab` và `Taskbar`.

### 2. Chống giảm hiệu năng trình duyệt chạy ngầm (`No-Occ` / Anti-Occlusion)
- Nút **`No-Occ`** cho phép khởi chạy lại nhanh các trình duyệt/ứng dụng nhân Chromium & Electron kèm bộ cờ vô hiệu hóa cơ chế bóp băng thông cửa sổ bị che khuất:
  `--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows --disable-renderer-backgrounding`
- Giúp WebGL, Canvas, WebRTC, hoặc các tác vụ tự động hóa trên trình duyệt luôn chạy ở tốc độ tối đa ngay cả khi bị cửa sổ khác đè lên.

### 3. Khay hệ thống & Phím tắt toàn cục (System Tray & Global Hotkey)
- Chạy ngầm gọn nhẹ dưới khay hệ thống (System Tray) với giao diện Dark Mode chuẩn DWM (`#0B0B0C`).
- Bấm tổ hợp phím **`Ctrl + Shift + Space`** ở bất kỳ đâu (hoặc nhấp chuột trái vào biểu tượng dưới khay hệ thống) để bật/tắt tức thì bảng điều khiển GhostDock.

### 4. Cơ chế chống mồ côi cửa sổ (Zero-Orphan Automatic Restore)
- Lưu trữ chính xác tọa độ gốc (`RECT`), trạng thái phóng to (`IsZoomed`) và cờ hiển thị (`GWL_EXSTYLE`) của từng cửa sổ.
- Khi bấm **`Restore All`** hoặc thoát chương trình (**`Quit (Restore All)`**), toàn bộ cửa sổ đang ẩn sẽ tự động trở về nguyên trạng ban đầu 100%, không bao giờ để sót cửa sổ bị ẩn ngầm.

### 5. Gom nhóm theo tiến trình & Hỗ trợ nâng quyền Admin (Process Grouping & Elevation)
- Tự động gom các cửa sổ theo tiến trình sở hữu (`.exe` / `PID`) kèm bộ đệm metadata tối ưu (`ProcessMetaCache`) không gây tốn CPU.
- Hỗ trợ lọc/tìm kiếm tức thì theo tên cửa sổ hoặc tên file thực thi, ẩn/hiện hàng loạt cửa sổ của cùng một ứng dụng chỉ với 1 cú nhấp (`Hide all` / `Show all`).
- Tự động phát hiện các cửa sổ chạy quyền Administrator (`ADMIN` badge) và cung cấp nút **`Elevate`** nâng quyền UAC trực tiếp, đồng bộ khóa đơn phiên (`Single-Instance Lock`) an toàn qua biên giới quyền UIPI.

---

## 📦 Tải về & Sử dụng (Download & Quick Start)

### Cách 1: Tải bản thực thi đóng gói sẵn (Khuyên dùng cho người dùng cuối)
1. Truy cập mục **[Releases](../../releases/latest)** ở cột bên phải của repository.
2. Tải gói **`GhostDock-v0.1.0-windows-x64.zip`** (hoặc `ghost_dock.exe`).
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
│       ├── mod.rs             # Định nghĩa HideStrategy, WindowState, ProcessGroup
│       ├── controller.rs      # Lõi điều khiển HWND, ẩn/khôi phục cửa sổ, DWM dark mode, No-Occlusion
│       ├── enumerator.rs      # Liệt kê cửa sổ chuẩn Alt+Tab với ProcessMetaCache theo PID
│       └── monitors.rs        # Nhận diện đa màn hình & tính toán vị trí Corner 1px / Monitor 2
└── tests/
    └── win32_stealth_realtest.rs # Bộ kiểm thử tự động kiểm chứng chu kỳ ẩn/khôi phục cửa sổ thật
```

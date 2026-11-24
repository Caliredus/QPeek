//! Windows'a özgü işler: tek örnek (single instance), AppUserModelID,
//! ekran çalışma alanı / ortalama, odak takibi, temalı başlık çubuğu ve başlangıç (autostart).

use std::ffi::c_void;

use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, HWND, POINT, RECT,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateEllipticRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, RGN_DIFF, RGN_ERROR, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_STATE, NIF_TIP, NIIF_LARGE_ICON, NIIF_USER, NIM_ADD, NIM_DELETE, NIS_HIDDEN,
    NOTIFYICONDATAW, SetCurrentProcessExplicitAppUserModelID, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, CreateWindowExW, DestroyIcon, DestroyWindow, GetCursorPos,
    GetForegroundWindow, GetMessageW, HICON, IMAGE_ICON, IsIconic, LR_DEFAULTCOLOR, LoadImageW, MSG, SW_RESTORE,
    SetForegroundWindow, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY,
};

const RUN_SUBKEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("QPeek");

/// Görev çubuğuna sabitlenen ikon ile çalışan uygulama penceresini aynı düğmede gruplar.
pub fn set_app_id() {
    // SAFETY: geçerli statik UTF-16 kimlik.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("Callidus.QPeek"));
    }
}

/// Slint penceresinin Win32 tutamacı (HWND). Pencere henüz oluşmadıysa None.
pub fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?;
    match raw.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// İmlecin konumu ve o ekranın görev çubuğu hariç alanı (fiziksel piksel).
#[derive(Clone, Copy)]
pub struct CursorScreen {
    pub cursor_x: i32,
    pub work: RECT,
    pub screen: RECT,
}

pub fn cursor_screen() -> Option<CursorScreen> {
    // SAFETY: yalnız yerel yapılara yazan, belgelenmiş Win32 çağrıları.
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        Some(CursorScreen { cursor_x: pt.x, work: info.rcWork, screen: info.rcMonitor })
    }
}

pub fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: salt okuma.
    unsafe { GetForegroundWindow() == hwnd }
}

pub fn bring_to_front(hwnd: HWND) {
    // SAFETY: geçerli HWND; simge durumundaysa geri getirir, ardından öne alır.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Windows 11 yuvarlak köşeleri ve temanın kart kenarlığı renginde ince çerçeve.
pub fn set_rounded_border(hwnd: HWND, border_rgb: (u8, u8, u8)) {
    let pref = DWMWCP_ROUND;
    let (r, g, b) = border_rgb;
    let border = COLORREF(u32::from_le_bytes([r, g, b, 0]));
    // SAFETY: değerler çağrı süresince yaşayan yerel değişkenler.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&pref).cast(),
            std::mem::size_of_val(&pref) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::from_ref(&border).cast(),
            std::mem::size_of_val(&border) as u32,
        );
    }
}

/// Hides the DWM border so it cannot cross a window's cutouts.
pub fn clear_window_border(hwnd: HWND) {
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    // SAFETY: the attribute reads a u32 that lives through the call.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::from_ref(&DWMWA_COLOR_NONE).cast(),
            std::mem::size_of_val(&DWMWA_COLOR_NONE) as u32,
        );
    }
}

/// Clips a frameless window to rounded corners and two ticket notches in physical pixels.
pub fn apply_ticket_cutout(hwnd: HWND, width: i32, height: i32, seam_x: i32, notch_r: i32, corner_r: i32) {
    if width <= 0 || height <= 0 || notch_r <= 0 {
        return;
    }

    let left = seam_x - notch_r;
    let right = seam_x + notch_r + 1;
    let notch_bounds = [(-notch_r, notch_r + 1), (height - notch_r, height + notch_r + 1)];

    // SAFETY: each GDI operation uses valid region handles. SetWindowRgn takes ownership on success.
    unsafe {
        let base = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner_r * 2, corner_r * 2);
        if base.is_invalid() {
            return;
        }

        for (top, bottom) in notch_bounds {
            let notch = CreateEllipticRgn(left, top, right, bottom);
            if notch.is_invalid() {
                let _ = DeleteObject(base.into());
                return;
            }
            let result = CombineRgn(Some(base), Some(base), Some(notch), RGN_DIFF);
            let _ = DeleteObject(notch.into());
            if result == RGN_ERROR {
                let _ = DeleteObject(base.into());
                return;
            }
        }

        if SetWindowRgn(hwnd, Some(base), true) == 0 {
            let _ = DeleteObject(base.into());
        }
    }
}

/// Başlık çubuğunun modunu, arka planını ve metin rengini temayla eşler.
pub fn set_titlebar_colors(hwnd: HWND, dark: bool, bg: (u8, u8, u8), text: (u8, u8, u8)) {
    let colorref = |(r, g, b)| u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
    let immersive_dark = u32::from(dark);
    let caption = colorref(bg);
    let text = colorref(text);
    // SAFETY: geçerli HWND ve çağrı süresince yaşayan yerel değişkenler.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&immersive_dark).cast(),
            std::mem::size_of_val(&immersive_dark) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            std::ptr::from_ref(&caption).cast(),
            std::mem::size_of_val(&caption) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            std::ptr::from_ref(&text).cast(),
            std::mem::size_of_val(&text) as u32,
        );
    }
}

pub enum Instance {
    /// İlk örnek: ikinci bir QPeek açılınca `on_open_app` çağrılır.
    Primary,
    /// Zaten çalışan bir QPeek var; bu örnek kapanmalı.
    Secondary,
}

/// Masaüstü kısayolundan veya sabitlenmiş görev çubuğu ikonundan ikinci bir QPeek başlatılınca,
/// çalışan ilk örneğe ortadaki uygulama penceresini açması için sinyal gönderip kapanır.
/// `--tray` ile açılan ikinci örnek sessizce çıkar.
pub fn single_instance(start_hidden: bool, on_open_app: impl Fn() + Send + 'static) -> Instance {
    // SAFETY: adlandırılmış olay oluşturma; tutamaç süreç ömrü boyunca açık kalır.
    unsafe {
        let Ok(event) = CreateEventW(None, false, false, w!("Local\\QPeek.OpenApp")) else {
            return Instance::Primary;
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if !start_hidden {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = SetEvent(event);
            }
            return Instance::Secondary;
        }
        let raw = event.0 as isize;
        std::thread::spawn(move || {
            let event = HANDLE(raw as *mut c_void);
            loop {
                WaitForSingleObject(event, INFINITE);
                on_open_app();
            }
        });
        Instance::Primary
    }
}

/// Windows açılışında tepside başlatma kaydı (HKCU\...\Run\QPeek) etkin mi?
pub fn autostart_enabled() -> bool {
    // SAFETY: salt okuma Registry çağrıları; anahtar kapatılır.
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, Some(0), KEY_QUERY_VALUE, &mut hkey).is_err() {
            return false;
        }
        let exists = RegQueryValueExW(hkey, RUN_VALUE, None, None, None, None).is_ok();
        let _ = RegCloseKey(hkey);
        exists
    }
}

/// Windows açılışında `--tray` ile otomatik başlatmayı açar veya kapatır.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Uygulama yolu okunamadı: {e}"))?;
    let cmd = format!("\"{}\" --tray", exe.display());
    let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: geçerli HKCU Run anahtarı ve null-sonlu UTF-16 tampon; anahtar kapatılır.
    unsafe {
        let mut hkey = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, Some(0), KEY_SET_VALUE, &mut hkey)
            .ok()
            .map_err(|e| format!("Başlangıç anahtarı açılamadı: {e}"))?;
        let res = if enabled {
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(hkey, RUN_VALUE, Some(0), REG_SZ, Some(bytes))
                .ok()
                .map_err(|e| format!("Başlangıç kaydı yazılamadı: {e}"))
        } else {
            let err = RegDeleteValueW(hkey, RUN_VALUE);
            if err.is_ok() || err == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("Başlangıç kaydı silinemedi: {err:?}"))
            }
        };
        let _ = RegCloseKey(hkey);
        res
    }
}

/// Global klavye kısayolu (`Alt + Shift + Q`): ayrı iş parçacığında çekirdek uyku (`GetMessageW`)
/// ile bekler, boşta %0 CPU tüketir.
pub fn spawn_hotkey(on_trigger: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        // SAFETY: bu iş parçacığına bağlı hotkey kaydı ve standart mesaj döngüsü.
        unsafe {
            if RegisterHotKey(None, 1, MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'Q')).is_err() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                    on_trigger();
                }
            }
        }
    });
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let mut i = 0;
    for u in text.encode_utf16() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = u;
        i += 1;
    }
    if i < dst.len() {
        dst[i] = 0;
    }
}

/// Doğal Windows 11 bildirimi (`Action Center` toast).
/// Gizli (`NIS_HIDDEN`) geçici bildirim ikonu kullanır; tepside ikinci ikon açmaz.
pub fn send_toast(title: String, body: String) {
    std::thread::spawn(move || {
        // SAFETY: kendi geçici penceresi ve yerel NOTIFYICONDATAW yapısıyla Win32 çağrıları;
        // işlem sonunda ikon ve pencere temizlenir.
        unsafe {
            let hinst = GetModuleHandleW(None).ok().map(Into::into);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("QPeekNotify"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                hinst,
                None,
            ) else {
                return;
            };
            let hicon = LoadImageW(
                hinst,
                PCWSTR(std::ptr::without_provenance::<u16>(1)),
                IMAGE_ICON,
                48,
                48,
                LR_DEFAULTCOLOR,
            )
                .ok()
                .map(|h| HICON(h.0))
                .unwrap_or_default();

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 0x5150,
                uFlags: NIF_ICON | NIF_TIP | NIF_STATE | NIF_INFO,
                dwState: NIS_HIDDEN,
                dwStateMask: NIS_HIDDEN,
                hIcon: hicon,
                hBalloonIcon: hicon,
                dwInfoFlags: if hicon.is_invalid() {
                    NIIF_USER
                } else {
                    NIIF_USER | NIIF_LARGE_ICON
                },
                ..Default::default()
            };
            copy_wide(&mut nid.szTip, "QPeek");
            copy_wide(&mut nid.szInfoTitle, &title);
            copy_wide(&mut nid.szInfo, &body);

            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            // Windows 11 bildirimi Action Center'a teslim ettikten sonra geçici tutamacı kaldır.
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            if !hicon.is_invalid() {
                let _ = DestroyIcon(hicon);
            }
            let _ = DestroyWindow(hwnd);
        }
    });
}

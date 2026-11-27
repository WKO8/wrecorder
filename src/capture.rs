//! Descoberta de alvos de captura (monitores e janelas) por plataforma.

use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Garante largura/altura pares (exigência do H.264 4:2:0).
    pub fn even(self) -> Rect {
        Rect { w: (self.w & !1).max(2), h: (self.h & !1).max(2), ..self }
    }

    pub fn parse(s: &str) -> Option<Rect> {
        let v: Vec<i64> = s
            .split([',', 'x', ' ', ':'])
            .filter(|p| !p.is_empty())
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        if v.len() != 4 || v[2] <= 0 || v[3] <= 0 {
            return None;
        }
        Some(Rect { x: v[0] as i32, y: v[1] as i32, w: v[2] as u32, h: v[3] as u32 })
    }
}

impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} em ({}, {})", self.w, self.h, self.x, self.y)
    }
}

#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
    pub primary: bool,
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// X11: window id. Windows: HWND.
    pub id: u64,
    pub title: String,
    pub app: String,
    #[allow(dead_code)]
    pub rect: Rect,
}

impl WindowInfo {
    pub fn label(&self) -> String {
        if self.app.is_empty() {
            self.title.clone()
        } else {
            format!("{}  —  [{}]", self.title, self.app)
        }
    }
}

/// O que gravar na tela.
#[derive(Debug, Clone)]
pub enum VideoTarget {
    /// Tela inteira: um monitor específico ou (None) toda a área de trabalho.
    Screen(Option<Monitor>),
    /// Área retangular em coordenadas absolutas da área de trabalho.
    Region(Rect),
    /// Uma janela (aplicativo, navegador...).
    Window(WindowInfo),
}

// ---------------------------------------------------------------- Linux / X11
#[cfg(target_os = "linux")]
pub mod x11 {
    use super::*;
    use anyhow::Context;
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};

    fn atom(conn: &impl Connection, name: &str) -> Result<u32> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    }

    pub fn monitors() -> Result<Vec<Monitor>> {
        let (conn, screen_num) = x11rb::connect(None).context("conectando ao servidor X11")?;
        let root = conn.setup().roots[screen_num].root;
        let reply = conn.randr_get_monitors(root, true)?.reply()?;
        let mut out = Vec::new();
        for m in reply.monitors {
            let name = conn
                .get_atom_name(m.name)?
                .reply()
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_else(|_| "Monitor".into());
            out.push(Monitor {
                name,
                rect: Rect { x: m.x as i32, y: m.y as i32, w: m.width as u32, h: m.height as u32 },
                primary: m.primary,
            });
        }
        if out.is_empty() {
            let s = &conn.setup().roots[screen_num];
            out.push(Monitor {
                name: "Tela".into(),
                rect: Rect { x: 0, y: 0, w: s.width_in_pixels as u32, h: s.height_in_pixels as u32 },
                primary: true,
            });
        }
        Ok(out)
    }

    fn prop_string(conn: &impl Connection, win: Window, prop: u32, ty: u32) -> Option<String> {
        let r = conn.get_property(false, win, prop, ty, 0, 1024).ok()?.reply().ok()?;
        if r.value.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&r.value).trim_end_matches('\0').to_string())
    }

    pub fn windows() -> Result<Vec<WindowInfo>> {
        let (conn, screen_num) = x11rb::connect(None).context("conectando ao servidor X11")?;
        let root = conn.setup().roots[screen_num].root;
        let client_list = atom(&conn, "_NET_CLIENT_LIST")?;
        let net_wm_name = atom(&conn, "_NET_WM_NAME")?;
        let utf8 = atom(&conn, "UTF8_STRING")?;
        let reply = conn.get_property(false, root, client_list, AtomEnum::WINDOW, 0, 4096)?.reply()?;
        let ids: Vec<u32> = reply.value32().map(|it| it.collect()).unwrap_or_default();

        let mut out = Vec::new();
        for id in ids {
            let title = prop_string(&conn, id, net_wm_name, utf8)
                .or_else(|| prop_string(&conn, id, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            // WM_CLASS = "instance\0Class\0"
            let app = prop_string(&conn, id, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
                .and_then(|c| c.split('\0').nth(1).map(str::to_string))
                .unwrap_or_default();
            let Ok(geo) = conn.get_geometry(id)?.reply() else { continue };
            let Ok(tr) = conn.translate_coordinates(id, root, 0, 0)?.reply() else { continue };
            if geo.width < 8 || geo.height < 8 {
                continue;
            }
            out.push(WindowInfo {
                id: id as u64,
                title,
                app,
                rect: Rect { x: tr.dst_x as i32, y: tr.dst_y as i32, w: geo.width as u32, h: geo.height as u32 },
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------- Windows
#[cfg(windows)]
pub mod win {
    use super::*;
    use std::mem::{size_of, zeroed};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow,
        IsWindowVisible, GW_OWNER, MONITORINFOF_PRIMARY,
    };

    /// Coordenadas em pixels físicos (necessário com escala de DPI ≠ 100%).
    pub fn set_dpi_aware() {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    fn wide_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    pub fn monitors() -> Result<Vec<Monitor>> {
        unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let out = &mut *(data as *mut Vec<Monitor>);
            let mut info: MONITORINFOEXW = zeroed();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(h, &mut info as *mut _ as *mut _) != 0 {
                let r = info.monitorInfo.rcMonitor;
                out.push(Monitor {
                    name: wide_to_string(&info.szDevice).trim_start_matches(r"\\.\").to_string(),
                    rect: Rect { x: r.left, y: r.top, w: (r.right - r.left) as u32, h: (r.bottom - r.top) as u32 },
                    primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
            TRUE
        }
        let mut out: Vec<Monitor> = Vec::new();
        unsafe {
            EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(cb), &mut out as *mut _ as LPARAM);
        }
        Ok(out)
    }

    pub fn window_title(hwnd: u64) -> Option<String> {
        unsafe {
            let h = hwnd as HWND;
            if IsWindow(h) == 0 {
                return None;
            }
            let len = GetWindowTextLengthW(h);
            if len <= 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize + 1];
            GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
            Some(wide_to_string(&buf))
        }
    }

    pub fn windows() -> Result<Vec<WindowInfo>> {
        unsafe extern "system" fn cb(h: HWND, data: LPARAM) -> BOOL {
            let out = &mut *(data as *mut Vec<WindowInfo>);
            if IsWindowVisible(h) == 0 || IsIconic(h) != 0 || !GetWindow(h, GW_OWNER).is_null() {
                return TRUE;
            }
            let mut cloaked: u32 = 0;
            DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut cloaked as *mut _ as *mut _, 4);
            if cloaked != 0 {
                return TRUE;
            }
            let Some(title) = window_title(h as u64) else { return TRUE };
            let mut r: RECT = zeroed();
            DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut r as *mut _ as *mut _,
                size_of::<RECT>() as u32,
            );
            let (w, hgt) = (r.right - r.left, r.bottom - r.top);
            if w < 8 || hgt < 8 || title == "Program Manager" {
                return TRUE;
            }
            out.push(WindowInfo {
                id: h as u64,
                title,
                app: String::new(),
                rect: Rect { x: r.left, y: r.top, w: w as u32, h: hgt as u32 },
            });
            TRUE
        }
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            EnumWindows(Some(cb), &mut out as *mut _ as LPARAM);
        }
        Ok(out)
    }
}

/// Lista monitores (X11 / Windows). No Wayland quem escolhe é o portal do sistema.
pub fn monitors() -> Result<Vec<Monitor>> {
    #[cfg(target_os = "linux")]
    return x11::monitors();
    #[cfg(windows)]
    return win::monitors();
    #[allow(unreachable_code)]
    Ok(Vec::new())
}

pub fn windows() -> Result<Vec<WindowInfo>> {
    #[cfg(target_os = "linux")]
    return x11::windows();
    #[cfg(windows)]
    return win::windows();
    #[allow(unreachable_code)]
    Ok(Vec::new())
}

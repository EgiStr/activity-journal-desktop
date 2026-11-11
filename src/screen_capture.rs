//! The Windows side of the screenshot daemon: the foreground window, free disk space, and a PNG
//! of every attached screen (the virtual desktop, like Pillow's `ImageGrab.grab(all_screens=True)`).
//! The only module that calls Win32.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::screenshot_rules::ActiveWindow;

const MEGABYTE: u64 = 1024 * 1024;

/// The foreground window's title and the file name of the executable that owns it.
pub fn active_window() -> ActiveWindow {
    // SAFETY: plain Win32 queries on the current foreground window; every buffer is sized and owned here.
    unsafe {
        let window = GetForegroundWindow();
        let mut title = [0u16; 512];
        let length = GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        let mut process_id = 0u32;
        GetWindowThreadProcessId(window, &mut process_id);
        let mut process_name = String::new();
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if !process.is_null() {
            let mut path = [0u16; 1024];
            let mut size = path.len() as u32;
            if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut size) != 0 {
                let full = String::from_utf16_lossy(&path[..size as usize]);
                process_name = full.rsplit(['\\', '/']).next().unwrap_or_default().to_owned();
            }
            CloseHandle(process);
        }
        ActiveWindow {
            process_name,
            title: String::from_utf16_lossy(&title[..length]),
        }
    }
}

/// Free megabytes on the volume holding `path`, measured at its nearest existing ancestor.
pub fn free_megabytes(path: &Path) -> Result<u64, String> {
    let mut probe = path;
    while !probe.exists() {
        probe = probe
            .parent()
            .ok_or_else(|| format!("no existing ancestor of {}", path.display()))?;
    }
    let wide: Vec<u16> = OsStr::new(probe).encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the other outputs may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!("cannot read free space of {}", probe.display()));
    }
    Ok(available / MEGABYTE)
}

/// Grab the whole virtual desktop and encode it as PNG.
pub fn capture_png() -> Result<Vec<u8>, String> {
    let (width, height, rgba) = grab_virtual_screen()?;
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(encoded)
}

fn grab_virtual_screen() -> Result<(u32, u32, Vec<u8>), String> {
    // SAFETY: standard GDI screen copy. Every DC and bitmap created here is released before return.
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err("no screen to capture".to_owned());
        }
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap);
        let copied = BitBlt(memory, 0, 0, width, height, screen, x, y, SRCCOPY | CAPTUREBLT);

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height; // top-down rows
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(memory, previous);
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);

        if copied == 0 || rows != height {
            return Err("the screen could not be copied".to_owned());
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2); // BGRA -> RGBA
            pixel[3] = 255;
        }
        Ok((width as u32, height as u32, pixels))
    }
}

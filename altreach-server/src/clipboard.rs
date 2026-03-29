use anyhow::Result;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::Foundation::*;

pub fn get_clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }

        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;

        let ptr = GlobalLock(HGLOBAL(handle.0 as *mut _));
        if ptr.is_null() {
            CloseClipboard().ok()?;
            return None;
        }

        let mut len = 0;
        while *((ptr as *const u16).add(len)) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr as *const u16, len);
        let result = String::from_utf16_lossy(slice);


        GlobalUnlock(HGLOBAL(handle.0 as *mut _)).ok();
        CloseClipboard().ok();

        Some(result)
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return Ok(());
        }

        EmptyClipboard()?;

        let alloc = GlobalAlloc(GMEM_MOVEABLE, (text.encode_utf16().count() + 1) * 2)?;

        let ptr = GlobalLock(alloc);
        if ptr.is_null() {
            CloseClipboard().ok();
            return Ok(());
        }

        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr as *mut u16, utf16.len());

        GlobalUnlock(alloc).ok();
        SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(alloc.0))?;
        CloseClipboard().ok();
        Ok(())
    }
}

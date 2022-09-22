use crate::{Error, SystrayEvent};
use std;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::{channel, Sender};
use std::{mem, thread};
use windows_sys::core::{GUID, PCWSTR};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{HBITMAP, HBRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetCursorPos, GetMenuItemID, GetMessageW, InsertMenuItemW, LoadCursorW, LoadIconW, LoadImageW,
    LookupIconIdFromDirectoryEx, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetMenuInfo, TrackPopupMenu, TranslateMessage, CW_USEDEFAULT, HICON,
    HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LR_LOADFROMFILE, MENUINFO, MENUITEMINFOW,
    MFT_SEPARATOR, MFT_STRING, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIM_APPLYTOSUBMENUS,
    MIM_STYLE, MNS_NOTIFYBYPOS, MSG, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_DESTROY, WM_LBUTTONUP,
    WM_MENUCOMMAND, WM_QUIT, WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

// Got this idea from glutin. Yay open source! Boo stupid winproc! Even more boo
// doing SetLongPtr tho.
thread_local!(static WININFO_STASH: RefCell<Option<WindowsLoopData>> = RefCell::new(None));

fn to_wstring(str: &str) -> Vec<u16> {
    OsStr::new(str)
        .encode_wide()
        .chain(Some(0).into_iter())
        .collect::<Vec<_>>()
}

#[derive(Clone)]
struct WindowInfo {
    pub hwnd: HWND,
    pub hinstance: HINSTANCE,
    pub hmenu: HMENU,
}

unsafe impl Send for WindowInfo {}
unsafe impl Sync for WindowInfo {}

#[derive(Clone)]
struct WindowsLoopData {
    pub info: WindowInfo,
    pub tx: Sender<SystrayEvent>,
}

unsafe fn get_win_os_error(msg: &str) -> Error {
    Error::OsError(format!("{}: {}", &msg, GetLastError()))
}

unsafe extern "system" fn window_proc(
    h_wnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if msg == WM_MENUCOMMAND {
        WININFO_STASH.with(|stash| {
            let stash = stash.borrow();
            let stash = stash.as_ref();
            if let Some(stash) = stash {
                let menu_id = GetMenuItemID(stash.info.hmenu, w_param as i32) as i32;
                if menu_id != -1 {
                    stash
                        .tx
                        .send(SystrayEvent {
                            menu_index: menu_id as u32,
                        })
                        .ok();
                }
            }
        });
    }

    if msg == WM_USER + 1 && (l_param as u32 == WM_LBUTTONUP || l_param as u32 == WM_RBUTTONUP) {
        let mut p = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut p as *mut POINT) == 0 {
            return 1;
        }
        SetForegroundWindow(h_wnd);
        WININFO_STASH.with(|stash| {
            let stash = stash.borrow();
            let stash = stash.as_ref();
            if let Some(stash) = stash {
                TrackPopupMenu(
                    stash.info.hmenu,
                    0,
                    p.x,
                    p.y,
                    (TPM_BOTTOMALIGN | TPM_LEFTALIGN) as i32,
                    h_wnd,
                    std::ptr::null_mut(),
                );
            }
        });
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
    }
    DefWindowProcW(h_wnd, msg, w_param, l_param)
}

fn get_nid_struct(hwnd: &HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: *hwnd,
        uID: 0x1_u32,
        uFlags: 0_u32,
        uCallbackMessage: 0_u32,
        hIcon: 0 as HICON,
        szTip: [0_u16; 128],
        dwState: 0_u32,
        dwStateMask: 0_u32,
        szInfo: [0_u16; 256],
        Anonymous: unsafe { mem::zeroed() },
        szInfoTitle: [0_u16; 64],
        dwInfoFlags: 0_u32,
        guidItem: GUID {
            data1: 0_u32,
            data2: 0_u16,
            data3: 0_u16,
            data4: [0; 8],
        },
        hBalloonIcon: 0 as HICON,
    }
}

fn get_menu_item_struct() -> MENUITEMINFOW {
    MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: 0_u32,
        fType: 0_u32,
        fState: 0_u32,
        wID: 0_u32,
        hSubMenu: 0 as HMENU,
        hbmpChecked: 0 as HBITMAP,
        hbmpUnchecked: 0 as HBITMAP,
        dwItemData: 0_usize,
        dwTypeData: std::ptr::null_mut(),
        cch: 0_u32,
        hbmpItem: 0 as HBITMAP,
    }
}

unsafe fn init_window() -> Result<WindowInfo, Error> {
    let class_name = to_wstring("my_window");
    let hinstance: HINSTANCE = GetModuleHandleA(std::ptr::null_mut());
    let wnd = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0 as HINSTANCE,
        hIcon: LoadIconW(0 as HINSTANCE, IDI_APPLICATION),
        hCursor: LoadCursorW(0 as HINSTANCE, IDI_APPLICATION),
        hbrBackground: 16 as HBRUSH,
        lpszMenuName: 0 as PCWSTR,
        lpszClassName: class_name.as_ptr(),
    };
    if RegisterClassW(&wnd) == 0 {
        return Err(get_win_os_error("Error creating window class"));
    }
    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        to_wstring("rust_systray_window").as_ptr(),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        0,
        CW_USEDEFAULT,
        0,
        0 as HWND,
        0 as HMENU,
        0 as HINSTANCE,
        std::ptr::null_mut(),
    );
    if hwnd == 0 || hwnd == -1 {
        return Err(get_win_os_error("Error creating window"));
    }
    let mut nid = get_nid_struct(&hwnd);
    nid.uID = 0x1;
    nid.uFlags = NIF_MESSAGE;
    nid.uCallbackMessage = WM_USER + 1;
    if Shell_NotifyIconW(NIM_ADD, &mut nid as *mut NOTIFYICONDATAW) == 0 {
        return Err(get_win_os_error("Error adding menu icon"));
    }
    // Setup menu
    let hmenu = CreatePopupMenu();
    let m = MENUINFO {
        cbSize: std::mem::size_of::<MENUINFO>() as u32,
        fMask: MIM_APPLYTOSUBMENUS | MIM_STYLE,
        dwStyle: MNS_NOTIFYBYPOS,
        cyMax: 0_u32,
        hbrBack: 0 as HBRUSH,
        dwContextHelpID: 0_u32,
        dwMenuData: 0_usize,
    };
    if SetMenuInfo(hmenu, &m as *const MENUINFO) == 0 {
        return Err(get_win_os_error("Error setting up menu"));
    }

    Ok(WindowInfo {
        hwnd,
        hmenu,
        hinstance,
    })
}

unsafe fn run_loop() {
    log::debug!("Running windows loop");
    // Run message loop
    let mut msg = MSG {
        hwnd: 0 as HWND,
        message: 0_u32,
        wParam: 0 as WPARAM,
        lParam: 0 as LPARAM,
        time: 0_u32,
        pt: POINT { x: 0, y: 0 },
    };
    loop {
        GetMessageW(&mut msg, 0 as HWND, 0, 0);
        if msg.message == WM_QUIT {
            break;
        }
        TranslateMessage(&mut msg);
        DispatchMessageW(&mut msg);
    }
    log::debug!("Leaving windows run loop");
}

pub struct Window {
    info: WindowInfo,
    windows_loop: Option<thread::JoinHandle<()>>,
}

impl Window {
    pub fn new(event_tx: Sender<SystrayEvent>) -> Result<Window, Error> {
        let (tx, rx) = channel();
        let windows_loop = thread::spawn(move || {
            unsafe {
                let i = init_window();
                let k = match i {
                    Ok(j) => {
                        tx.send(Ok(j.clone())).ok();
                        j
                    }
                    Err(e) => {
                        // If creation didn't work, return out of the thread.
                        tx.send(Err(e)).ok();
                        return;
                    }
                };
                WININFO_STASH.with(|stash| {
                    let data = WindowsLoopData {
                        info: k,
                        tx: event_tx,
                    };
                    (*stash.borrow_mut()) = Some(data);
                });
                run_loop();
            }
        });
        let info = match rx.recv().unwrap() {
            Ok(i) => i,
            Err(e) => {
                return Err(e);
            }
        };
        let w = Window {
            info,
            windows_loop: Some(windows_loop),
        };
        Ok(w)
    }

    pub fn quit(&mut self) {
        unsafe {
            PostMessageW(self.info.hwnd, WM_DESTROY, 0 as WPARAM, 0 as LPARAM);
        }
        if let Some(t) = self.windows_loop.take() {
            t.join().ok();
        }
    }

    pub fn set_tooltip(&self, tooltip: &str) -> Result<(), Error> {
        // Add Tooltip
        log::debug!("Setting tooltip to {}", tooltip);
        // Gross way to convert String to [i8; 128]
        // TODO: Clean up conversion, test for length so we don't panic at runtime
        let tt = tooltip.as_bytes().clone();
        let mut nid = get_nid_struct(&self.info.hwnd);
        for (i, item) in tt.iter().enumerate() {
            nid.szTip[i] = *item as u16;
        }
        nid.uFlags = NIF_TIP;
        unsafe {
            if Shell_NotifyIconW(NIM_MODIFY, &mut nid as *mut NOTIFYICONDATAW) == 0 {
                return Err(get_win_os_error("Error setting tooltip"));
            }
        }
        Ok(())
    }

    pub fn add_menu_entry(&self, item_idx: u32, item_name: &str) -> Result<(), Error> {
        let mut st = to_wstring(item_name);
        let mut item = get_menu_item_struct();
        item.fMask = MIIM_FTYPE | MIIM_STRING | MIIM_ID | MIIM_STATE;
        item.fType = MFT_STRING;
        item.wID = item_idx;
        item.dwTypeData = st.as_mut_ptr();
        item.cch = (item_name.len() * 2) as u32;
        unsafe {
            if InsertMenuItemW(self.info.hmenu, item_idx, 1, &item as *const MENUITEMINFOW) == 0 {
                return Err(get_win_os_error("Error inserting menu item"));
            }
        }
        Ok(())
    }

    pub fn add_menu_separator(&self, item_idx: u32) -> Result<(), Error> {
        let mut item = get_menu_item_struct();
        item.fMask = MIIM_FTYPE;
        item.fType = MFT_SEPARATOR;
        item.wID = item_idx;
        unsafe {
            if InsertMenuItemW(self.info.hmenu, item_idx, 1, &item as *const MENUITEMINFOW) == 0 {
                return Err(get_win_os_error("Error inserting separator"));
            }
        }
        Ok(())
    }

    fn set_icon(&self, icon: HICON) -> Result<(), Error> {
        unsafe {
            let mut nid = get_nid_struct(&self.info.hwnd);
            nid.uFlags = NIF_ICON;
            nid.hIcon = icon;
            if Shell_NotifyIconW(NIM_MODIFY, &mut nid as *mut NOTIFYICONDATAW) == 0 {
                return Err(get_win_os_error("Error setting icon"));
            }
        }
        Ok(())
    }

    pub fn set_icon_from_resource(&self, resource_name: &str) -> Result<(), Error> {
        let icon;
        unsafe {
            icon = LoadImageW(
                self.info.hinstance,
                to_wstring(resource_name).as_ptr(),
                IMAGE_ICON,
                64,
                64,
                0,
            ) as HICON;
            if icon == -1 || icon == 0 {
                return Err(get_win_os_error("Error setting icon from resource"));
            }
        }
        self.set_icon(icon)
    }

    pub fn set_icon_from_file(&self, icon_file: &str) -> Result<(), Error> {
        let wstr_icon_file = to_wstring(icon_file);
        let hicon;
        unsafe {
            hicon = LoadImageW(
                0 as HINSTANCE,
                wstr_icon_file.as_ptr(),
                IMAGE_ICON,
                64,
                64,
                LR_LOADFROMFILE,
            ) as HICON;
            if hicon == 0 || hicon == -1 {
                return Err(get_win_os_error("Error setting icon from file"));
            }
        }
        self.set_icon(hicon)
    }

    pub fn set_icon_from_buffer(
        &self,
        buffer: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(), Error> {
        let offset = unsafe {
            LookupIconIdFromDirectoryEx(
                buffer.as_ptr() as *const u8,
                1,
                width as i32,
                height as i32,
                LR_DEFAULTCOLOR,
            )
        };

        if offset != 0 {
            let icon_data = &buffer[offset as usize..];
            let hicon = unsafe {
                CreateIconFromResourceEx(
                    icon_data.as_ptr() as *const u8,
                    0,
                    1,
                    0x30000,
                    width as i32,
                    height as i32,
                    LR_DEFAULTCOLOR,
                )
            };

            if hicon == 0 || hicon == -1 {
                return Err(unsafe { get_win_os_error("Cannot load icon from the buffer") });
            }

            self.set_icon(hicon)
        } else {
            Err(unsafe { get_win_os_error("Error setting icon from buffer") })
        }
    }

    pub fn shutdown(&self) -> Result<(), Error> {
        unsafe {
            let mut nid = get_nid_struct(&self.info.hwnd);
            nid.uFlags = NIF_ICON;
            if Shell_NotifyIconW(NIM_DELETE, &mut nid as *mut NOTIFYICONDATAW) == 0 {
                return Err(get_win_os_error("Error deleting icon from menu"));
            }
        }
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.shutdown().ok();
    }
}

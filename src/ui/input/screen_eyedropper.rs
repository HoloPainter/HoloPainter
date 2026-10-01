use eframe::egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct ScreenSample {
    pub x: i32,
    pub y: i32,
    pub color: Option<Color32>,
}

pub fn available() -> bool {
    platform::available()
}

pub fn current_screen_sample() -> Option<ScreenSample> {
    platform::current_screen_sample()
}

pub fn escape_down() -> bool {
    platform::escape_down()
}

pub struct GlobalEyedropperSession {
    inner: platform::GlobalEyedropperSession,
}

impl GlobalEyedropperSession {
    pub fn begin() -> Result<Self, String> {
        platform::GlobalEyedropperSession::begin().map(|inner| Self { inner })
    }

    pub fn pick_requested(&self) -> bool {
        self.inner.pick_requested()
    }

    pub fn refresh_cursor(&mut self) {
        self.inner.refresh_cursor();
    }
}

pub struct EyedropperOverlay {
    inner: platform::EyedropperOverlay,
}

impl EyedropperOverlay {
    pub fn new(
        owner_hwnd: Option<isize>,
        no_sample_text: String,
        instructions_text: String,
    ) -> Result<Self, String> {
        platform::EyedropperOverlay::new(owner_hwnd, no_sample_text, instructions_text)
            .map(|inner| Self { inner })
    }

    pub fn update(&mut self, x: i32, y: i32, color: Option<Color32>) {
        self.inner.update(x, y, color);
    }

    pub fn hide(&mut self) {
        self.inner.hide();
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{Color32, ScreenSample};
    use std::{
        ffi::c_void,
        mem, ptr,
        sync::{
            Once,
            atomic::{AtomicBool, AtomicU8, Ordering},
        },
    };
    use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::core::BOOL;

    type Hdc = *mut c_void;
    type Hbrush = *mut c_void;
    type Hcursor = *mut c_void;
    type Hicon = *mut c_void;
    type Hmenu = *mut c_void;
    type Hgdiobj = *mut c_void;
    type Hhook = *mut c_void;

    const CLR_INVALID: u32 = 0xffff_ffff;
    const VK_ESCAPE: i32 = 0x1b;

    const IDC_CROSS: usize = 32515;

    const WH_MOUSE_LL: i32 = 14;
    const HC_ACTION: i32 = 0;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const PICK_IDLE: u8 = 0;
    const PICK_BUTTON_DOWN: u8 = 1;
    const PICK_REQUESTED: u8 = 2;

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

    const SW_HIDE: i32 = 0;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const HWND_TOPMOST: isize = -1;

    const GWLP_USERDATA: i32 = -21;
    const WM_NCCREATE: u32 = 0x0081;
    const WM_PAINT: u32 = 0x000f;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_NCHITTEST: u32 = 0x0084;
    const HTTRANSPARENT: LRESULT = -1;
    const DT_LEFT: u32 = 0x0000;
    const DT_TOP: u32 = 0x0000;
    const DT_SINGLELINE: u32 = 0x0020;
    const TRANSPARENT: i32 = 1;

    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;

    const OVERLAY_WIDTH: i32 = 214;
    const OVERLAY_HEIGHT: i32 = 62;
    const OVERLAY_OFFSET: i32 = 18;

    static REGISTER_OVERLAY_CLASS: Once = Once::new();
    static MOUSE_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
    static PICK_STATE: AtomicU8 = AtomicU8::new(PICK_IDLE);
    static OVERLAY_CLASS_NAME: [u16; 40] = [
        b'H' as u16,
        b'o' as u16,
        b'l' as u16,
        b'o' as u16,
        b'P' as u16,
        b'a' as u16,
        b'i' as u16,
        b'n' as u16,
        b't' as u16,
        b'e' as u16,
        b'r' as u16,
        b'E' as u16,
        b'y' as u16,
        b'e' as u16,
        b'd' as u16,
        b'r' as u16,
        b'o' as u16,
        b'p' as u16,
        b'p' as u16,
        b'e' as u16,
        b'r' as u16,
        b'O' as u16,
        b'v' as u16,
        b'e' as u16,
        b'r' as u16,
        b'l' as u16,
        b'a' as u16,
        b'y' as u16,
        b'W' as u16,
        b'i' as u16,
        b'n' as u16,
        b'd' as u16,
        b'o' as u16,
        b'w' as u16,
        b'C' as u16,
        b'l' as u16,
        b'a' as u16,
        b's' as u16,
        b's' as u16,
        0,
    ];

    #[repr(C)]
    struct WNDCLASSW {
        style: u32,
        lpfn_wnd_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: HINSTANCE,
        h_icon: Hicon,
        h_cursor: Hcursor,
        hbr_background: Hbrush,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
    }

    #[repr(C)]
    struct CREATESTRUCTW {
        lp_create_params: *mut c_void,
        h_instance: HINSTANCE,
        h_menu: Hmenu,
        hwnd_parent: HWND,
        cy: i32,
        cx: i32,
        y: i32,
        x: i32,
        style: i32,
        lpsz_name: *const u16,
        lpsz_class: *const u16,
        dw_ex_style: u32,
    }

    #[repr(C)]
    struct PAINTSTRUCT {
        hdc: Hdc,
        f_erase: BOOL,
        rc_paint: RECT,
        f_restore: BOOL,
        f_inc_update: BOOL,
        rgb_reserved: [u8; 32],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(lp_point: *mut POINT) -> BOOL;
        fn GetDC(hwnd: HWND) -> Hdc;
        fn ReleaseDC(hwnd: HWND, hdc: Hdc) -> i32;
        fn GetAsyncKeyState(vkey: i32) -> i16;
        fn LoadCursorW(h_instance: HINSTANCE, cursor_name: *const u16) -> Hcursor;
        fn SetCursor(cursor: Hcursor) -> Hcursor;
        fn SetWindowsHookExW(
            id_hook: i32,
            hook_proc: Option<unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT>,
            instance: HINSTANCE,
            thread_id: u32,
        ) -> Hhook;
        fn CallNextHookEx(hook: Hhook, code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
        fn UnhookWindowsHookEx(hook: Hhook) -> BOOL;
        fn RegisterClassW(class: *const WNDCLASSW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: HWND,
            menu: Hmenu,
            instance: HINSTANCE,
            param: *mut c_void,
        ) -> HWND;
        fn DestroyWindow(hwnd: HWND) -> BOOL;
        fn ShowWindow(hwnd: HWND, cmd_show: i32) -> BOOL;
        fn SetWindowPos(
            hwnd: HWND,
            insert_after: HWND,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> BOOL;
        fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: BOOL) -> BOOL;
        fn UpdateWindow(hwnd: HWND) -> BOOL;
        fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
        fn SetWindowLongPtrW(hwnd: HWND, index: i32, new_long: isize) -> isize;
        fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
        fn BeginPaint(hwnd: HWND, paint: *mut PAINTSTRUCT) -> Hdc;
        fn EndPaint(hwnd: HWND, paint: *const PAINTSTRUCT) -> BOOL;
        fn FillRect(hdc: Hdc, rect: *const RECT, brush: Hbrush) -> i32;
        fn FrameRect(hdc: Hdc, rect: *const RECT, brush: Hbrush) -> i32;
        fn DrawTextW(hdc: Hdc, text: *const u16, count: i32, rect: *mut RECT, format: u32) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetPixel(hdc: Hdc, x: i32, y: i32) -> u32;
        fn CreateSolidBrush(color: u32) -> Hbrush;
        fn DeleteObject(object: Hgdiobj) -> BOOL;
        fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        fn SetTextColor(hdc: Hdc, color: u32) -> u32;
    }

    pub fn available() -> bool {
        true
    }

    fn cursor_screen_pos() -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut point) };
        (ok != 0).then_some((point.x, point.y))
    }

    fn screen_pixel_color_at(x: i32, y: i32) -> Option<Color32> {
        unsafe {
            let hwnd = ptr::null_mut();
            let hdc = GetDC(hwnd);
            if hdc.is_null() {
                return None;
            }

            let color_ref = GetPixel(hdc, x, y);
            let _ = ReleaseDC(hwnd, hdc);

            if color_ref == CLR_INVALID {
                return None;
            }

            let r = (color_ref & 0x0000_00ff) as u8;
            let g = ((color_ref & 0x0000_ff00) >> 8) as u8;
            let b = ((color_ref & 0x00ff_0000) >> 16) as u8;
            Some(Color32::from_rgb(r, g, b))
        }
    }

    pub fn current_screen_sample() -> Option<ScreenSample> {
        let (x, y) = cursor_screen_pos()?;
        Some(ScreenSample {
            x,
            y,
            color: screen_pixel_color_at(x, y),
        })
    }

    pub fn escape_down() -> bool {
        key_down(VK_ESCAPE)
    }

    fn key_down(vkey: i32) -> bool {
        let state = unsafe { GetAsyncKeyState(vkey) } as u16;
        (state & 0x8000) != 0
    }

    pub struct GlobalEyedropperSession {
        hook: Hhook,
        crosshair: Hcursor,
        previous_cursor: Hcursor,
    }

    impl GlobalEyedropperSession {
        pub fn begin() -> Result<Self, String> {
            if MOUSE_HOOK_ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err("Eyedropper: a global mouse capture is already active".to_owned());
            }

            unsafe {
                let cross = LoadCursorW(ptr::null_mut(), IDC_CROSS as *const u16);
                if cross.is_null() {
                    MOUSE_HOOK_ACTIVE.store(false, Ordering::Release);
                    return Err("Eyedropper: failed to load crosshair cursor".to_owned());
                }

                PICK_STATE.store(PICK_IDLE, Ordering::Release);
                let instance = GetModuleHandleW(ptr::null());
                let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), instance, 0);
                if hook.is_null() {
                    MOUSE_HOOK_ACTIVE.store(false, Ordering::Release);
                    return Err(format!(
                        "Eyedropper: failed to install mouse hook: {}",
                        std::io::Error::last_os_error()
                    ));
                }

                let previous_cursor = SetCursor(cross);
                Ok(Self {
                    hook,
                    crosshair: cross,
                    previous_cursor,
                })
            }
        }

        pub fn pick_requested(&self) -> bool {
            PICK_STATE.load(Ordering::Acquire) == PICK_REQUESTED
        }

        pub fn refresh_cursor(&mut self) {
            if !self.crosshair.is_null() {
                unsafe {
                    let _ = SetCursor(self.crosshair);
                }
            }
        }
    }

    impl Drop for GlobalEyedropperSession {
        fn drop(&mut self) {
            if !self.hook.is_null() {
                unsafe {
                    let _ = UnhookWindowsHookEx(self.hook);
                }
                self.hook = ptr::null_mut();
            }
            PICK_STATE.store(PICK_IDLE, Ordering::Release);
            MOUSE_HOOK_ACTIVE.store(false, Ordering::Release);
            unsafe {
                let _ = SetCursor(self.previous_cursor);
            }
        }
    }

    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code == HC_ACTION && MOUSE_HOOK_ACTIVE.load(Ordering::Acquire) {
            let (next_state, consume) =
                pick_transition(PICK_STATE.load(Ordering::Acquire), wparam as u32);
            PICK_STATE.store(next_state, Ordering::Release);
            if consume {
                return 1;
            }
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }

    fn pick_transition(state: u8, message: u32) -> (u8, bool) {
        match message {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => (PICK_BUTTON_DOWN, true),
            WM_LBUTTONUP if state == PICK_BUTTON_DOWN => (PICK_REQUESTED, true),
            _ => (state, false),
        }
    }

    struct OverlayData {
        color: Option<Color32>,
        no_sample_text: String,
        instructions_text: String,
    }

    pub struct EyedropperOverlay {
        hwnd: HWND,
        data: Box<OverlayData>,
    }

    impl EyedropperOverlay {
        pub fn new(
            _owner_hwnd: Option<isize>,
            no_sample_text: String,
            instructions_text: String,
        ) -> Result<Self, String> {
            unsafe {
                register_overlay_class();

                let mut data = Box::new(OverlayData {
                    color: None,
                    no_sample_text,
                    instructions_text,
                });
                let data_ptr = data.as_mut() as *mut OverlayData as *mut c_void;
                let h_instance = GetModuleHandleW(ptr::null());
                // Keep the preview as an independent top-level tool window.
                // Owned popup windows can be clipped/hidden together with the main app
                // on some WGPU/winit setups, which made the preview silently vanish.
                let parent = ptr::null_mut();
                let hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    OVERLAY_CLASS_NAME.as_ptr(),
                    OVERLAY_CLASS_NAME.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    OVERLAY_WIDTH,
                    OVERLAY_HEIGHT,
                    parent,
                    ptr::null_mut(),
                    h_instance,
                    data_ptr,
                );
                if hwnd.is_null() {
                    return Err("Eyedropper: failed to create preview overlay".to_owned());
                }

                Ok(Self { hwnd, data })
            }
        }

        pub fn update(&mut self, x: i32, y: i32, color: Option<Color32>) {
            self.data.color = color;
            if self.hwnd.is_null() {
                return;
            }

            let (overlay_x, overlay_y) = place_overlay(x, y);
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST as HWND,
                    overlay_x,
                    overlay_y,
                    OVERLAY_WIDTH,
                    OVERLAY_HEIGHT,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(self.hwnd, ptr::null(), 0);
                let _ = UpdateWindow(self.hwnd);
            }
        }

        pub fn hide(&mut self) {
            if self.hwnd.is_null() {
                return;
            }
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    impl Drop for EyedropperOverlay {
        fn drop(&mut self) {
            if self.hwnd.is_null() {
                return;
            }
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
            self.hwnd = ptr::null_mut();
        }
    }

    fn register_overlay_class() {
        REGISTER_OVERLAY_CLASS.call_once(|| unsafe {
            let h_instance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSW {
                style: 0,
                lpfn_wnd_proc: Some(overlay_window_proc),
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon: ptr::null_mut(),
                h_cursor: ptr::null_mut(),
                hbr_background: ptr::null_mut(),
                lpsz_menu_name: ptr::null(),
                lpsz_class_name: OVERLAY_CLASS_NAME.as_ptr(),
            };
            let _ = RegisterClassW(&class);
        });
    }

    unsafe extern "system" fn overlay_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCCREATE => {
                let create = lparam as *const CREATESTRUCTW;
                if !create.is_null() {
                    let data = unsafe { (*create).lp_create_params } as isize;
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, data) };
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => HTTRANSPARENT,
            WM_PAINT => {
                paint_overlay(hwnd);
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn paint_overlay(hwnd: HWND) {
        unsafe {
            let mut paint: PAINTSTRUCT = mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut paint);
            if hdc.is_null() {
                let _ = EndPaint(hwnd, &paint);
                return;
            }

            let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const OverlayData;
            let data = data_ptr.as_ref();
            let color = data.and_then(|data| data.color);

            let background = CreateSolidBrush(color_ref(28, 28, 32));
            let border = CreateSolidBrush(color_ref(82, 88, 96));
            let text = color_ref(238, 238, 238);
            let muted = color_ref(188, 192, 198);
            let whole = RECT {
                left: 0,
                top: 0,
                right: OVERLAY_WIDTH,
                bottom: OVERLAY_HEIGHT,
            };
            let _ = FillRect(hdc, &whole, background);
            let _ = FrameRect(hdc, &whole, border);

            let swatch_color = color.unwrap_or(Color32::from_rgb(0, 0, 0));
            let swatch = CreateSolidBrush(color_ref(
                swatch_color.r(),
                swatch_color.g(),
                swatch_color.b(),
            ));
            let swatch_border = CreateSolidBrush(color_ref(220, 220, 220));
            let swatch_rect = RECT {
                left: 10,
                top: 10,
                right: 42,
                bottom: 42,
            };
            let _ = FillRect(hdc, &swatch_rect, swatch);
            let _ = FrameRect(hdc, &swatch_rect, swatch_border);

            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(hdc, text);
            let label = match color {
                Some(c) => format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()),
                None => data.map_or_else(String::new, |data| data.no_sample_text.clone()),
            };
            draw_text(hdc, &label, 52, 10, OVERLAY_WIDTH - 10, 30, DT_SINGLELINE);

            let _ = SetTextColor(hdc, muted);
            draw_text(
                hdc,
                data.map_or("", |data| data.instructions_text.as_str()),
                52,
                32,
                OVERLAY_WIDTH - 10,
                52,
                DT_SINGLELINE,
            );

            let _ = DeleteObject(background as Hgdiobj);
            let _ = DeleteObject(border as Hgdiobj);
            let _ = DeleteObject(swatch as Hgdiobj);
            let _ = DeleteObject(swatch_border as Hgdiobj);
            let _ = EndPaint(hwnd, &paint);
        }
    }

    unsafe fn draw_text(
        hdc: Hdc,
        text: &str,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        extra_flags: u32,
    ) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left,
            top,
            right,
            bottom,
        };
        unsafe {
            let _ = DrawTextW(
                hdc,
                wide.as_ptr(),
                wide.len().min(i32::MAX as usize) as i32,
                &mut rect,
                DT_LEFT | DT_TOP | extra_flags,
            );
        }
    }

    fn place_overlay(cursor_x: i32, cursor_y: i32) -> (i32, i32) {
        let virtual_left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let virtual_top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let virtual_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let virtual_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        let virtual_right = virtual_left + virtual_width.max(1);
        let virtual_bottom = virtual_top + virtual_height.max(1);

        let mut x = cursor_x + OVERLAY_OFFSET;
        let mut y = cursor_y + OVERLAY_OFFSET;
        if x + OVERLAY_WIDTH > virtual_right {
            x = cursor_x - OVERLAY_WIDTH - OVERLAY_OFFSET;
        }
        if y + OVERLAY_HEIGHT > virtual_bottom {
            y = cursor_y - OVERLAY_HEIGHT - OVERLAY_OFFSET;
        }

        let max_x = (virtual_right - OVERLAY_WIDTH).max(virtual_left);
        let max_y = (virtual_bottom - OVERLAY_HEIGHT).max(virtual_top);
        x = x.clamp(virtual_left, max_x);
        y = y.clamp(virtual_top, max_y);
        (x, y)
    }

    fn color_ref(r: u8, g: u8, b: u8) -> u32 {
        r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn left_click_is_consumed_until_pick_is_requested() {
            let (state, consume) = pick_transition(PICK_IDLE, WM_LBUTTONDOWN);
            assert_eq!(state, PICK_BUTTON_DOWN);
            assert!(consume);

            let (state, consume) = pick_transition(state, WM_LBUTTONUP);
            assert_eq!(state, PICK_REQUESTED);
            assert!(consume);
        }

        #[test]
        fn unrelated_mouse_messages_pass_through() {
            let (state, consume) = pick_transition(PICK_IDLE, 0x0200);
            assert_eq!(state, PICK_IDLE);
            assert!(!consume);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{Color32, ScreenSample};

    pub fn available() -> bool {
        false
    }

    pub fn current_screen_sample() -> Option<ScreenSample> {
        None
    }

    pub fn escape_down() -> bool {
        false
    }

    pub struct GlobalEyedropperSession;

    impl GlobalEyedropperSession {
        pub fn begin() -> Result<Self, String> {
            Err("Screen eyedropper is only available on Windows.".to_owned())
        }

        pub fn pick_requested(&self) -> bool {
            false
        }

        pub fn refresh_cursor(&mut self) {}
    }

    pub struct EyedropperOverlay;

    impl EyedropperOverlay {
        pub fn new(
            _owner_hwnd: Option<isize>,
            _no_sample_text: String,
            _instructions_text: String,
        ) -> Result<Self, String> {
            Err("Screen eyedropper preview overlay is only available on Windows.".to_owned())
        }

        pub fn update(&mut self, _x: i32, _y: i32, _color: Option<Color32>) {}

        pub fn hide(&mut self) {}
    }
}

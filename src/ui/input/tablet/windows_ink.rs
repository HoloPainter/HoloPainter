#[derive(Debug, Clone, Copy)]
pub enum WindowsInkPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsInkPointerKind {
    Pen,
    Eraser,
}

#[derive(Debug, Clone, Copy)]
pub struct WindowsInkSample {
    pub phase: WindowsInkPhase,
    pub client_x: f32,
    pub client_y: f32,
    pub pressure_u16: u16,
    pub pointer_kind: WindowsInkPointerKind,
    pub primary_button: bool,
    pub secondary_button: bool,
    pub raw_msg: u32,
    pub pointer_flags: u32,
    pub contact: bool,
    pub pressed: bool,
    pub last_pressed_before: bool,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{WindowsInkPhase, WindowsInkPointerKind, WindowsInkSample};
    use std::collections::VecDeque;
    use std::mem;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::UI::Input::Pointer::{
        GetPointerPenInfo, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_INCONTACT,
        POINTER_FLAG_SECONDBUTTON, POINTER_PEN_INFO,
    };
    use windows_sys::core::BOOL;

    type SubclassProc =
        unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

    const PEN_MASK_PRESSURE: u32 = 0x0000_0001;
    const PEN_FLAG_ERASER: u32 = 0x0000_0004;

    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(
            hwnd: HWND,
            subclass_proc: Option<SubclassProc>,
            subclass_id: usize,
            ref_data: usize,
        ) -> BOOL;
        fn RemoveWindowSubclass(
            hwnd: HWND,
            subclass_proc: Option<SubclassProc>,
            subclass_id: usize,
        ) -> BOOL;
        fn DefSubclassProc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    }

    const WM_POINTERUPDATE: u32 = 0x0245;
    const WM_POINTERDOWN: u32 = 0x0246;
    const WM_POINTERUP: u32 = 0x0247;
    const WM_POINTERCAPTURECHANGED: u32 = 0x024C;
    const WINDOWS_INK_SUBCLASS_ID: usize = 0x4d50_494e;

    #[link(name = "user32")]
    unsafe extern "system" {
        #[link_name = "ScreenToClient"]
        fn screen_to_client(hwnd: HWND, point: *mut POINT) -> BOOL;
    }

    struct SharedState {
        last_pressed: bool,
        samples: VecDeque<WindowsInkSample>,
    }

    pub struct WindowsInkInput {
        hwnd: HWND,
        shared: Box<SharedState>,
    }

    impl WindowsInkInput {
        pub fn new(hwnd: isize) -> Result<Self, String> {
            let hwnd = hwnd as HWND;
            if hwnd.is_null() {
                return Err("Windows Ink: HWND is not available".to_owned());
            }

            unsafe {
                let mut shared = Box::new(SharedState {
                    last_pressed: false,
                    samples: VecDeque::new(),
                });
                let shared_ptr = shared.as_mut() as *mut SharedState as usize;
                if SetWindowSubclass(
                    hwnd,
                    Some(subclass_proc),
                    WINDOWS_INK_SUBCLASS_ID,
                    shared_ptr,
                ) == 0
                {
                    return Err("Windows Ink: SetWindowSubclass failed".to_owned());
                }

                Ok(Self { hwnd, shared })
            }
        }

        pub fn poll_samples(&mut self) -> Vec<WindowsInkSample> {
            self.shared.samples.drain(..).collect()
        }
    }

    impl Drop for WindowsInkInput {
        fn drop(&mut self) {
            unsafe {
                if !self.hwnd.is_null() {
                    RemoveWindowSubclass(self.hwnd, Some(subclass_proc), WINDOWS_INK_SUBCLASS_ID);
                    self.hwnd = std::ptr::null_mut();
                }
            }
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        ref_data: usize,
    ) -> LRESULT {
        match msg {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
                let shared_ptr = ref_data as *mut SharedState;
                if !shared_ptr.is_null() {
                    let shared = unsafe { &mut *shared_ptr };
                    handle_pointer_message(hwnd, msg, wparam, shared);
                }
            }
            _ => {}
        }

        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    fn handle_pointer_message(hwnd: HWND, msg: u32, wparam: WPARAM, shared: &mut SharedState) {
        if msg == WM_POINTERCAPTURECHANGED {
            if shared.last_pressed {
                shared.last_pressed = false;
            }
            return;
        }

        let pointer_id = (wparam & 0xffff) as u32;
        let mut pen_info: POINTER_PEN_INFO = unsafe { mem::zeroed() };
        if unsafe { GetPointerPenInfo(pointer_id, &mut pen_info) } == 0 {
            return;
        }

        let flags = pen_info.pointerInfo.pointerFlags;
        let contact = flags & POINTER_FLAG_INCONTACT != 0;
        let primary_button = flags & POINTER_FLAG_FIRSTBUTTON != 0;
        let secondary_button = flags & POINTER_FLAG_SECONDBUTTON != 0;
        let pressed = msg == WM_POINTERDOWN || (contact || primary_button);
        let last_pressed_before = shared.last_pressed;

        let phase = match (last_pressed_before, pressed, msg) {
            (_, _, WM_POINTERDOWN) => WindowsInkPhase::Down,
            (true, _, WM_POINTERUP) => WindowsInkPhase::Up,
            (true, false, _) => WindowsInkPhase::Up,
            (false, false, _) => return,
            (false, true, _) => WindowsInkPhase::Down,
            (true, true, _) => WindowsInkPhase::Move,
        };

        let mut pt = pen_info.pointerInfo.ptPixelLocation;
        unsafe { screen_to_client(hwnd, &mut pt) };

        let pressure_u16 = if pen_info.penMask & PEN_MASK_PRESSURE != 0 {
            ink_pressure_to_u16(pen_info.pressure)
        } else if contact {
            u16::MAX
        } else {
            0
        };

        let pointer_kind = if pen_info.penFlags & PEN_FLAG_ERASER != 0 {
            WindowsInkPointerKind::Eraser
        } else {
            WindowsInkPointerKind::Pen
        };

        shared.last_pressed = pressed && !matches!(phase, WindowsInkPhase::Up);
        shared.samples.push_back(WindowsInkSample {
            phase,
            client_x: pt.x as f32,
            client_y: pt.y as f32,
            pressure_u16,
            pointer_kind,
            primary_button,
            secondary_button,
            raw_msg: msg,
            pointer_flags: flags,
            contact,
            pressed,
            last_pressed_before,
        });
    }

    fn ink_pressure_to_u16(pressure: u32) -> u16 {
        ((pressure.min(1024) as f32 / 1024.0) * u16::MAX as f32)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::WindowsInkSample;

    pub struct WindowsInkInput;

    impl WindowsInkInput {
        pub fn new(_hwnd: isize) -> Result<Self, String> {
            Err("Windows Ink: Windows only".to_owned())
        }

        pub fn poll_samples(&mut self) -> Vec<WindowsInkSample> {
            Vec::new()
        }
    }
}

pub use imp::WindowsInkInput;

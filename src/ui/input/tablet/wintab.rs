#[derive(Debug, Clone, Copy)]
pub enum WinTabPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinTabCursorKind {
    Unknown,
    Pressure,
    Eraser,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WinTabPacketFlags {
    pub in_range: bool,
    pub pressure_valid: bool,
    pub button1: bool,
    pub button2: bool,
    pub eraser: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct WinTabSample {
    pub phase: WinTabPhase,
    pub client_x: f32,
    pub client_y: f32,
    pub screen_x: f32,
    pub screen_y: f32,
    pub pressure: f32,
    /// Pressure normalized to SAI's recovered 0..0xffff tablet range.
    pub pressure_u16: u16,
    /// Tablet position in SAI-style 16.16 fixed-point screen pixels.
    pub x_fixed_16_16: i32,
    pub y_fixed_16_16: i32,
    pub cursor_kind: WinTabCursorKind,
    pub flags: WinTabPacketFlags,
    pub raw_buttons: u32,
    pub button_state: u32,
    pub pressed: bool,
    pub last_pressed_before: bool,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{WinTabCursorKind, WinTabPacketFlags, WinTabPhase, WinTabSample};
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::mem;
    use std::ptr;
    use windows_sys::Win32::Foundation::{
        FreeLibrary, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    use windows_sys::core::BOOL;

    type SubclassProc =
        unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

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

    #[link(name = "user32")]
    unsafe extern "system" {
        #[link_name = "ScreenToClient"]
        fn screen_to_client(hwnd: HWND, point: *mut POINT) -> BOOL;
    }

    type Hctx = *mut c_void;
    type Hmodule = HINSTANCE;

    const WT_DEFBASE: u32 = 0x7ff0;
    const WT_PACKET: u32 = WT_DEFBASE;

    const PK_CURSOR: u32 = 0x0020;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_X: u32 = 0x0080;
    const PK_Y: u32 = 0x0100;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    const PACKETDATA: u32 = PK_CURSOR | PK_X | PK_Y | PK_BUTTONS | PK_NORMAL_PRESSURE;
    const PACKETMODE: u32 = PK_BUTTONS;

    const WTI_DEFSYSCTX: u32 = 4;
    const WTI_DEVICES: u32 = 100;
    const WTI_CURSORS: u32 = 200;
    const CSR_NAME: u32 = 1;
    const DVC_X: u32 = 12;
    const DVC_Y: u32 = 13;
    const DVC_NPRESSURE: u32 = 15;
    const MAX_CURSOR_KINDS: usize = 64;

    const CXO_SYSTEM: u32 = 0x0001;
    const CXO_MESSAGES: u32 = 0x0004;

    const TBN_DOWN: u32 = 2;
    const WINTAB_SUBCLASS_ID: usize = 0x4d50_5754;

    type WTInfoA = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
    type WTOpenA = unsafe extern "system" fn(HWND, *mut LogContextA, BOOL) -> Hctx;
    type WTClose = unsafe extern "system" fn(Hctx) -> BOOL;
    type WTEnable = unsafe extern "system" fn(Hctx, BOOL) -> BOOL;
    type WTOverlap = unsafe extern "system" fn(Hctx, BOOL) -> BOOL;
    type WTPacket = unsafe extern "system" fn(Hctx, u32, *mut c_void) -> BOOL;
    type WTQueueSizeSet = unsafe extern "system" fn(Hctx, i32) -> BOOL;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Axis {
        ax_min: i32,
        ax_max: i32,
        ax_units: u32,
        ax_resolution: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContextA {
        lc_name: [i8; 40],
        lc_options: u32,
        lc_status: u32,
        lc_locks: u32,
        lc_msg_base: u32,
        lc_device: u32,
        lc_pkt_rate: u32,
        lc_pkt_data: u32,
        lc_pkt_mode: u32,
        lc_move_mask: u32,
        lc_btn_dn_mask: u32,
        lc_btn_up_mask: u32,
        lc_in_org_x: i32,
        lc_in_org_y: i32,
        lc_in_org_z: i32,
        lc_in_ext_x: i32,
        lc_in_ext_y: i32,
        lc_in_ext_z: i32,
        lc_out_org_x: i32,
        lc_out_org_y: i32,
        lc_out_org_z: i32,
        lc_out_ext_x: i32,
        lc_out_ext_y: i32,
        lc_out_ext_z: i32,
        lc_sens_x: u32,
        lc_sens_y: u32,
        lc_sens_z: u32,
        lc_sys_mode: i32,
        lc_sys_org_x: i32,
        lc_sys_org_y: i32,
        lc_sys_ext_x: i32,
        lc_sys_ext_y: i32,
        lc_sys_sens_x: u32,
        lc_sys_sens_y: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Packet {
        pk_cursor: u32,
        pk_buttons: u32,
        pk_x: i32,
        pk_y: i32,
        pk_normal_pressure: u32,
    }

    struct WinTabApi {
        module: Hmodule,
        wt_info_a: WTInfoA,
        wt_open_a: WTOpenA,
        wt_close: WTClose,
        wt_enable: WTEnable,
        wt_overlap: WTOverlap,
        wt_packet: WTPacket,
        wt_queue_size_set: Option<WTQueueSizeSet>,
    }

    impl WinTabApi {
        fn load() -> Result<Self, String> {
            unsafe {
                let module = LoadLibraryA(b"Wintab32.dll\0".as_ptr());
                if module.is_null() {
                    return Err("WinTab: Wintab32.dll not found".to_owned());
                }

                let api = Self {
                    module,
                    wt_info_a: load_required(module, b"WTInfoA\0")?,
                    wt_open_a: load_required(module, b"WTOpenA\0")?,
                    wt_close: load_required(module, b"WTClose\0")?,
                    wt_enable: load_required(module, b"WTEnable\0")?,
                    wt_overlap: load_required(module, b"WTOverlap\0")?,
                    wt_packet: load_required(module, b"WTPacket\0")?,
                    wt_queue_size_set: load_optional(module, b"WTQueueSizeSet\0"),
                };

                if (api.wt_info_a)(0, 0, ptr::null_mut()) == 0 {
                    return Err("WinTab: driver is not available".to_owned());
                }

                Ok(api)
            }
        }
    }

    impl Drop for WinTabApi {
        fn drop(&mut self) {
            unsafe {
                if !self.module.is_null() {
                    FreeLibrary(self.module);
                    self.module = ptr::null_mut();
                }
            }
        }
    }

    struct SharedState {
        api: WinTabApi,
        hctx: Hctx,
        pressure_min: i32,
        pressure_max: i32,
        out_left: i32,
        out_right: i32,
        out_top: i32,
        out_bottom: i32,
        cursor_kinds: [WinTabCursorKind; MAX_CURSOR_KINDS],
        last_pressed: bool,
        samples: VecDeque<WinTabSample>,
    }

    pub struct WinTabInput {
        hwnd: HWND,
        shared: Box<SharedState>,
    }

    impl WinTabInput {
        pub fn new(hwnd: isize) -> Result<Self, String> {
            let hwnd = hwnd as HWND;
            if hwnd.is_null() {
                return Err("WinTab: HWND is not available".to_owned());
            }

            unsafe {
                let api = WinTabApi::load()?;
                let mut shared = Box::new(SharedState {
                    api,
                    hctx: ptr::null_mut(),
                    pressure_min: 0,
                    pressure_max: 1023,
                    out_left: 0,
                    out_right: 0,
                    out_top: 0,
                    out_bottom: 0,
                    cursor_kinds: [WinTabCursorKind::Pressure; MAX_CURSOR_KINDS],
                    last_pressed: false,
                    samples: VecDeque::new(),
                });

                let shared_ptr = shared.as_mut() as *mut SharedState as usize;
                if SetWindowSubclass(hwnd, Some(subclass_proc), WINTAB_SUBCLASS_ID, shared_ptr) == 0
                {
                    return Err("WinTab: SetWindowSubclass failed".to_owned());
                }

                if let Err(error) = init_context(hwnd, &mut shared) {
                    RemoveWindowSubclass(hwnd, Some(subclass_proc), WINTAB_SUBCLASS_ID);
                    return Err(error);
                }

                Ok(Self { hwnd, shared })
            }
        }

        pub fn poll_samples(&mut self) -> Vec<WinTabSample> {
            self.shared.samples.drain(..).collect()
        }
    }

    impl Drop for WinTabInput {
        fn drop(&mut self) {
            unsafe {
                if !self.shared.hctx.is_null() {
                    (self.shared.api.wt_close)(self.shared.hctx);
                    self.shared.hctx = ptr::null_mut();
                }
                if !self.hwnd.is_null() {
                    RemoveWindowSubclass(self.hwnd, Some(subclass_proc), WINTAB_SUBCLASS_ID);
                    self.hwnd = ptr::null_mut();
                }
            }
        }
    }

    unsafe fn init_context(hwnd: HWND, shared: &mut SharedState) -> Result<(), String> {
        let mut context: LogContextA = unsafe { mem::zeroed() };
        let ret = unsafe {
            (shared.api.wt_info_a)(
                WTI_DEFSYSCTX,
                0,
                &mut context as *mut LogContextA as *mut c_void,
            )
        };
        if ret == 0 {
            return Err("WinTab: WTInfoA(WTI_DEFSYSCTX) failed".to_owned());
        }

        write_lc_name(&mut context, b"HoloPainter Wintab\0");
        context.lc_options |= CXO_SYSTEM | CXO_MESSAGES;
        context.lc_pkt_data = PACKETDATA;
        context.lc_pkt_mode = PACKETMODE;
        context.lc_move_mask = PACKETDATA;
        context.lc_btn_up_mask = context.lc_btn_dn_mask;

        let tablet_x = unsafe { query_axis(&shared.api, WTI_DEVICES, DVC_X) };
        let tablet_y = unsafe { query_axis(&shared.api, WTI_DEVICES, DVC_Y) };
        if let Some(axis) = tablet_x {
            context.lc_in_org_x = axis.ax_min;
            context.lc_in_ext_x = axis.ax_max - axis.ax_min;
        }
        if let Some(axis) = tablet_y {
            context.lc_in_org_y = axis.ax_min;
            context.lc_in_ext_y = axis.ax_max - axis.ax_min;
        }

        if let Some(axis) = unsafe { query_axis(&shared.api, WTI_DEVICES, DVC_NPRESSURE) } {
            shared.pressure_min = axis.ax_min;
            shared.pressure_max = axis.ax_max.max(axis.ax_min + 1);
        }

        let virtual_left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let virtual_top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let virtual_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let virtual_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        context.lc_out_org_x = virtual_left;
        context.lc_out_org_y = virtual_top;
        context.lc_out_ext_x = virtual_width;
        context.lc_out_ext_y = -virtual_height;
        shared.out_left = virtual_left;
        shared.out_right = virtual_left.saturating_add(virtual_width);
        shared.out_top = virtual_top;
        shared.out_bottom = virtual_top.saturating_add(virtual_height);
        unsafe { refresh_cursor_kinds(shared) };

        let hctx = unsafe { (shared.api.wt_open_a)(hwnd, &mut context, 1) };
        if hctx.is_null() {
            return Err("WinTab: WTOpenA failed".to_owned());
        }

        shared.hctx = hctx;
        if let Some(queue_size_set) = shared.api.wt_queue_size_set {
            unsafe { queue_size_set(hctx, 256) };
        }
        unsafe {
            (shared.api.wt_enable)(hctx, 1);
            (shared.api.wt_overlap)(hctx, 1);
        }

        Ok(())
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        ref_data: usize,
    ) -> LRESULT {
        if msg == WT_PACKET {
            let shared_ptr = ref_data as *mut SharedState;
            if !shared_ptr.is_null() {
                let shared = unsafe { &mut *shared_ptr };
                let hctx = lparam as Hctx;
                let mut packet = Packet::default();
                if unsafe {
                    (shared.api.wt_packet)(
                        hctx,
                        wparam as u32,
                        &mut packet as *mut Packet as *mut c_void,
                    )
                } != 0
                {
                    push_packet(hwnd, shared, packet);
                    return 0;
                }
            }
        }

        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    fn push_packet(hwnd: HWND, shared: &mut SharedState, packet: Packet) {
        let pressure_u16 = normalize_pressure_u16(
            packet.pk_normal_pressure as i32,
            shared.pressure_min,
            shared.pressure_max,
        );
        let pressure = pressure_u16 as f32 / u16::MAX as f32;
        let button_state = (packet.pk_buttons >> 16) & 0xffff;
        let button_number = packet.pk_buttons & 0xffff;
        let pressure_valid = shared.pressure_max > shared.pressure_min;
        // With lc_pkt_mode=PK_BUTTONS, WinTab reports button changes in
        // pk_buttons rather than a simple held-button bitmask: the high word
        // is TBN_DOWN/TBN_UP and the low word is the changed button number.
        // Treating TBN_DOWN (2) as a bitmask makes every pen-down packet look
        // like button2, which selects the eraser for the first dab.
        let button1 = button_state == TBN_DOWN && button_number == 0;
        let button2 = button_state == TBN_DOWN && button_number == 1;
        let cursor_kind = classify_cursor_id(shared, packet.pk_cursor);
        let flags = WinTabPacketFlags {
            in_range: axis_in_range(packet.pk_x, shared.out_left, shared.out_right)
                && axis_in_range(packet.pk_y, shared.out_top, shared.out_bottom),
            pressure_valid,
            button1,
            button2,
            eraser: cursor_kind == WinTabCursorKind::Eraser,
        };
        let pressed = flags.in_range && ((pressure_valid && pressure_u16 > 0) || flags.button1);
        let last_pressed_before = shared.last_pressed;

        let phase = match (last_pressed_before, pressed) {
            (false, false) => return,
            (false, true) => WinTabPhase::Down,
            (true, true) => WinTabPhase::Move,
            (true, false) => WinTabPhase::Up,
        };

        let mut client_point = POINT {
            x: packet.pk_x,
            y: packet.pk_y,
        };
        unsafe { screen_to_client(hwnd, &mut client_point) };

        shared.last_pressed = pressed && !matches!(phase, WinTabPhase::Up);
        shared.samples.push_back(WinTabSample {
            phase,
            client_x: client_point.x as f32,
            client_y: client_point.y as f32,
            screen_x: packet.pk_x as f32,
            screen_y: packet.pk_y as f32,
            pressure,
            pressure_u16,
            x_fixed_16_16: packet.pk_x.saturating_mul(0x10000),
            y_fixed_16_16: packet.pk_y.saturating_mul(0x10000),
            cursor_kind,
            flags,
            raw_buttons: packet.pk_buttons,
            button_state,
            pressed,
            last_pressed_before,
        });
    }

    fn classify_cursor_id(shared: &SharedState, cursor_id: u32) -> WinTabCursorKind {
        shared
            .cursor_kinds
            .get(cursor_id as usize)
            .copied()
            .unwrap_or(WinTabCursorKind::Pressure)
    }

    unsafe fn refresh_cursor_kinds(shared: &mut SharedState) {
        for cursor_id in 0..MAX_CURSOR_KINDS {
            let mut name = [0i8; 128];
            let bytes = unsafe {
                (shared.api.wt_info_a)(
                    WTI_CURSORS + cursor_id as u32,
                    CSR_NAME,
                    name.as_mut_ptr() as *mut c_void,
                )
            };
            if bytes == 0 {
                shared.cursor_kinds[cursor_id] = WinTabCursorKind::Pressure;
                continue;
            }
            shared.cursor_kinds[cursor_id] = classify_cursor_name(&name);
        }
    }

    fn classify_cursor_name(name: &[i8]) -> WinTabCursorKind {
        let nul = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let bytes: Vec<u8> = name[..nul].iter().map(|&c| c as u8).collect();
        if bytes.is_empty() {
            return WinTabCursorKind::Unknown;
        }
        if ascii_prefix_eq_ci(&bytes, b"Pressure") {
            return WinTabCursorKind::Pressure;
        }
        if ascii_prefix_eq_ci(&bytes, b"Eraser") {
            return WinTabCursorKind::Eraser;
        }
        WinTabCursorKind::Pressure
    }

    fn ascii_prefix_eq_ci(value: &[u8], prefix: &[u8]) -> bool {
        value.len() >= prefix.len()
            && value
                .iter()
                .zip(prefix.iter())
                .all(|(&a, &b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    }

    fn axis_in_range(value: i32, a: i32, b: i32) -> bool {
        let min = a.min(b);
        let max = a.max(b);
        value >= min && value <= max
    }

    fn normalize_pressure_u16(raw: i32, min: i32, max: i32) -> u16 {
        if max <= min {
            return 0;
        }
        let normalized = ((raw - min) as f32 / (max - min) as f32).clamp(0.0, 1.0);
        (normalized * u16::MAX as f32)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16
    }

    unsafe fn query_axis(api: &WinTabApi, category: u32, index: u32) -> Option<Axis> {
        let mut axis: Axis = unsafe { mem::zeroed() };
        let ret =
            unsafe { (api.wt_info_a)(category, index, &mut axis as *mut Axis as *mut c_void) };
        if ret == mem::size_of::<Axis>() as u32 {
            Some(axis)
        } else {
            None
        }
    }

    fn write_lc_name(context: &mut LogContextA, name: &[u8]) {
        for (dst, src) in context.lc_name.iter_mut().zip(name.iter().copied()) {
            *dst = src as i8;
        }
    }

    unsafe fn load_required<T>(module: Hmodule, name: &'static [u8]) -> Result<T, String> {
        match unsafe { GetProcAddress(module, name.as_ptr()) } {
            Some(proc) => Ok(unsafe { mem::transmute_copy(&proc) }),
            None => {
                unsafe { FreeLibrary(module) };
                Err(format!(
                    "WinTab: {} not found",
                    String::from_utf8_lossy(&name[..name.len().saturating_sub(1)])
                ))
            }
        }
    }

    unsafe fn load_optional<T>(module: Hmodule, name: &'static [u8]) -> Option<T> {
        unsafe { GetProcAddress(module, name.as_ptr()) }
            .map(|proc| unsafe { mem::transmute_copy(&proc) })
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::WinTabSample;

    pub struct WinTabInput;

    impl WinTabInput {
        pub fn new(_hwnd: isize) -> Result<Self, String> {
            Err("WinTab: Windows only".to_owned())
        }

        pub fn poll_samples(&mut self) -> Vec<WinTabSample> {
            Vec::new()
        }
    }
}

pub use imp::WinTabInput;

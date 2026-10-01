use std::sync::atomic::{AtomicBool, Ordering};

use crate::ui::input::shortcut_profile::{ShortcutAction, ShortcutBinding, ShortcutProfile};

static IMAGE_PASTE_REQUESTED: AtomicBool = AtomicBool::new(false);
static IMAGE_COPY_REQUESTED: AtomicBool = AtomicBool::new(false);
static IMAGE_CUT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn take_image_paste_request() -> bool {
    IMAGE_PASTE_REQUESTED.swap(false, Ordering::AcqRel)
}

pub fn take_image_copy_request() -> bool {
    IMAGE_COPY_REQUESTED.swap(false, Ordering::AcqRel)
}

pub fn take_image_cut_request() -> bool {
    IMAGE_CUT_REQUESTED.swap(false, Ordering::AcqRel)
}

pub(crate) fn configure_shortcuts(profile: &ShortcutProfile) {
    #[cfg(target_os = "windows")]
    {
        let bindings = profile
            .resolved_bindings()
            .into_iter()
            .filter_map(|(_, binding)| {
                let ShortcutBinding::Invoke { chord, action } = binding else {
                    return None;
                };
                let command = match action {
                    ShortcutAction::CutImage => CtrlCommand::Cut,
                    ShortcutAction::CopyImage => CtrlCommand::Copy,
                    ShortcutAction::PasteImage => CtrlCommand::Paste,
                    _ => return None,
                };
                Some((
                    NativeChord {
                        virtual_key: native_virtual_key(chord.key),
                        ctrl: chord.ctrl,
                        shift: chord.shift,
                        alt: chord.alt,
                    },
                    command,
                ))
            })
            .collect();
        *clipboard_bindings()
            .write()
            .expect("clipboard shortcut lock poisoned") = bindings;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = profile;
}

#[cfg(target_os = "windows")]
pub fn clipboard_sequence_number() -> Option<u32> {
    let sequence =
        unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
    (sequence != 0).then_some(sequence)
}

#[cfg(not(target_os = "windows"))]
pub fn clipboard_sequence_number() -> Option<u32> {
    None
}

#[cfg(target_os = "windows")]
pub fn install_event_loop_hook(options: &mut eframe::NativeOptions) {
    use winit::platform::windows::EventLoopBuilderExtWindows;

    options.event_loop_builder = Some(Box::new(|builder| {
        builder.with_msg_hook(|message| {
            use windows_sys::Win32::UI::{
                Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT},
                WindowsAndMessaging::MSG,
            };

            let message = unsafe { &*(message.cast::<MSG>()) };
            let key_down = |key| unsafe { GetKeyState(key as i32) < 0 };
            match configured_clipboard_command(
                message.message,
                message.wParam,
                message.lParam,
                key_down(VK_CONTROL),
                key_down(VK_SHIFT),
                key_down(VK_MENU),
            ) {
                Some(CtrlCommand::Paste) => {
                    IMAGE_PASTE_REQUESTED.store(true, Ordering::Release);
                }
                Some(CtrlCommand::Copy) => {
                    IMAGE_COPY_REQUESTED.store(true, Ordering::Release);
                }
                Some(CtrlCommand::Cut) => {
                    IMAGE_CUT_REQUESTED.store(true, Ordering::Release);
                }
                None => {}
            }
            false
        });
    }));
}

#[cfg(not(target_os = "windows"))]
pub fn install_event_loop_hook(_options: &mut eframe::NativeOptions) {}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtrlCommand {
    Cut,
    Copy,
    Paste,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeChord {
    virtual_key: usize,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

#[cfg(target_os = "windows")]
fn clipboard_bindings() -> &'static std::sync::RwLock<Vec<(NativeChord, CtrlCommand)>> {
    static BINDINGS: std::sync::OnceLock<std::sync::RwLock<Vec<(NativeChord, CtrlCommand)>>> =
        std::sync::OnceLock::new();
    BINDINGS.get_or_init(|| std::sync::RwLock::new(Vec::new()))
}

#[cfg(target_os = "windows")]
fn configured_clipboard_command(
    message: u32,
    virtual_key: usize,
    lparam: isize,
    ctrl_down: bool,
    shift_down: bool,
    alt_down: bool,
) -> Option<CtrlCommand> {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
    const PREVIOUS_KEY_STATE_BIT: usize = 1 << 30;
    if message != WM_KEYDOWN || (lparam as usize & PREVIOUS_KEY_STATE_BIT) != 0 {
        return None;
    }
    clipboard_bindings()
        .read()
        .expect("clipboard shortcut lock poisoned")
        .iter()
        .find_map(|(chord, command)| {
            (chord.virtual_key == virtual_key
                && chord.ctrl == ctrl_down
                && chord.shift == shift_down
                && chord.alt == alt_down)
                .then_some(*command)
        })
}

#[cfg(target_os = "windows")]
fn native_virtual_key(key: crate::ui::input::shortcut_profile::ShortcutKey) -> usize {
    use crate::ui::input::shortcut_profile::ShortcutKey as Key;
    match key {
        Key::A => 0x41,
        Key::B => 0x42,
        Key::C => 0x43,
        Key::D => 0x44,
        Key::E => 0x45,
        Key::F => 0x46,
        Key::G => 0x47,
        Key::H => 0x48,
        Key::I => 0x49,
        Key::J => 0x4A,
        Key::K => 0x4B,
        Key::L => 0x4C,
        Key::M => 0x4D,
        Key::N => 0x4E,
        Key::O => 0x4F,
        Key::P => 0x50,
        Key::Q => 0x51,
        Key::R => 0x52,
        Key::S => 0x53,
        Key::T => 0x54,
        Key::U => 0x55,
        Key::V => 0x56,
        Key::W => 0x57,
        Key::X => 0x58,
        Key::Y => 0x59,
        Key::Z => 0x5A,
        Key::Num0 => 0x30,
        Key::Num1 => 0x31,
        Key::Num2 => 0x32,
        Key::Num3 => 0x33,
        Key::Num4 => 0x34,
        Key::Num5 => 0x35,
        Key::Num6 => 0x36,
        Key::Num7 => 0x37,
        Key::Num8 => 0x38,
        Key::Num9 => 0x39,
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7A,
        Key::F12 => 0x7B,
        Key::Space => 0x20,
        Key::Enter => 0x0D,
        Key::Escape => 0x1B,
        Key::Tab => 0x09,
        Key::Backspace => 0x08,
        Key::Delete => 0x2E,
        Key::ArrowUp => 0x26,
        Key::ArrowDown => 0x28,
        Key::ArrowLeft => 0x25,
        Key::ArrowRight => 0x27,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::Minus => 0xBD,
        Key::Equals => 0xBB,
        Key::Comma => 0xBC,
        Key::Period => 0xBE,
        Key::LeftBracket => 0xDB,
        Key::RightBracket => 0xDD,
        Key::Slash => 0xBF,
        Key::Backslash => 0xDC,
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{VK_C, VK_V},
        WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP},
    };

    #[test]
    fn configured_profile_controls_clipboard_shortcuts() {
        configure_shortcuts(&ShortcutProfile::load_default().expect("default shortcut profile"));
        assert_eq!(
            configured_clipboard_command(WM_KEYDOWN, usize::from(VK_V), 0, true, false, false),
            Some(CtrlCommand::Paste)
        );
        assert_eq!(
            configured_clipboard_command(WM_KEYDOWN, usize::from(VK_C), 0, true, false, false),
            Some(CtrlCommand::Copy)
        );
        assert_eq!(
            configured_clipboard_command(WM_KEYUP, usize::from(VK_V), 0, true, false, false),
            None
        );
        assert_eq!(
            configured_clipboard_command(
                WM_KEYDOWN,
                usize::from(VK_V),
                1isize << 30,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            configured_clipboard_command(WM_KEYDOWN, usize::from(VK_V), 0, true, true, false),
            None
        );
    }
}

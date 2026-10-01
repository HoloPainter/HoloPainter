pub mod windows_ink;
pub mod wintab;

use eframe::egui;

use crate::settings::TabletBackend;

use self::windows_ink::{WindowsInkInput, WindowsInkPhase, WindowsInkPointerKind};
use self::wintab::{WinTabCursorKind, WinTabInput, WinTabPhase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabletSamplePhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabletPointerKind {
    Pen,
    Eraser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabletInputSource {
    WinTab,
    WindowsInk,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabletSample {
    pub phase: TabletSamplePhase,
    pub client_pos_points: egui::Pos2,
    pub pressure: f32,
    pub kind: TabletPointerKind,
    pub secondary: bool,
    pub source: TabletInputSource,
}

pub struct TabletInputState {
    probed_hwnd: Option<isize>,
    availability_resolved: bool,
    wintab_available: bool,
    windows_ink_available: bool,
    wintab: Option<WinTabInput>,
    windows_ink: Option<WindowsInkInput>,
    wintab_error: Option<String>,
    windows_ink_error: Option<String>,
}

impl Default for TabletInputState {
    fn default() -> Self {
        Self {
            probed_hwnd: None,
            availability_resolved: false,
            wintab_available: false,
            windows_ink_available: false,
            wintab: None,
            windows_ink: None,
            wintab_error: None,
            windows_ink_error: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabletBackendStatus {
    pub availability_resolved: bool,
    pub available_backends: Vec<TabletBackend>,
    pub active_backend: Option<TabletBackend>,
    pub wintab_error: Option<String>,
    pub windows_ink_error: Option<String>,
}

impl TabletBackendStatus {
    pub fn error(&self, backend: TabletBackend) -> Option<&str> {
        match backend {
            TabletBackend::WinTab => self.wintab_error.as_deref(),
            TabletBackend::WindowsInk => self.windows_ink_error.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TabletSyncResult {
    pub normalized_backend: Option<TabletBackend>,
    pub active_backend_changed: bool,
}

impl TabletInputState {
    pub fn sync_backend(
        &mut self,
        hwnd: Option<isize>,
        requested: TabletBackend,
    ) -> TabletSyncResult {
        let previous_active = self.active_backend();
        let Some(hwnd) = hwnd else {
            return TabletSyncResult::default();
        };

        if self.probed_hwnd != Some(hwnd) {
            self.reset_for_hwnd(hwnd);
            self.probe_backends(hwnd);
        }

        let mut selected =
            resolve_backend(requested, self.wintab_available, self.windows_ink_available);
        for _ in 0..TabletBackend::PRIORITY.len() {
            let Some(backend) = selected else {
                self.wintab = None;
                self.windows_ink = None;
                break;
            };
            if self.active_backend() == Some(backend) {
                break;
            }

            self.wintab = None;
            self.windows_ink = None;
            if let Err(error) = self.activate_backend(hwnd, backend) {
                self.mark_unavailable(backend, error);
                selected =
                    resolve_backend(requested, self.wintab_available, self.windows_ink_available);
                continue;
            }
            break;
        }

        let active = self.active_backend();
        TabletSyncResult {
            normalized_backend: active.filter(|backend| *backend != requested),
            active_backend_changed: active != previous_active,
        }
    }

    pub fn status(&self) -> TabletBackendStatus {
        TabletBackendStatus {
            availability_resolved: self.availability_resolved,
            available_backends: TabletBackend::PRIORITY
                .into_iter()
                .filter(|backend| self.is_available(*backend))
                .collect(),
            active_backend: self.active_backend(),
            wintab_error: self.wintab_error.clone(),
            windows_ink_error: self.windows_ink_error.clone(),
        }
    }

    pub fn poll_samples(&mut self, pixels_per_point: f32) -> Vec<TabletSample> {
        if let Some(wintab) = self.wintab.as_mut() {
            return wintab
                .poll_samples()
                .into_iter()
                .map(|sample| tablet_sample_from_wintab(sample, pixels_per_point))
                .collect();
        }
        if let Some(windows_ink) = self.windows_ink.as_mut() {
            return windows_ink
                .poll_samples()
                .into_iter()
                .map(|sample| tablet_sample_from_windows_ink(sample, pixels_per_point))
                .collect();
        }
        Vec::new()
    }

    fn reset_for_hwnd(&mut self, hwnd: isize) {
        self.probed_hwnd = Some(hwnd);
        self.availability_resolved = false;
        self.wintab_available = false;
        self.windows_ink_available = false;
        self.wintab = None;
        self.windows_ink = None;
        self.wintab_error = None;
        self.windows_ink_error = None;
    }

    fn probe_backends(&mut self, hwnd: isize) {
        match WinTabInput::new(hwnd) {
            Ok(input) => {
                drop(input);
                self.wintab_available = true;
            }
            Err(error) => self.wintab_error = Some(error),
        }
        match WindowsInkInput::new(hwnd) {
            Ok(input) => {
                drop(input);
                self.windows_ink_available = true;
            }
            Err(error) => self.windows_ink_error = Some(error),
        }
        self.availability_resolved = true;
    }

    fn activate_backend(&mut self, hwnd: isize, backend: TabletBackend) -> Result<(), String> {
        match backend {
            TabletBackend::WinTab => self.wintab = Some(WinTabInput::new(hwnd)?),
            TabletBackend::WindowsInk => self.windows_ink = Some(WindowsInkInput::new(hwnd)?),
        }
        Ok(())
    }

    fn mark_unavailable(&mut self, backend: TabletBackend, error: String) {
        match backend {
            TabletBackend::WinTab => {
                self.wintab_available = false;
                self.wintab_error = Some(error);
            }
            TabletBackend::WindowsInk => {
                self.windows_ink_available = false;
                self.windows_ink_error = Some(error);
            }
        }
    }

    fn is_available(&self, backend: TabletBackend) -> bool {
        match backend {
            TabletBackend::WinTab => self.wintab_available,
            TabletBackend::WindowsInk => self.windows_ink_available,
        }
    }

    fn active_backend(&self) -> Option<TabletBackend> {
        if self.wintab.is_some() {
            Some(TabletBackend::WinTab)
        } else if self.windows_ink.is_some() {
            Some(TabletBackend::WindowsInk)
        } else {
            None
        }
    }
}

fn resolve_backend(
    requested: TabletBackend,
    wintab_available: bool,
    windows_ink_available: bool,
) -> Option<TabletBackend> {
    let available = |backend| match backend {
        TabletBackend::WinTab => wintab_available,
        TabletBackend::WindowsInk => windows_ink_available,
    };
    available(requested).then_some(requested).or_else(|| {
        TabletBackend::PRIORITY
            .into_iter()
            .find(|backend| available(*backend))
    })
}

fn tablet_sample_from_wintab(sample: wintab::WinTabSample, pixels_per_point: f32) -> TabletSample {
    let pressure = if sample.pressure_u16 == 0 && sample.pressure > 0.0 {
        sample.pressure.clamp(0.0, 1.0)
    } else {
        (sample.pressure_u16 as f32 / u16::MAX as f32).clamp(0.0, 1.0)
    };
    TabletSample {
        phase: match sample.phase {
            WinTabPhase::Down => TabletSamplePhase::Down,
            WinTabPhase::Move => TabletSamplePhase::Move,
            WinTabPhase::Up => TabletSamplePhase::Up,
        },
        client_pos_points: egui::pos2(
            sample.client_x / pixels_per_point,
            sample.client_y / pixels_per_point,
        ),
        pressure,
        kind: match sample.cursor_kind {
            WinTabCursorKind::Eraser => TabletPointerKind::Eraser,
            WinTabCursorKind::Pressure | WinTabCursorKind::Unknown => TabletPointerKind::Pen,
        },
        secondary: sample.flags.eraser || sample.flags.button2,
        source: TabletInputSource::WinTab,
    }
}

fn tablet_sample_from_windows_ink(
    sample: windows_ink::WindowsInkSample,
    pixels_per_point: f32,
) -> TabletSample {
    TabletSample {
        phase: match sample.phase {
            WindowsInkPhase::Down => TabletSamplePhase::Down,
            WindowsInkPhase::Move => TabletSamplePhase::Move,
            WindowsInkPhase::Up => TabletSamplePhase::Up,
        },
        client_pos_points: egui::pos2(
            sample.client_x / pixels_per_point,
            sample.client_y / pixels_per_point,
        ),
        pressure: (sample.pressure_u16 as f32 / u16::MAX as f32).clamp(0.0, 1.0),
        kind: match sample.pointer_kind {
            WindowsInkPointerKind::Pen => TabletPointerKind::Pen,
            WindowsInkPointerKind::Eraser => TabletPointerKind::Eraser,
        },
        secondary: sample.secondary_button,
        source: TabletInputSource::WindowsInk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_resolution_honors_request_then_priority() {
        assert_eq!(
            resolve_backend(TabletBackend::WinTab, true, true),
            Some(TabletBackend::WinTab)
        );
        assert_eq!(
            resolve_backend(TabletBackend::WindowsInk, true, true),
            Some(TabletBackend::WindowsInk)
        );
        assert_eq!(
            resolve_backend(TabletBackend::WinTab, false, true),
            Some(TabletBackend::WindowsInk)
        );
        assert_eq!(
            resolve_backend(TabletBackend::WindowsInk, true, false),
            Some(TabletBackend::WinTab)
        );
        assert_eq!(resolve_backend(TabletBackend::WinTab, false, false), None);
    }
}

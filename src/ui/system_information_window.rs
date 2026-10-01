use crate::localization::Localization;
use eframe::egui;

#[derive(Debug, Clone)]
pub(crate) struct SystemInformation {
    version: String,
    operating_system: String,
    architecture: String,
    gpu: Option<GpuInformation>,
}

#[derive(Debug, Clone)]
struct GpuInformation {
    name: String,
    backend: String,
    device_type: String,
    vendor_id: u32,
    device_id: u32,
    driver: String,
    driver_info: String,
}

impl SystemInformation {
    pub(crate) fn from_creation_context(cc: &eframe::CreationContext<'_>) -> Self {
        let gpu = cc.wgpu_render_state.as_ref().map(|render_state| {
            let info = render_state.adapter.get_info();
            GpuInformation {
                name: info.name,
                backend: format!("{:?}", info.backend),
                device_type: format!("{:?}", info.device_type),
                vendor_id: info.vendor,
                device_id: info.device,
                driver: info.driver,
                driver_info: info.driver_info,
            }
        });
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            operating_system: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            gpu,
        }
    }
}

pub(crate) fn draw_system_information_window(
    ctx: &egui::Context,
    open: &mut bool,
    l10n: &Localization,
    information: &SystemInformation,
) {
    if !*open {
        return;
    }
    egui::Window::new(l10n.text("system-information-title"))
        .id(egui::Id::new("system_information_window"))
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("system_information_grid")
                .num_columns(2)
                .spacing([20.0, 6.0])
                .show(ui, |ui| {
                    row(ui, l10n.text("app-name"), &information.version);
                    row(
                        ui,
                        l10n.text("system-information-os"),
                        &information.operating_system,
                    );
                    row(
                        ui,
                        l10n.text("system-information-architecture"),
                        &information.architecture,
                    );
                    if let Some(gpu) = &information.gpu {
                        row(ui, l10n.text("system-information-gpu"), &gpu.name);
                        row(ui, l10n.text("system-information-backend"), &gpu.backend);
                        row(
                            ui,
                            l10n.text("system-information-device-type"),
                            &gpu.device_type,
                        );
                        row(
                            ui,
                            l10n.text("system-information-vendor-id"),
                            &format!("0x{:04X}", gpu.vendor_id),
                        );
                        row(
                            ui,
                            l10n.text("system-information-device-id"),
                            &format!("0x{:04X}", gpu.device_id),
                        );
                        row(
                            ui,
                            l10n.text("system-information-driver"),
                            value_or_unavailable(l10n, &gpu.driver),
                        );
                        row(
                            ui,
                            l10n.text("system-information-driver-info"),
                            value_or_unavailable(l10n, &gpu.driver_info),
                        );
                    } else {
                        row(
                            ui,
                            l10n.text("system-information-gpu"),
                            l10n.text("system-information-unavailable"),
                        );
                    }
                });
        });
}

fn row(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, value: impl Into<egui::WidgetText>) {
    ui.label(label);
    ui.add(egui::Label::new(value).selectable(true).wrap());
    ui.end_row();
}

fn value_or_unavailable(l10n: &Localization, value: &str) -> String {
    if value.trim().is_empty() {
        l10n.text("system-information-unavailable")
    } else {
        value.to_owned()
    }
}

use eframe::egui;

use crate::{
    licensing::{ProLicenseState, PurchaseOutcome, PurchaseState},
    localization::Localization,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProUnlockAction {
    None,
    Close,
    Purchase,
    Refresh,
    ExportPsd,
}

pub fn draw_pro_unlock_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    state: &ProLicenseState,
    purchase_state: PurchaseState,
    formatted_price: Option<&str>,
    product_error: Option<&str>,
    purchase_outcome: Option<PurchaseOutcome>,
) -> ProUnlockAction {
    let mut action = ProUnlockAction::None;
    let response = egui::Modal::new(egui::Id::new("pro_unlock_dialog")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading(l10n.text("pro-unlock-title"));
        ui.add_space(8.0);
        ui.label(l10n.text("pro-unlock-description"));
        ui.label(format!("• {}", l10n.text("pro-unlock-feature-psd")));
        ui.add_space(8.0);

        match state {
            ProLicenseState::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(l10n.text("pro-unlock-checking"));
                });
            }
            ProLicenseState::Owned => {
                ui.label(l10n.text("pro-unlock-owned"));
            }
            ProLicenseState::Free => {
                if let Some(price) = formatted_price {
                    ui.label(l10n.format(
                        "pro-unlock-price",
                        Some(&fluent::FluentArgs::from_iter([("price", price)])),
                    ));
                } else if let Some(error) = product_error {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        l10n.text("pro-unlock-product-unavailable"),
                    );
                    ui.small(error);
                } else {
                    ui.label(l10n.text("pro-unlock-price-loading"));
                }
            }
            ProLicenseState::Unavailable { error } => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    l10n.text("pro-unlock-unavailable"),
                );
                ui.small(error);
            }
        }

        if let Some(outcome) = purchase_outcome {
            ui.add_space(6.0);
            let key = match outcome {
                PurchaseOutcome::Purchased => "pro-unlock-purchased",
                PurchaseOutcome::Cancelled => "pro-unlock-cancelled",
                PurchaseOutcome::NetworkError => "pro-unlock-network-error",
                PurchaseOutcome::ServerError => "pro-unlock-server-error",
            };
            ui.label(l10n.text(key));
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(l10n.text("pro-unlock-close")).clicked() {
                action = ProUnlockAction::Close;
            }

            match state {
                ProLicenseState::Owned => {
                    if ui.button(l10n.text("pro-unlock-export-psd")).clicked() {
                        action = ProUnlockAction::ExportPsd;
                    }
                }
                ProLicenseState::Free => {
                    let purchasing = purchase_state == PurchaseState::Purchasing;
                    if ui
                        .add_enabled(
                            !purchasing && formatted_price.is_some() && product_error.is_none(),
                            egui::Button::new(if purchasing {
                                l10n.text("pro-unlock-purchasing")
                            } else {
                                l10n.text("pro-unlock-purchase")
                            }),
                        )
                        .clicked()
                    {
                        action = ProUnlockAction::Purchase;
                    }
                    if ui
                        .add_enabled(
                            !purchasing,
                            egui::Button::new(l10n.text("pro-unlock-refresh")),
                        )
                        .clicked()
                    {
                        action = ProUnlockAction::Refresh;
                    }
                }
                ProLicenseState::Unavailable { .. } => {
                    if ui.button(l10n.text("pro-unlock-refresh")).clicked() {
                        action = ProUnlockAction::Refresh;
                    }
                }
                ProLicenseState::Checking => {}
            }
        });
    });
    if response.should_close() && action == ProUnlockAction::None {
        ProUnlockAction::Close
    } else {
        action
    }
}

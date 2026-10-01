use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use windows::{
    Foundation::TypedEventHandler,
    Services::Store::{
        StoreAppLicense, StoreContext, StoreProductQueryResult, StorePurchaseResult,
        StorePurchaseStatus,
    },
    Win32::{
        Foundation::HWND,
        System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize},
        UI::Shell::IInitializeWithWindow,
    },
    core::{HRESULT, HSTRING, Interface},
};
use windows_collections::IIterable;

use super::{PRO_UNLOCK_PRODUCT_ID, ProProductInfo, PurchaseOutcome};

const RO_E_CHANGED_MODE: HRESULT = HRESULT(0x8001_0106_u32 as i32);

#[derive(Debug)]
pub enum StoreEvent {
    RefreshRequested,
    License {
        generation: u64,
        result: Result<bool, String>,
    },
    Product {
        generation: u64,
        result: Result<ProProductInfo, String>,
    },
    Purchase(Result<PurchaseOutcome, String>),
}

pub struct StoreBridge {
    context: Option<StoreContext>,
    receiver: Receiver<StoreEvent>,
    sender: Sender<StoreEvent>,
    license_change_token: Option<i64>,
    owns_windows_runtime_initialization: bool,
}

impl std::fmt::Debug for StoreBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoreBridge")
            .field("initialized", &self.context.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for StoreBridge {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            context: None,
            receiver,
            sender,
            license_change_token: None,
            owns_windows_runtime_initialization: false,
        }
    }
}

impl StoreBridge {
    pub fn is_initialized(&self) -> bool {
        self.context.is_some()
    }

    pub fn initialize(&mut self, hwnd: isize, egui_ctx: egui::Context) -> Result<(), String> {
        if self.context.is_some() {
            return Ok(());
        }

        self.owns_windows_runtime_initialization =
            match unsafe { RoInitialize(RO_INIT_SINGLETHREADED) } {
                Ok(()) => true,
                Err(error) if error.code() == RO_E_CHANGED_MODE => false,
                Err(error) => return Err(store_error(error)),
            };
        let result = (|| {
            let context = StoreContext::GetDefault().map_err(store_error)?;
            let initialize: IInitializeWithWindow = context.cast().map_err(store_error)?;
            unsafe { initialize.Initialize(HWND(hwnd as *mut _)) }.map_err(store_error)?;

            let sender = self.sender.clone();
            let repaint = egui_ctx.clone();
            let handler = TypedEventHandler::new(move |_, _| {
                let _ = sender.send(StoreEvent::RefreshRequested);
                repaint.request_repaint();
                Ok(())
            });
            self.license_change_token = Some(
                context
                    .OfflineLicensesChanged(&handler)
                    .map_err(store_error)?,
            );
            self.context = Some(context);
            Ok(())
        })();
        if result.is_err() && self.owns_windows_runtime_initialization {
            unsafe { RoUninitialize() };
            self.owns_windows_runtime_initialization = false;
        }
        result
    }

    pub fn refresh(&self, generation: u64, egui_ctx: egui::Context) -> Result<(), String> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| "Microsoft Store context is not initialized".to_owned())?;

        let sender = self.sender.clone();
        let repaint = egui_ctx.clone();
        context
            .GetAppLicenseAsync()
            .map_err(store_error)?
            .when(move |result| {
                let result = result.map_err(store_error).and_then(pro_is_owned);
                let _ = sender.send(StoreEvent::License { generation, result });
                repaint.request_repaint();
            })
            .map_err(store_error)?;

        let offer_tokens: IIterable<HSTRING> = vec![HSTRING::from(PRO_UNLOCK_PRODUCT_ID)].into();
        let sender = self.sender.clone();
        context
            .GetAssociatedStoreProductsByInAppOfferTokenAsync(&offer_tokens)
            .map_err(store_error)?
            .when(move |result| {
                let result = result.map_err(store_error).and_then(pro_product_info);
                let _ = sender.send(StoreEvent::Product { generation, result });
                egui_ctx.request_repaint();
            })
            .map_err(store_error)
    }

    pub fn request_purchase(&self, store_id: &str, egui_ctx: egui::Context) -> Result<(), String> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| "Microsoft Store context is not initialized".to_owned())?;
        let sender = self.sender.clone();
        context
            .RequestPurchaseAsync(&HSTRING::from(store_id))
            .map_err(store_error)?
            .when(move |result| {
                let result = result.map_err(store_error).and_then(purchase_outcome);
                let _ = sender.send(StoreEvent::Purchase(result));
                egui_ctx.request_repaint();
            })
            .map_err(store_error)
    }

    pub fn poll(&self) -> Option<StoreEvent> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for StoreBridge {
    fn drop(&mut self) {
        if let (Some(context), Some(token)) = (&self.context, self.license_change_token) {
            let _ = context.RemoveOfflineLicensesChanged(token);
        }
        if self.owns_windows_runtime_initialization {
            unsafe { RoUninitialize() };
        }
    }
}

fn pro_is_owned(license: StoreAppLicense) -> Result<bool, String> {
    let licenses = license.AddOnLicenses().map_err(store_error)?;
    for pair in &licenses {
        let add_on = pair.Value().map_err(store_error)?;
        if add_on.InAppOfferToken().map_err(store_error)? == PRO_UNLOCK_PRODUCT_ID
            && add_on.IsActive().map_err(store_error)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn pro_product_info(result: StoreProductQueryResult) -> Result<ProProductInfo, String> {
    let extended_error = result.ExtendedError().map_err(store_error)?;
    if extended_error.is_err() {
        return Err(format!(
            "Microsoft Store product query failed: {extended_error:?}"
        ));
    }
    for pair in &result.Products().map_err(store_error)? {
        let product = pair.Value().map_err(store_error)?;
        if product.InAppOfferToken().map_err(store_error)? == PRO_UNLOCK_PRODUCT_ID {
            let store_id = product.StoreId().map_err(store_error)?.to_string();
            let formatted_price = product
                .Price()
                .and_then(|price| price.FormattedPrice())
                .map(|price| price.to_string())
                .map_err(store_error)?;
            return Ok(ProProductInfo {
                store_id,
                formatted_price,
            });
        }
    }
    Err(format!(
        "Microsoft Store add-on {PRO_UNLOCK_PRODUCT_ID:?} was not found"
    ))
}

fn purchase_outcome(result: StorePurchaseResult) -> Result<PurchaseOutcome, String> {
    let status = result.Status().map_err(store_error)?;
    if status == StorePurchaseStatus::Succeeded || status == StorePurchaseStatus::AlreadyPurchased {
        Ok(PurchaseOutcome::Purchased)
    } else if status == StorePurchaseStatus::NotPurchased {
        Ok(PurchaseOutcome::Cancelled)
    } else if status == StorePurchaseStatus::NetworkError {
        Ok(PurchaseOutcome::NetworkError)
    } else if status == StorePurchaseStatus::ServerError {
        Ok(PurchaseOutcome::ServerError)
    } else {
        let extended_error = result.ExtendedError().map_err(store_error)?;
        Err(format!(
            "Microsoft Store returned purchase status {status:?}: {extended_error:?}"
        ))
    }
}

fn store_error(error: windows::core::Error) -> String {
    format!("{} ({:?})", error.message(), error.code())
}

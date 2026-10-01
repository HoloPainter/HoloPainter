//! Microsoft Store entitlement state for optional Pro features.

use eframe::egui;

pub const PRO_UNLOCK_PRODUCT_ID: &str = "pro_unlock";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProProductInfo {
    store_id: String,
    formatted_price: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProLicenseState {
    Checking,
    Free,
    Owned,
    Unavailable { error: String },
}

impl ProLicenseState {
    pub const fn owns_pro(&self) -> bool {
        matches!(self, Self::Owned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseState {
    Idle,
    Purchasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseOutcome {
    Purchased,
    Cancelled,
    NetworkError,
    ServerError,
}

#[derive(Debug)]
pub struct ProLicenseRuntime {
    state: ProLicenseState,
    purchase_state: PurchaseState,
    store_id: Option<String>,
    formatted_price: Option<String>,
    product_error: Option<String>,
    purchase_outcome: Option<PurchaseOutcome>,
    bridge: platform::StoreBridge,
    license_generation: u64,
}

impl Default for ProLicenseRuntime {
    fn default() -> Self {
        Self {
            state: ProLicenseState::Checking,
            purchase_state: PurchaseState::Idle,
            store_id: None,
            formatted_price: None,
            product_error: None,
            purchase_outcome: None,
            bridge: platform::StoreBridge::default(),
            license_generation: 0,
        }
    }
}

impl ProLicenseRuntime {
    pub const fn state(&self) -> &ProLicenseState {
        &self.state
    }

    pub const fn purchase_state(&self) -> PurchaseState {
        self.purchase_state
    }

    pub fn formatted_price(&self) -> Option<&str> {
        self.formatted_price.as_deref()
    }

    pub fn product_error(&self) -> Option<&str> {
        self.product_error.as_deref()
    }

    pub const fn purchase_outcome(&self) -> Option<PurchaseOutcome> {
        self.purchase_outcome
    }

    pub fn clear_purchase_outcome(&mut self) {
        self.purchase_outcome = None;
    }

    pub fn initialize(&mut self, hwnd: Option<isize>, egui_ctx: &egui::Context) {
        if self.bridge.is_initialized() {
            return;
        }
        let Some(hwnd) = hwnd else {
            return;
        };
        match self.bridge.initialize(hwnd, egui_ctx.clone()) {
            Ok(()) => self.refresh(egui_ctx),
            Err(error) => self.apply_license_result(self.license_generation, Err(error)),
        }
    }

    pub fn refresh(&mut self, egui_ctx: &egui::Context) {
        self.license_generation = self.license_generation.wrapping_add(1);
        let generation = self.license_generation;
        if !self.state.owns_pro() {
            self.state = ProLicenseState::Checking;
        }
        if let Err(error) = self.bridge.refresh(generation, egui_ctx.clone()) {
            self.apply_license_result(generation, Err(error));
        }
    }

    pub fn retry(&mut self, hwnd: Option<isize>, egui_ctx: &egui::Context) {
        self.purchase_outcome = None;
        self.product_error = None;
        self.state = ProLicenseState::Checking;
        if self.bridge.is_initialized() {
            self.refresh(egui_ctx);
        } else {
            self.initialize(hwnd, egui_ctx);
        }
    }

    pub fn request_purchase(&mut self, egui_ctx: &egui::Context) {
        self.purchase_outcome = None;
        let Some(store_id) = self.store_id.as_deref() else {
            self.purchase_state = PurchaseState::Idle;
            if !self.state.owns_pro() {
                self.state = ProLicenseState::Unavailable {
                    error: "Microsoft Store product information is not available".to_owned(),
                };
            }
            return;
        };
        match self.bridge.request_purchase(store_id, egui_ctx.clone()) {
            Ok(()) => self.purchase_state = PurchaseState::Purchasing,
            Err(error) => {
                self.purchase_state = PurchaseState::Idle;
                if !self.state.owns_pro() {
                    self.state = ProLicenseState::Unavailable { error };
                }
            }
        }
    }

    pub fn poll(&mut self, egui_ctx: &egui::Context) -> bool {
        let mut changed = false;
        while let Some(event) = self.bridge.poll() {
            changed = true;
            match event {
                platform::StoreEvent::RefreshRequested => self.refresh(egui_ctx),
                platform::StoreEvent::License { generation, result } => {
                    self.apply_license_result(generation, result);
                }
                platform::StoreEvent::Product { generation, result }
                    if generation == self.license_generation =>
                {
                    match result {
                        Ok(product) => {
                            self.store_id = Some(product.store_id);
                            self.formatted_price = Some(product.formatted_price);
                            self.product_error = None;
                        }
                        Err(error) => {
                            eprintln!("Unable to load HoloPainter Pro price: {error}");
                            self.store_id = None;
                            self.formatted_price = None;
                            self.product_error = Some(error);
                        }
                    }
                }
                platform::StoreEvent::Product { .. } => {}
                platform::StoreEvent::Purchase(result) => {
                    self.purchase_state = PurchaseState::Idle;
                    match result {
                        Ok(PurchaseOutcome::Purchased) => {
                            self.purchase_outcome = Some(PurchaseOutcome::Purchased);
                            self.refresh(egui_ctx);
                        }
                        Ok(outcome) => self.purchase_outcome = Some(outcome),
                        Err(error) => {
                            self.purchase_outcome = Some(PurchaseOutcome::ServerError);
                            if !self.state.owns_pro() {
                                self.state = ProLicenseState::Unavailable { error };
                            }
                        }
                    }
                }
            }
        }
        changed
    }

    fn apply_license_result(&mut self, generation: u64, result: Result<bool, String>) {
        if generation != self.license_generation {
            return;
        }
        match result {
            Ok(true) => self.state = ProLicenseState::Owned,
            Ok(false) => self.state = ProLicenseState::Free,
            Err(error) if self.state.owns_pro() => {
                eprintln!("Unable to refresh HoloPainter Pro license: {error}");
            }
            Err(error) => self.state = ProLicenseState::Unavailable { error },
        }
    }
}

#[cfg(target_os = "windows")]
mod platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use eframe::egui;

    use super::{PRO_UNLOCK_PRODUCT_ID, ProProductInfo, PurchaseOutcome};

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

    #[derive(Debug, Default)]
    pub struct StoreBridge;

    impl StoreBridge {
        pub const fn is_initialized(&self) -> bool {
            false
        }

        pub fn initialize(&mut self, _hwnd: isize, _ctx: egui::Context) -> Result<(), String> {
            Err(format!(
                "Microsoft Store licensing for {PRO_UNLOCK_PRODUCT_ID} is available only on Windows"
            ))
        }

        pub fn refresh(&self, _generation: u64, _ctx: egui::Context) -> Result<(), String> {
            Err("Microsoft Store licensing is unavailable on this platform".to_owned())
        }

        pub fn request_purchase(&self, _store_id: &str, _ctx: egui::Context) -> Result<(), String> {
            Err("Microsoft Store purchases are unavailable on this platform".to_owned())
        }

        pub fn poll(&self) -> Option<StoreEvent> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProLicenseRuntime, ProLicenseState};

    #[test]
    fn only_owned_state_enables_pro_features() {
        assert!(!ProLicenseState::Checking.owns_pro());
        assert!(!ProLicenseState::Free.owns_pro());
        assert!(
            !ProLicenseState::Unavailable {
                error: "offline".to_owned()
            }
            .owns_pro()
        );
        assert!(ProLicenseState::Owned.owns_pro());
    }

    #[test]
    fn a_definitive_license_result_replaces_the_previous_state() {
        let mut runtime = ProLicenseRuntime::default();
        runtime.apply_license_result(0, Ok(false));
        assert_eq!(runtime.state(), &ProLicenseState::Free);

        runtime.apply_license_result(0, Ok(true));
        assert_eq!(runtime.state(), &ProLicenseState::Owned);

        runtime.apply_license_result(0, Ok(false));
        assert_eq!(runtime.state(), &ProLicenseState::Free);
    }

    #[test]
    fn a_transient_refresh_error_does_not_revoke_verified_ownership() {
        let mut runtime = ProLicenseRuntime::default();
        runtime.apply_license_result(0, Ok(true));
        runtime.apply_license_result(0, Err("offline".to_owned()));
        assert_eq!(runtime.state(), &ProLicenseState::Owned);
    }

    #[test]
    fn an_initial_store_error_does_not_unlock_pro() {
        let mut runtime = ProLicenseRuntime::default();
        runtime.apply_license_result(0, Err("unavailable".to_owned()));
        assert_eq!(
            runtime.state(),
            &ProLicenseState::Unavailable {
                error: "unavailable".to_owned()
            }
        );
    }

    #[test]
    fn stale_license_result_does_not_replace_newer_state() {
        let mut runtime = ProLicenseRuntime::default();
        runtime.license_generation = 2;
        runtime.apply_license_result(2, Ok(true));
        runtime.apply_license_result(1, Ok(false));
        assert_eq!(runtime.state(), &ProLicenseState::Owned);
    }
}

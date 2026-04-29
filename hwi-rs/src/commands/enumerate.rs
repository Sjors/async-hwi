//! `enumerate` — list connected signers as JSON.

use async_hwi::ledger::{HidApi, LedgerSimulator};

use crate::devices::ledger::{ledger_iface_ok, ledger_model, use_simulator, LEDGER_VENDOR_ID};
use crate::devices::mock::MockDevice;
use crate::devices::DeviceEntry;

pub async fn run_enumerate() -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.enumerate();
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        let mut entry = DeviceEntry {
            kind: "ledger",
            model: "ledger_nano_x".to_string(),
            label: None,
            path: "speculos://127.0.0.1:9999".to_string(),
            fingerprint: None,
            needs_pin_sent: false,
            needs_passphrase_sent: false,
            error: None,
        };
        match async_hwi::HWI::get_master_fingerprint(&device).await {
            Ok(fp) => entry.fingerprint = Some(format!("{fp:x}")),
            Err(e) => entry.error = Some(format!("get_master_fingerprint: {e:?}")),
        }
        return serde_json::to_string(&[entry]).map_err(|e| e.to_string());
    }
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let mut entries: Vec<DeviceEntry> = Vec::new();

    for info in api.device_list() {
        if info.vendor_id() != LEDGER_VENDOR_ID {
            continue;
        }
        if !ledger_iface_ok(info) {
            continue;
        }
        let model = match ledger_model(info.product_id()) {
            Some(m) => m,
            None => continue,
        };
        let path = info
            .path()
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_default();

        let mut entry = DeviceEntry {
            kind: "ledger",
            model: model.to_string(),
            label: None,
            path,
            fingerprint: None,
            needs_pin_sent: false,
            needs_passphrase_sent: false,
            error: None,
        };

        match async_hwi::ledger::Ledger::<async_hwi::ledger::TransportHID>::connect(&api, info) {
            Ok(device) => match async_hwi::HWI::get_master_fingerprint(&device).await {
                Ok(fp) => entry.fingerprint = Some(format!("{fp:x}")),
                Err(e) => entry.error = Some(format!("get_master_fingerprint: {e:?}")),
            },
            Err(e) => entry.error = Some(format!("connect: {e:?}")),
        }

        entries.push(entry);
    }

    serde_json::to_string(&entries).map_err(|e| e.to_string())
}

//! `enumerate` — list connected signers as JSON.

use async_hwi::coldcard::api as ckcc;
use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::devices::coldcard::{
    coldcard_model, open_simulator as open_cc_simulator, use_simulator as use_cc_simulator,
    SIMULATOR_SOCKET,
};
use crate::devices::ledger::{ledger_iface_ok, ledger_model, use_simulator, LEDGER_VENDOR_ID};
use crate::devices::mock::MockDevice;
use crate::devices::DeviceEntry;

const COINKITE_VID: u16 = 0xd13e;

pub async fn run_enumerate() -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.enumerate();
    }

    // Simulator paths: when either (or both) of HWI_RS_LEDGER_SIMULATOR
    // and HWI_RS_COLDCARD_SIMULATOR is set, return the corresponding
    // simulated devices and skip HID enumeration entirely. The kumbaya
    // 3-of-3 MuSig2 scenario sets both at once.
    let mut sim_entries: Vec<DeviceEntry> = Vec::new();
    if use_simulator() {
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
        match LedgerSimulator::try_connect().await {
            Ok(device) => match async_hwi::HWI::get_master_fingerprint(&device).await {
                Ok(fp) => entry.fingerprint = Some(format!("{fp:x}")),
                Err(e) => entry.error = Some(format!("get_master_fingerprint: {e:?}")),
            },
            Err(e) => entry.error = Some(format!("speculos connect: {e:?}")),
        }
        sim_entries.push(entry);
    }
    if use_cc_simulator() {
        let mut entry = DeviceEntry {
            kind: "coldcard",
            model: "coldcard_simulator".to_string(),
            label: None,
            path: format!("unix://{SIMULATOR_SOCKET}"),
            fingerprint: None,
            needs_pin_sent: false,
            needs_passphrase_sent: false,
            error: None,
        };
        match open_cc_simulator() {
            Ok((mut cc, fp)) => {
                entry.fingerprint = Some(format!("{fp:x}"));
                if let Ok(v) = cc.version() {
                    entry.model = coldcard_model(&v);
                }
            }
            Err(e) => entry.error = Some(e),
        }
        sim_entries.push(entry);
    }
    if !sim_entries.is_empty() {
        return serde_json::to_string(&sim_entries).map_err(|e| e.to_string());
    }

    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let mut entries: Vec<DeviceEntry> = Vec::new();

    // Snapshot the device list up front so we can later borrow `api`
    // mutably for the Coldcard `Api::from_borrowed` enumerator.
    let infos: Vec<_> = api.device_list().cloned().collect();

    for info in &infos {
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

    // Coldcard: enumerate via the vendored crate's own detector, which
    // reads the SoftCard-style serial number from each HID interface.
    if infos.iter().any(|i| i.vendor_id() == COINKITE_VID) {
        let mut ck_api = ckcc::Api::from_borrowed(&mut api);
        if let Ok(serials) = ck_api.detect() {
            for sn in serials {
                let path = sn.as_ref().to_string();
                let mut entry = DeviceEntry {
                    kind: "coldcard",
                    model: "coldcard".to_string(),
                    label: None,
                    path,
                    fingerprint: None,
                    needs_pin_sent: false,
                    needs_passphrase_sent: false,
                    error: None,
                };
                match ck_api.open(&sn, None) {
                    Ok((mut cc, info)) => {
                        if let Some(info) = info {
                            entry.fingerprint =
                                Some(format!("{:x}", Fingerprint::from(info.fingerprint)));
                        }
                        if let Ok(v) = cc.version() {
                            entry.model = coldcard_model(&v);
                        }
                    }
                    Err(e) => entry.error = Some(format!("coldcard open: {e:?}")),
                }
                entries.push(entry);
            }
        }
    }

    serde_json::to_string(&entries).map_err(|e| e.to_string())
}

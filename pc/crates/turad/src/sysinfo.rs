//! Host facts the phone needs: hostname and wired MACs for Wake on LAN.

use std::fs;

pub fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "unknown".into())
}

/// MACs of physical Ethernet interfaces. Wi-Fi and virtual interfaces are
/// skipped because Wake on LAN practically only works over a cable.
pub fn wol_macs() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else { return vec![] };
    let mut macs: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let is_physical = p.join("device").exists();
            let is_wireless = p.join("wireless").exists();
            // ARPHRD_ETHER == 1
            let is_ether = fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "1");
            if !(is_physical && is_ether && !is_wireless) {
                return None;
            }
            let mac = fs::read_to_string(p.join("address")).ok()?.trim().to_owned();
            (mac != "00:00:00:00:00:00").then_some(mac)
        })
        .collect();
    macs.sort();
    macs
}

//! Device names from the system `pci.ids` database (pciutils / hwdata).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Distribution locations, relative to the host root.
const PATHS: [&str; 4] = [
    "usr/share/hwdata/pci.ids",
    "usr/share/misc/pci.ids",
    "usr/share/pci.ids",
    "run/current-system/sw/share/hwdata/pci.ids",
];

/// Memoised lookups, so identical boards scan the database once.
#[derive(Default)]
pub(crate) struct Cache(HashMap<(u16, u16), Option<String>>);

impl Cache {
    pub fn device_name(&mut self, root: &Path, vendor: u16, device: u16) -> Option<String> {
        self.0
            .entry((vendor, device))
            .or_insert_with(|| {
                PATHS.iter().find_map(|p| {
                    let file = File::open(root.join(p)).ok()?;
                    lookup(BufReader::new(file), vendor, device)
                })
            })
            .clone()
    }
}

/// Find `device` under `vendor`. Vendor lines are unindented, device lines
/// have one tab, subsystem lines two; the file is sorted by vendor.
fn lookup(reader: impl BufRead, vendor: u16, device: u16) -> Option<String> {
    let vendor = format!("{vendor:04x}  ");
    let device = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in reader.split(b'\n') {
        let line = line.ok()?;
        let line = String::from_utf8_lossy(&line);
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&vendor);
        } else if in_vendor {
            if let Some(name) = line.strip_prefix(&device) {
                return Some(name.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "# comment\n\
1000  Broadcom / LSI\n\
\t7551  Not this one\n\
1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
\t7550  Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]\n\
\t\t1002 7550  Radeon RX 9070 XT\n\
\t7551  Navi 48 [Radeon AI PRO R9700]\n\
1003  ULSI Systems\n";

    #[test]
    fn finds_device_within_vendor_only() {
        let find = |d| lookup(DB.as_bytes(), 0x1002, d);
        assert_eq!(
            find(0x7551).as_deref(),
            Some("Navi 48 [Radeon AI PRO R9700]")
        );
        assert_eq!(
            find(0x7550).as_deref(),
            Some("Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]")
        );
        assert_eq!(find(0x1234), None);
        assert_eq!(lookup(DB.as_bytes(), 0x1003, 0x7551), None);
    }
}

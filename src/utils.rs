#[cfg(target_os = "linux")]
pub fn wsl() -> bool {
    if let Ok(b) = std::fs::read("/proc/sys/kernel/osrelease") {
        if let Ok(s) = std::str::from_utf8(&b) {
            let a = s.to_ascii_lowercase();
            return a.contains("microsoft") || a.contains("wsl");
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub fn wsl() -> bool {
    false
}

//! What this replica calls itself in its device record (glonOdin
//! `core/device.odin`, Settings → "Your devices"): its kind and the name it
//! writes until a person renames it.

/// A device record's `kind` and auto-generated `name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// "browser", "iphone" or "desktop" (a daemon is "engine").
    pub kind: &'static str,
    pub name: String,
}

impl DeviceInfo {
    /// A browser tab: "Chrome at roostr.space".
    pub fn browser(user_agent: &str, host: &str) -> Self {
        let host = host.trim();
        let browser = browser_name(user_agent);
        Self {
            kind: "browser",
            name: if host.is_empty() {
                browser.to_string()
            } else {
                format!("{browser} at {host}")
            },
        }
    }

    /// The desktop app's replica: "Desktop app on <hostname>".
    pub fn desktop(hostname: &str) -> Self {
        let host = hostname.trim().trim_end_matches(".local");
        Self {
            kind: "desktop",
            name: if host.is_empty() {
                "Desktop app".into()
            } else {
                format!("Desktop app on {host}")
            },
        }
    }

    /// The iOS app, from the hardware identifier (`hw.machine`, e.g.
    /// "iPhone15,2"): the model's name when known, else "iPhone" / "iPad".
    pub fn iphone(machine: &str) -> Self {
        let name = iphone_model(machine)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if machine.starts_with("iPad") {
                    "iPad".into()
                } else {
                    "iPhone".into()
                }
            });
        Self {
            kind: "iphone",
            name,
        }
    }

    /// This process's device: the browser in wasm, the iPhone on iOS, the
    /// desktop app elsewhere.
    pub fn this_platform() -> Self {
        #[cfg(target_family = "wasm")]
        {
            let window = web_sys::window();
            let agent = window
                .as_ref()
                .and_then(|w| w.navigator().user_agent().ok())
                .unwrap_or_default();
            let host = window
                .as_ref()
                .and_then(|w| w.location().host().ok())
                .unwrap_or_default();
            Self::browser(&agent, &host)
        }
        #[cfg(target_os = "ios")]
        {
            Self::iphone(&ios_machine())
        }
        #[cfg(not(any(target_family = "wasm", target_os = "ios")))]
        {
            Self::desktop(&gethostname::gethostname().to_string_lossy())
        }
    }
}

/// The browser a user agent names; order matters (Edge and Opera also say
/// Chrome, Chrome also says Safari).
fn browser_name(user_agent: &str) -> &'static str {
    let has = |needle: &str| user_agent.contains(needle);
    if has("Edg/") || has("EdgiOS/") || has("EdgA/") {
        "Edge"
    } else if has("OPR/") || has("Opera") {
        "Opera"
    } else if has("Firefox/") || has("FxiOS/") {
        "Firefox"
    } else if has("CriOS/") || has("Chrome/") || has("Chromium/") {
        "Chrome"
    } else if has("Safari/") {
        "Safari"
    } else {
        "Browser"
    }
}

/// iPhone hardware identifiers → model names.
fn iphone_model(machine: &str) -> Option<&'static str> {
    Some(match machine {
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,6" => "iPhone SE",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    })
}

/// `hw.machine` ("iPhone15,2"; the simulator answers its host's "arm64").
#[cfg(target_os = "ios")]
fn ios_machine() -> String {
    let name = c"hw.machine";
    let mut size: libc::size_t = 0;
    // SAFETY: sysctlbyname with a null buffer only reports the size.
    let sized = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if sized != 0 || size == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` holds `size` bytes, as the first call reported.
    let read = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_names() {
        let chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        assert_eq!(
            DeviceInfo::browser(chrome, "roostr.space").name,
            "Chrome at roostr.space"
        );
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
        assert_eq!(
            DeviceInfo::browser(safari, "roostr.space").name,
            "Safari at roostr.space"
        );
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
        assert_eq!(
            DeviceInfo::browser(firefox, "localhost:8090").name,
            "Firefox at localhost:8090"
        );
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
        assert_eq!(
            DeviceInfo::browser(edge, "roostr.space").name,
            "Edge at roostr.space"
        );
        let chrome_ios = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0 Mobile/15E148 Safari/604.1";
        assert_eq!(
            DeviceInfo::browser(chrome_ios, "roostr.space").name,
            "Chrome at roostr.space"
        );
        assert_eq!(DeviceInfo::browser("", "").name, "Browser");
        assert_eq!(DeviceInfo::browser("", "").kind, "browser");
    }

    #[test]
    fn desktop_and_iphone_names() {
        assert_eq!(
            DeviceInfo::desktop("Grants-MacBook-Pro.local").name,
            "Desktop app on Grants-MacBook-Pro"
        );
        assert_eq!(DeviceInfo::desktop("").name, "Desktop app");
        assert_eq!(DeviceInfo::desktop("x").kind, "desktop");
        assert_eq!(DeviceInfo::iphone("iPhone15,2").name, "iPhone 14 Pro");
        assert_eq!(DeviceInfo::iphone("iPhone99,9").name, "iPhone");
        assert_eq!(DeviceInfo::iphone("arm64").name, "iPhone");
        assert_eq!(DeviceInfo::iphone("iPad13,4").name, "iPad");
        assert_eq!(DeviceInfo::iphone("iPhone15,2").kind, "iphone");
    }
}

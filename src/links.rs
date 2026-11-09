//! The app's public pages and accounts (Settings → Follow us, About, Help).

// The app's public website: its legal pages.
pub const PRIVACY_URL: &str = "https://reel-studio.expo.app/privacy";
pub const TERMS_URL: &str = "https://reel-studio.expo.app/terms";
pub const DELETE_DATA_URL: &str = "https://reel-studio.expo.app/delete-data";

/// Where bug reports go (the address the website's legal pages give).
pub const SUPPORT_EMAIL: &str = "roth.dev.ops@gmail.com";

/// (name, handle, url)
pub const INSTAGRAM: (&str, &str, &str) = (
    "Instagram",
    "@rs.vdo.editor",
    "https://www.instagram.com/rs.vdo.editor",
);
pub const FACEBOOK: (&str, &str, &str) = (
    "Facebook",
    "RS: Photo & Video Editor",
    "https://www.facebook.com/profile.php?id=61595351946182",
);

/// Percent-encodes a mailto query value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Settings → Report a bug: a mail draft to support with the facts we
/// always need (app version, OS, language) under a short prompt. Nothing
/// identifying: no machine names or ids.
pub fn bug_report_url() -> String {
    use crate::i18n::t;
    let subject = t("Reel Studio bug report");
    let body = [
        t(
            "Tell us what went wrong and how to make it happen again. A screenshot or screen \
             recording helps a lot.",
        )
        .to_string(),
        String::new(),
        String::new(),
        String::new(),
        "———".into(),
        t("Device info (helps us fix it):").to_string(),
        format!("App: {} (desktop)", crate::updater::VERSION),
        format!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH),
        format!("Language: {}", crate::i18n::locale().code()),
    ]
    .join("\n");
    format!(
        "mailto:{SUPPORT_EMAIL}?subject={}&body={}",
        encode(subject),
        encode(&body)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn mailto_is_encoded() {
        let url = super::bug_report_url();
        assert!(url.starts_with("mailto:"));
        assert!(!url.contains(' ') && !url.contains('\n'));
        assert!(url.contains("subject=Reel%20Studio%20bug%20report"));
    }
}

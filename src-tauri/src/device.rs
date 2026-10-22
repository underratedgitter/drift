//! A name people recognise in a device list ("Priya's MacBook Air"), read from
//! macOS. It is a label only, never an identity.

pub fn name() -> String {
    let name = platform_name()
        .map(|name| {
            name.trim()
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback().to_owned());
    // The engine accepts 1–100 bytes; cut on a character boundary.
    let mut end = name.len().min(100);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

fn command(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "macos")]
fn platform_name() -> Option<String> {
    // The name set in System Settings → General → Sharing.
    command("/usr/sbin/scutil", &["--get", "ComputerName"])
}

#[cfg(not(target_os = "macos"))]
fn platform_name() -> Option<String> {
    None
}

fn fallback() -> &'static str {
    "Mac"
}

#[cfg(test)]
mod tests {
    #[test]
    fn name_is_never_empty_and_fits_the_engine_limit() {
        let name = super::name();
        assert!(!name.is_empty() && name.len() <= 100, "{name:?}");
        assert!(!name.contains('\n'));
    }
}

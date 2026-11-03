//! Platform-specific PDF association setup, initiated by the user.

/// What happened, to tell the person.
pub fn offer() -> Result<&'static str, String> {
    imp::offer()
}

/// Whether this system has a way to offer it.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const PROG_ID: &str = "Atelier.PDF";

    pub fn offer() -> Result<&'static str, String> {
        register().map_err(|e| format!("Could not register Atelier: {e}"))?;
        // Opens Settings at Default apps, on Atelier's own page.
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "Start-Process -FilePath 'ms-settings:defaultapps?registeredAppUser=Atelier' -ErrorAction Stop"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()
            .map_err(|e| format!("Could not open Settings: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Could not open Default apps: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(
            "In Atelier's Windows Settings page, select .pdf, choose Atelier, then select Set default. If Windows opens the full Default apps list, search for Atelier first.",
        )
    }

    fn register() -> std::io::Result<()> {
        let exe = std::env::current_exe()?;
        let exe = exe.to_string_lossy();
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let (prog, _) = user.create_subkey(format!(r"Software\Classes\{PROG_ID}"))?;
        prog.set_value("", &"PDF Document")?;
        let (icon, _) = prog.create_subkey("DefaultIcon")?;
        icon.set_value("", &format!("\"{exe}\",0"))?;
        let (open, _) = prog.create_subkey(r"shell\open\command")?;
        open.set_value("", &format!("\"{exe}\" \"%1\""))?;
        let (with, _) = user.create_subkey(r"Software\Classes\.pdf\OpenWithProgids")?;
        with.set_value(PROG_ID, &"")?;
        let (caps, _) = user.create_subkey(r"Software\Atelier\Capabilities")?;
        caps.set_value("ApplicationName", &"Atelier")?;
        caps.set_value(
            "ApplicationDescription",
            &"View, fill in and inspect PDF documents.",
        )?;
        let (assoc, _) = caps.create_subkey("FileAssociations")?;
        assoc.set_value(".pdf", &PROG_ID)?;
        let (apps, _) = user.create_subkey(r"Software\RegisteredApplications")?;
        apps.set_value("Atelier", &r"Software\Atelier\Capabilities")?;
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn offer() -> Result<&'static str, String> {
        Err("Choose the default PDF app in your desktop's settings.".into())
    }
}

#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux {
    use std::path::PathBuf;
    use std::process::Command;

    const DESKTOP_ID: &str = "org.atelier.Viewer.desktop";

    pub fn offer() -> Result<&'static str, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe
            .to_str()
            .ok_or("Atelier's installation path is not UTF-8.")?;
        let exec = desktop_exec(exe)?;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
            .ok_or("Could not locate your applications directory.")?;
        let applications = data.join("applications");
        std::fs::create_dir_all(&applications).map_err(|e| e.to_string())?;
        std::fs::write(applications.join(DESKTOP_ID), format!(
            "[Desktop Entry]\nType=Application\nName=Atelier\nComment=PDF viewer\nExec={exec} %f\nTerminal=false\nMimeType=application/pdf;\nCategories=Office;Viewer;\n"
        )).map_err(|e| format!("Could not register Atelier: {e}"))?;
        // Some desktops use the MIME cache to populate their application chooser.
        let _ = Command::new("update-desktop-database")
            .arg(&applications)
            .output();
        let result = Command::new("xdg-mime")
            .args(["default", DESKTOP_ID, "application/pdf"])
            .output()
            .map_err(|e| format!("Could not set the PDF default: {e}. Install xdg-utils, or choose Atelier in your desktop's Default Applications settings."))?;
        if !result.status.success() {
            return Err(format!(
                "Could not set the PDF default: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        let query = Command::new("xdg-mime")
            .args(["query", "default", "application/pdf"])
            .output()
            .map_err(|e| format!("Could not verify the PDF default: {e}"))?;
        if !query.status.success() || String::from_utf8_lossy(&query.stdout).trim() != DESKTOP_ID {
            return Err("Your desktop did not confirm Atelier as the PDF default. Open Default Applications in your desktop settings and choose Atelier for PDF documents.".into());
        }
        Ok("Atelier is now your default PDF app.")
    }

    fn desktop_exec(exe: &str) -> Result<String, String> {
        if exe.contains(['\n', '\r', '\0']) {
            return Err("Atelier's installation path contains unsupported characters.".into());
        }
        // Escape the Exec argument first, then the desktop-entry string value.
        let mut quoted = String::from("\"");
        for c in exe.chars() {
            match c {
                '%' => quoted.push_str("%%"),
                '\\' | '"' | '`' | '$' => {
                    quoted.push('\\');
                    quoted.push(c);
                }
                _ => quoted.push(c),
            }
        }
        quoted.push('"');
        Ok(quoted.replace('\\', "\\\\"))
    }

    #[cfg(test)]
    mod tests {
        use super::desktop_exec;

        #[test]
        fn quotes_desktop_exec_paths_and_field_codes() {
            assert_eq!(
                desktop_exec("/opt/Atelier Viewer/100%/viewer").unwrap(),
                "\"/opt/Atelier Viewer/100%%/viewer\""
            );
            assert_eq!(
                desktop_exec("/opt/$app/viewer").unwrap(),
                "\"/opt/\\\\$app/viewer\""
            );
            assert_eq!(
                desktop_exec("/opt/\"app\"/viewer").unwrap(),
                "\"/opt/\\\\\"app\\\\\"/viewer\""
            );
        }

        #[test]
        fn rejects_desktop_entry_injection() {
            assert!(desktop_exec("/opt/viewer\nHidden=true").is_err());
            assert!(desktop_exec("/opt/viewer\rHidden=true").is_err());
            assert!(desktop_exec("/opt/viewer\0").is_err());
        }
    }
}

/// Platform-specific instructions; association changes remain the user's choice.
pub fn guidance() -> &'static str {
    if cfg!(target_os = "macos") {
        "Open the PDF default-app controls below. In Get Info, expand Open with, choose Atelier, then click Change All… and Continue. Install Atelier in Applications first so it appears in the list."
    } else if cfg!(windows) {
        "Open Atelier's Default apps page below. Select .pdf, choose Atelier, then select Set default. If the full Default apps list opens, search for Atelier first."
    } else if cfg!(target_os = "linux") {
        "Choose Make Atelier the default PDF app to register this installation and use it for PDF files in your desktop. Keep Atelier installed at its current location."
    } else {
        "In your file manager, right-click a PDF, open Properties → Open With, choose Atelier, and set it as the default. The wording depends on your desktop."
    }
}

/// Opens the PDF's Get Info window at the association controls on macOS.
pub fn reveal_pdf(path: &std::path::Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Finder is available on macOS.".into());
    }
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv\n tell application \"Finder\"\n activate\n open information window of (POSIX file (item 1 of argv) as alias)\n end tell\nend run", "--"])
        .arg(path)
        .output()
        .map_err(|e| format!("Could not open Get Info: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not open Get Info: {}. In Finder, select the PDF and press Command-I.",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

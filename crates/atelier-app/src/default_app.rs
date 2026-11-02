//! Offering Atelier as the default PDF app. Windows lets an app register
//! itself as able to open PDFs, but only the person can make it the
//! default, in Settings; so this registers Atelier for the current user
//! and opens that page.

/// What happened, to tell the person.
pub fn offer() -> Result<&'static str, String> {
    imp::offer()
}

/// Whether this system has a way to offer it.
pub fn supported() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod imp {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const PROG_ID: &str = "Atelier.PDF";

    pub fn offer() -> Result<&'static str, String> {
        register().map_err(|e| format!("Could not register Atelier: {e}"))?;
        // Opens Settings at Default apps, on Atelier's own page.
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:defaultapps?registeredAppUser=Atelier")
            .spawn()
            .map_err(|e| format!("Could not open Settings: {e}"))?;
        Ok("Atelier is registered for PDF files. In Settings, set it as the default for .pdf.")
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

#[cfg(not(windows))]
mod imp {
    pub fn offer() -> Result<&'static str, String> {
        Err("Choose the default PDF app in your desktop's settings.".into())
    }
}

/// Platform-specific instructions; association changes remain the user's choice.
pub fn guidance() -> &'static str {
    if cfg!(target_os = "macos") {
        "In Finder, select a PDF and choose File → Get Info. Under Open with, choose Atelier, then Change All… and Continue. Install Atelier in Applications first so it appears in the list."
    } else if cfg!(windows) {
        "Open Default apps, choose Atelier, and assign it to .pdf files."
    } else {
        "In your file manager, right-click a PDF, open Properties → Open With, choose Atelier, and set it as the default. The wording depends on your desktop."
    }
}

/// Selects a PDF so the person can open its Get Info association controls.
pub fn reveal_pdf(path: &std::path::Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Finder is available on macOS.".into());
    }
    std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not reveal the PDF in Finder: {e}"))
}

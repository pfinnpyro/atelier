//! How much memory Atelier lets itself use for drawn pages and decoded
//! images, worked out from the computer's memory or picked in Settings.

/// The choice in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryUse {
    /// Sized to the computer's memory.
    #[default]
    Auto,
    Low,
    Balanced,
    High,
}

impl MemoryUse {
    pub const ALL: [MemoryUse; 4] = [Self::Auto, Self::Low, Self::Balanced, Self::High];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Balanced => "balanced",
            Self::High => "high",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "low" => Self::Low,
            "balanced" => Self::Balanced,
            "high" => Self::High,
            _ => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Low => "Low",
            Self::Balanced => "Balanced",
            Self::High => "High",
        }
    }
}

/// The limits a choice comes to, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    /// Drawn page tiles of the file in front.
    pub tiles: usize,
    /// Drawn page tiles of each file in a background tab.
    pub parked: usize,
    /// Decoded images kept per open file.
    pub images: usize,
}

const MB: usize = 1 << 20;

impl Budgets {
    pub fn of(mode: MemoryUse) -> Budgets {
        let (tiles, images) = match mode {
            MemoryUse::Low => (128 * MB, 64 * MB),
            MemoryUse::Balanced => (384 * MB, 192 * MB),
            MemoryUse::High => (1024 * MB, 512 * MB),
            MemoryUse::Auto => {
                // A sixteenth of the memory for tiles, a thirty-second for
                // images; 8 GB is assumed when the size cannot be read.
                let ram = host_memory().unwrap_or(8 << 30) as usize;
                (
                    (ram / 16).clamp(128 * MB, 1024 * MB),
                    (ram / 32).clamp(64 * MB, 512 * MB),
                )
            }
        };
        Budgets {
            tiles,
            parked: (tiles / 8).clamp(16 * MB, 64 * MB),
            images,
        }
    }

    /// For Settings: "512 MB for pages, 256 MB for images".
    pub fn describe(&self) -> String {
        format!(
            "{} MB for drawn pages, {} MB for images in each open file",
            self.tiles / MB,
            self.images / MB
        )
    }
}

/// The computer's memory in bytes, when it can be found out without
/// platform calls: `sysctl` on macOS, `/proc/meminfo` on Linux.
pub fn host_memory() -> Option<u64> {
    static FOUND: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        if cfg!(target_os = "macos") {
            let out = std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "hw.memsize"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        } else if cfg!(target_os = "linux") {
            let text = std::fs::read_to_string("/proc/meminfo").ok()?;
            let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kb * 1024)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_and_stay_in_bounds() {
        for mode in MemoryUse::ALL {
            assert_eq!(MemoryUse::parse(mode.key()), mode);
            let b = Budgets::of(mode);
            assert!(b.tiles >= 128 * MB && b.tiles <= 1024 * MB);
            assert!(b.parked <= b.tiles && b.images >= 64 * MB);
        }
    }
}

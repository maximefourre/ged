//! Free/total space of the filesystem holding a path, via POSIX `df -Pk`
//! (portable columns, 1024-byte blocks): port of `app/services/disk_space.rb`.
//! Any failure is `None`.

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    pub total_kb: u64,
    pub available_kb: u64,
}

impl DiskSpace {
    /// Share of the disk in use, 0.0 to 1.0.
    pub fn used_fraction(&self) -> f64 {
        if self.total_kb == 0 {
            0.0
        } else {
            1.0 - (self.available_kb as f64 / self.total_kb as f64)
        }
    }

    /// Reads `df -Pk` output: a header line, then
    /// `filesystem total used available capacity mount`.
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().nth(1)?;
        let mut cols = line.split_whitespace();
        let _filesystem = cols.next()?;
        let total = cols.next()?;
        let _used = cols.next()?;
        let available = cols.next()?;
        // `String#to_i`: a non-number reads as 0.
        let kb = |s: &str| s.parse::<u64>().unwrap_or(0);
        Some(Self {
            total_kb: kb(total),
            available_kb: kb(available),
        })
    }

    /// Blocking: runs `df`.
    pub fn for_path(path: &Path) -> Option<Self> {
        let out = Command::new("df")
            .arg("-Pk")
            .arg(path)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Self::parse(&String::from_utf8_lossy(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DF: &str = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                      /dev/disk3s1s1   971350180  11190560 459431196     3%   /\n";

    #[test]
    fn parses_df_output() {
        let d = DiskSpace::parse(DF).unwrap();
        assert_eq!((d.total_kb, d.available_kb), (971_350_180, 459_431_196));
        assert!((d.used_fraction() - 0.527).abs() < 0.001);
    }

    #[test]
    fn a_header_only_or_empty_output_is_none() {
        assert_eq!(DiskSpace::parse("Filesystem 1024-blocks\n"), None);
        assert_eq!(DiskSpace::parse(""), None);
    }

    #[test]
    fn an_empty_disk_reads_as_unused() {
        let d = DiskSpace {
            total_kb: 0,
            available_kb: 0,
        };
        assert_eq!(d.used_fraction(), 0.0);
    }

    #[test]
    fn the_current_directory_has_a_disk() {
        let d = DiskSpace::for_path(Path::new(".")).expect("df works here");
        assert!(d.total_kb > 0);
    }

    #[test]
    fn a_missing_path_is_none() {
        assert_eq!(DiskSpace::for_path(Path::new("/definitely/not/here")), None);
    }
}

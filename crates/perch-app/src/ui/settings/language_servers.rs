//! Language servers: how much memory they may use between them.

use perch_github::Setting;

use super::controls::dropdown;
use super::{Group, Page, Row};

/// The limits offered, in gigabytes.
const LIMITS_GB: [u64; 5] = [2, 4, 8, 16, 32];

pub(super) fn groups(page: &Page) -> Vec<Group> {
    vec![Group::new("Memory", vec![memory(page)])]
}

/// `mb` as a person would say it: "8 GB", or "1500 MB" when it isn't a
/// whole number of gigabytes.
pub(super) fn memory_label(mb: u64) -> String {
    if mb >= 1024 && mb.is_multiple_of(1024) {
        format!("{} GB", mb / 1024)
    } else {
        format!("{mb} MB")
    }
}

fn memory(page: &Page) -> Row {
    let control = {
        let page = page.clone();
        move || {
            dropdown(
                &page,
                |page| {
                    let current = page.with_config(|c| c.lsp.max_memory_mb);
                    let mut offered: Vec<u64> = LIMITS_GB.iter().map(|gb| gb * 1024).collect();
                    if !offered.contains(&current) {
                        offered.push(current);
                        offered.sort_unstable();
                    }
                    let offered = offered
                        .into_iter()
                        .map(|mb| (memory_label(mb), mb))
                        .collect();
                    (offered, current)
                },
                |mb| Setting::LspMaxMemoryMb(Some(mb)),
                "settings-lsp-memory",
            )
        }
    };
    Row::new(
        "lsp-memory",
        "Memory limit",
        || {
            "What the language servers may use between them. Past it, the ones kept for \
             items you have left are stopped, oldest first."
                .to_string()
        },
        control,
    )
    .keywords(&[
        "lsp",
        "rust-analyzer",
        "jdtls",
        "ram",
        "memory",
        "lsp.max_memory_mb",
    ])
    .reset("lsp.max_memory_mb", Setting::LspMaxMemoryMb(None))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{goes_dark, open_on, pick, shown, window, written};
    use super::super::Section;
    use super::*;

    #[test]
    fn a_limit_reads_in_gigabytes_when_it_can() {
        assert_eq!(memory_label(8192), "8 GB");
        assert_eq!(memory_label(1024), "1 GB");
        assert_eq!(memory_label(1500), "1500 MB");
        assert_eq!(memory_label(512), "512 MB");
    }

    #[test]
    fn the_memory_limit_offers_the_files_own_and_writes_what_is_picked() {
        let (mut harness, state, mut commands) =
            window(1400.0, 900.0, "[lsp]\nmax_memory_mb = 3000\n");
        open_on(&mut harness, state, Section::LanguageServers);
        assert!(shown(&harness, "settings-row:lsp-memory"));
        assert!(shown(&harness, "settings-reset:lsp-memory"));
        pick(&mut harness, "settings-lsp-memory", "16 GB");
        assert_eq!(
            written(&mut commands),
            vec![Setting::LspMaxMemoryMb(Some(16 * 1024))]
        );
        pick(&mut harness, "settings-lsp-memory", "3000 MB");
        assert_eq!(
            written(&mut commands),
            vec![Setting::LspMaxMemoryMb(Some(3000))]
        );
        goes_dark(&mut harness);
    }
}

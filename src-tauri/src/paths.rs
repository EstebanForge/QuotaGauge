use std::path::PathBuf;

/// Home directory of the current user. `None` means unknown; callers must fail
/// closed (skip credential discovery) instead of resolving vendor paths against
/// the process working directory.
pub(crate) fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

//! Where the library lives on this computer when nobody says otherwise.
//!
//! The library is a database that is open while the app runs, so it must sit in a folder that nothing else copies
//! or syncs behind its back: a sync client that uploads a database's files mid-write can leave them inconsistent.
//! Every system has a place for exactly this: data that belongs to one program on one computer.
//!
//! | system  | folder                                                    |
//! |---------|-----------------------------------------------------------|
//! | Windows | `%LOCALAPPDATA%\Constellate` (not Roaming, which may sync) |
//! | macOS   | `~/Library/Application Support/Constellate`               |
//! | Linux   | `$XDG_DATA_HOME/constellate`, or `~/.local/share/constellate` |
//!
//! It is never the folder the program happened to be started from. Someone who wants it elsewhere sets
//! `CONSTELLATE_DATA`.

use std::path::PathBuf;

/// The system the folder is worked out for: one of the three layouts above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Windows,
    MacOs,
    /// Linux and anything else that follows the XDG layout.
    Xdg,
}

impl System {
    /// The system this program was built for.
    pub const fn this() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Xdg
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot tell where to keep the library: {missing} is not set. Set CONSTELLATE_DATA to the folder to use.")]
pub struct NoPlace {
    /// The environment variable the system's layout needs.
    pub missing: &'static str,
}

/// The default data folder for `system`, reading the environment through `variable` (so tests pass their own).
/// An empty or relative value counts as not set: a relative folder would depend on where the program was started.
pub fn default_folder(system: System, variable: impl Fn(&str) -> Option<String>) -> Result<PathBuf, NoPlace> {
    let folder = |name: &'static str| {
        variable(name).map(PathBuf::from).filter(|path| path.is_absolute()).ok_or(NoPlace { missing: name })
    };
    match system {
        System::Windows => Ok(folder("LOCALAPPDATA")?.join("Constellate")),
        System::MacOs => Ok(folder("HOME")?.join("Library").join("Application Support").join("Constellate")),
        System::Xdg => match folder("XDG_DATA_HOME") {
            Ok(shared) => Ok(shared.join("constellate")),
            Err(_) => Ok(folder("HOME")?.join(".local").join("share").join("constellate")),
        },
    }
}

/// The data folder: the one asked for, or this system's default.
pub fn data_folder(asked: Option<String>) -> Result<PathBuf, NoPlace> {
    match asked.filter(|folder| !folder.trim().is_empty()) {
        Some(folder) => Ok(PathBuf::from(folder)),
        None => default_folder(System::this(), |name| std::env::var(name).ok()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// An environment holding exactly these variables.
    fn with(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
    }

    /// An absolute folder on the system the tests run on.
    fn root(rest: &str) -> String {
        if cfg!(windows) { format!("C:\\{rest}") } else { format!("/{rest}") }
    }

    #[test]
    fn each_system_keeps_the_library_in_its_own_place_for_local_data() {
        let local: &'static str = Box::leak(root("local").into_boxed_str());
        let home: &'static str = Box::leak(root("home").into_boxed_str());
        let shared: &'static str = Box::leak(root("shared").into_boxed_str());

        let windows = default_folder(System::Windows, with(Box::leak(Box::new([("LOCALAPPDATA", local)])))).unwrap();
        assert_eq!(windows, PathBuf::from(local).join("Constellate"));

        let mac = default_folder(System::MacOs, with(Box::leak(Box::new([("HOME", home)])))).unwrap();
        assert_eq!(mac, PathBuf::from(home).join("Library").join("Application Support").join("Constellate"));

        let linux = default_folder(System::Xdg, with(Box::leak(Box::new([("HOME", home)])))).unwrap();
        assert_eq!(linux, PathBuf::from(home).join(".local").join("share").join("constellate"));

        let both = default_folder(System::Xdg, with(Box::leak(Box::new([("HOME", home), ("XDG_DATA_HOME", shared)]))));
        assert_eq!(both.unwrap(), PathBuf::from(shared).join("constellate"));
    }

    #[test]
    fn windows_uses_the_local_folder_never_the_roaming_one() {
        let roaming: &'static str = Box::leak(root("roaming").into_boxed_str());
        let missing = default_folder(System::Windows, with(Box::leak(Box::new([("APPDATA", roaming)]))));
        assert_eq!(missing, Err(NoPlace { missing: "LOCALAPPDATA" }));
    }

    #[test]
    fn an_unset_empty_or_relative_variable_is_an_error_that_says_what_to_do() {
        for value in ["", "relative/folder", "."] {
            let found = default_folder(System::Windows, move |_| Some(value.to_owned()));
            assert_eq!(found, Err(NoPlace { missing: "LOCALAPPDATA" }), "{value:?}");
        }
        let error = default_folder(System::MacOs, |_| None).unwrap_err();
        assert!(error.to_string().contains("HOME is not set"));
        assert!(error.to_string().contains("CONSTELLATE_DATA"));
        // a relative XDG_DATA_HOME is ignored, as the XDG rules say, and the home folder is used
        let home: &'static str = Box::leak(root("home").into_boxed_str());
        let found = default_folder(System::Xdg, with(Box::leak(Box::new([("XDG_DATA_HOME", "data"), ("HOME", home)]))));
        assert_eq!(found.unwrap(), PathBuf::from(home).join(".local").join("share").join("constellate"));
    }

    #[test]
    fn a_folder_that_was_asked_for_is_used_as_given() {
        assert_eq!(data_folder(Some(root("my-library"))).unwrap(), PathBuf::from(root("my-library")));
        // nothing asked for (or only spaces): this system's default, which is never the working folder
        let chosen = data_folder(Some("  ".to_owned())).unwrap();
        assert!(chosen.is_absolute(), "{chosen:?}");
        assert_eq!(chosen, data_folder(None).unwrap());
    }
}

use std::path::PathBuf;

pub mod version;

/// Gets the location to the launcher manifest.
///
/// - Windows: `%APPDATA%/.minecraft/versions/version_manifest_v2.json`
/// - Mac:  `~/Library/Application Support/minecraft/versions/version_manifest_v2.json`
/// - Linux:   `~/.minecraft/versions/version_manifest_v2.json`
///
/// returns: `PathBuf`
fn get_manifest_location() -> PathBuf {
    const MANIFEST_NAME: &str = "version_manifest_v2.json";
    #[cfg(windows)]
    let manifest_parent = {
        let appdata = env!("APPDATA");
        let mut appdata_path = PathBuf::from(appdata);
        appdata_path.push(".minecraft");
        appdata_path
    };

    #[cfg(target_os = "macos")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push("Library");
        home_path.push("Application Support");
        home_path.push("minecraft");
        home_path
    };

    #[cfg(target_os = "linux")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push(".minecraft");
        home_path
    };

    manifest_parent.join("versions").join(MANIFEST_NAME)
}

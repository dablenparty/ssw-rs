use std::path::PathBuf;

const MANIFEST_V2_LINK: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

/// Gets the location to the launcher manifest.
///
/// - Windows: `%APPDATA%/.minecraft/versions/version_manifest_v2.json`
/// - macOSX:  `~/Library/Application Support/minecraft/versions/version_manifest_v2.json`
/// - Linux:   `~/.minecraft/versions/version_manifest_v2.json`
fn get_manifest_location() -> PathBuf {
    const MANIFEST_NAME: &str = "version_manifest_v2.json";
    #[cfg(windows)]
    let manifest_parent = {
        let appdata = env!("APPDATA");
        let mut appdata_path = PathBuf::from(appdata);
        appdata_path.push(".minecraft");
        appdata_path.push("versions");
        appdata_path
    };

    // TODO: mac and linux
    // mac: ~/Library/Application Support/minecraft/versions
    // linux: ~/.minecraft/versions

    manifest_parent.join(MANIFEST_NAME)
}

use settings::macros::define_settings_group;
use settings::{SupportedPlatforms, SyncToCloud};

pub const DEFAULT_REMOTE_ACCESS_SERVER_ADDRESS: &str = "wss://hako.egehakan.com";

// Neither setting is synced: remote access is a decision about one computer, and the access key
// it relies on lives on that computer only.
define_settings_group!(RemoteAccessSettings, settings: [
    enabled: RemoteAccessEnabled {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "remote_access.enabled",
        description: "Whether the app connects to the remote access server, so that its windows, tabs and terminals can be seen and controlled from a browser.",
    },
    server_address: RemoteAccessServerAddress {
        type: String,
        default: DEFAULT_REMOTE_ACCESS_SERVER_ADDRESS.to_string(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "remote_access.server_address",
        description: "The address of the remote access server, starting with wss://. ws:// is accepted for a server on this computer.",
    },
]);

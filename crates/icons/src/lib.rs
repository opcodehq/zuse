//! One embedded Hugeicons selection for native and browser clients.
//! Free assets are bundled; licensed solid assets stay in Cargo's build output.
include!(concat!(env!("OUT_DIR"), "/icons.rs"));

mod files;
pub use files::{file_icon_path, load_file_icon};

mod plugins;
pub use plugins::load_plugin_icon;

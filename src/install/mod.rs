#![cfg(not(target_os = "windows"))]

//! Non-Windows desktop integration installers. Today that is only the GNOME
//! Shell helper extension, which the Linux injection path and recording pill
//! reach over D-Bus.

pub mod gnome_extension;

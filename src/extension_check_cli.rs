//! Read-only manifest checking, before application/credential/GPU startup.
use std::{ffi::OsString, path::PathBuf};
use waifu_studio::extension_manifest::check_pack;

pub fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let root = args
        .next()
        .ok_or("Usage: waifu-studio --check-extension FOLDER")?;
    if root.is_empty() || args.next().is_some() {
        return Err("Usage: waifu-studio --check-extension FOLDER (no other options)".into());
    }
    let root = PathBuf::from(root);
    let pack = check_pack(&root)?;
    let executable = pack.manifest.process.is_some() || !pack.manifest.hooks.is_empty();
    // Fixed labels and counts: do not echo arbitrary manifest text or credentials.
    println!(
        "Valid {} manifest (API {}): {} content entries, {} hooks, {} settings, {} credential fields.",
        if executable {
            "integration"
        } else {
            "content-pack"
        },
        pack.manifest.api_version,
        pack.content.len(),
        pack.manifest.hooks.len(),
        pack.manifest.settings.len(),
        pack.manifest.credentials.len()
    );
    println!(
        "Checked declared paths and schema. No code ran and nothing was installed. Installation and runtime checks still apply."
    );
    Ok(())
}

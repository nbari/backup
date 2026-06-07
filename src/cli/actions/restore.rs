use crate::cli::{actions::Action, globals::GlobalArgs};
use anyhow::Result;

/// Handle the restore action.
///
/// # Errors
/// Currently never errors — restore is not implemented yet.
pub fn handle(action: Action, _globals: &GlobalArgs) -> Result<()> {
    if let Action::Restore {
        name,
        target,
        version,
        into,
    } = action
    {
        // TODO: implement restore. Once content storage exists, this should:
        //   1. resolve the target (file id / path / whole snapshot) at `version`,
        //   2. unwrap each file key with the recovery mnemonic,
        //   3. fetch the blob, decrypt (ChaCha20-Poly1305) and decompress it,
        //   4. write it to `into` (or its original path), verifying the hash.
        // Blocked on the data plane (compression/encryption/upload) which is
        // not built yet.
        let scope = target.unwrap_or_else(|| "everything".to_string());
        let version = version.map_or_else(|| "latest".to_string(), |v| v.to_string());
        let destination =
            into.map_or_else(|| "original paths".to_string(), |p| p.display().to_string());

        println!("`restore` is not implemented yet.");
        println!(
            "Planned: restore {scope} from backup \"{name}\" (version {version}) into {destination}."
        );
    }

    Ok(())
}

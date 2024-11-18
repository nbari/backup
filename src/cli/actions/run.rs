use crate::cli::actions::Action;
use anyhow::Result;

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    if let Action::Run {
        name,
        no_gitignore,
        no_compression,
        no_encryption,
        dry_run,
    } = action
    {
        println!("Running backup: {}", name);
        println!("Ignore .gitignore: {}", no_gitignore);
        println!("No compression: {}", no_compression);
        println!("No encryption: {}", no_encryption);
        println!("Dry run: {}", dry_run);
        todo!()
    }

    Ok(())
}

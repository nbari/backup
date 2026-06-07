use crate::{
    cli::actions::Action,
    engine::create::{CreateBackupRequest, create},
};
use anyhow::Result;

/// Handle the create action.
///
/// # Errors
/// Returns an error if the backup database cannot be created or initialized.
pub fn handle(action: Action) -> Result<()> {
    if let Action::New {
        name,
        config,
        directory,
        file,
    } = action
    {
        let result = create(CreateBackupRequest {
            name,
            config_dir: config,
            directories: directory.unwrap_or_default(),
            files: file.unwrap_or_default(),
        })?;

        print_recovery_phrase(&result.recovery_phrase);
    }

    Ok(())
}

fn print_recovery_phrase(recovery_phrase: &str) {
    let words: Vec<&str> = recovery_phrase.split_whitespace().collect();

    println!("Your recovery phrase is:\n");
    println!("[ {recovery_phrase} ]\n");

    for (i, word) in words.iter().enumerate() {
        print!("{:2}. {:12}", i + 1, word);
        if (i + 1) % 4 == 0 {
            println!();
        }
    }

    println!("\n\nPlease write this down and store it in a safe place.");
}

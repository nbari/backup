use crate::cli::actions::Action;
use anyhow::Result;

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    match action {
        Action::New {
            name,
            directory: _,
            file: _,
            exclude: _,
        } => {
            println!("Creating new project: {}", name);
        }
    }

    Ok(())
}

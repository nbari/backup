use crate::cli::actions::Action;
use anyhow::Result;

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    match action {
        Action::New {
            name,
            directory,
            file,
            exclude,
        } => {
            println!("Creating new project: {}", name);
        }

        _ => unreachable!(),
    }

    Ok(())
}

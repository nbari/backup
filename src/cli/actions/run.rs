use crate::cli::actions::Action;
use anyhow::Result;

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    if let Action::Run { name } = action {
        println!("Running {}", name);
    }

    Ok(())
}

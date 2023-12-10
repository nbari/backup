use anyhow::Result;
use backup::cli::{actions, actions::Action, start};
use std::process;

// Main function
fn main() -> Result<()> {
    // Start the program
    let action = start()?;

    // Handle the action
    match action {
        Action::New { .. } => actions::new::handle(action)?,
        _ => todo!(),
    }

    Ok(())
}

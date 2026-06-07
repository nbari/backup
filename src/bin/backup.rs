use anyhow::Result;
use backup::cli::{actions, actions::Action, start};

// Main function
#[tokio::main]
async fn main() -> Result<()> {
    // Start the program
    let (action, globals) = start()?;

    // Handle the action
    match action {
        Action::New { .. } => actions::new::handle(action)?,
        Action::Show => actions::show::handle(globals)?,
        Action::Run { .. } => actions::run::handle(action, globals).await?,
    }

    Ok(())
}

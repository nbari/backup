use crate::cli::actions::Action;
use anyhow::Result;

pub const fn dispatch() -> Result<Action> {
    Ok(Action::Show {})
}

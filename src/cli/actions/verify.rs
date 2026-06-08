use crate::{
    cli::{actions::Action, actions::run::resolve_naming_key, globals::GlobalArgs},
    engine::verify::{VerifyReport, verify},
};
use anyhow::Result;

/// Handle the verify action.
///
/// # Errors
/// Returns an error if the backup is missing or a destination/catalog op fails.
pub async fn handle(action: Action, globals: &GlobalArgs) -> Result<()> {
    if let Action::Verify { name, repair } = action {
        // Re-sealing missing-everywhere blobs reads the source files and needs the
        // naming key to confirm they still match; resolving it may prompt for the
        // mnemonic. An existence-only check needs no secret.
        let naming_key = if repair {
            Some(resolve_naming_key(&globals.home, &name)?)
        } else {
            None
        };

        let report = verify(&globals.home, &name, repair, naming_key).await?;

        if !globals.quiet {
            print_report(&name, repair, &report);
        }
    }

    Ok(())
}

fn print_report(name: &str, repair: bool, report: &VerifyReport) {
    println!(
        "Verified {} content object(s) across {} destination(s) for \"{name}\".",
        report.content_ids, report.destinations
    );

    if report.missing == 0 {
        println!("All blobs present. Nothing to repair.");
        return;
    }

    println!("Missing blob copies: {}.", report.missing);

    if !repair {
        println!("Run `backup verify {name} --repair` to restore them.");
        return;
    }

    if report.repaired_by_copy > 0 {
        println!(
            "Repaired {} copy(ies) from a healthy destination.",
            report.repaired_by_copy
        );
    }
    if report.repaired_by_reseal > 0 {
        println!(
            "Re-sealed {} object(s) from source files.",
            report.repaired_by_reseal
        );
    }
    if report.unrecoverable.is_empty() {
        println!("All missing blobs repaired.");
    } else {
        println!(
            "{} object(s) could NOT be recovered (gone from every destination and the \
             source files are missing or changed):",
            report.unrecoverable.len()
        );
        for id in &report.unrecoverable {
            println!("  {id}");
        }
    }
}

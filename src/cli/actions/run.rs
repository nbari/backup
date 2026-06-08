use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    db::sqlite::SqliteCatalog,
    engine::{
        run::{
            IgnoreRules, NamingKey, ProgressCallback, RunBackupRequest, RunProgress, run,
            scan_worker_count,
        },
        wkey,
    },
    utils::{crypto::unseal_naming_key, format::format_duration},
};
use anyhow::{Result, anyhow};
use bip39::{Language, Mnemonic};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::{path::Path, sync::Arc, time::Duration};
use tracing::instrument;
use zeroize::Zeroizing;

struct RunProgressRenderer {
    multi: MultiProgress,
    spinner: ProgressBar,
    workers: Arc<Vec<ProgressBar>>,
}

impl RunProgressRenderer {
    fn new() -> Result<Self> {
        let multi = MultiProgress::new();
        let spinner = multi.add(ProgressBar::new(0));
        let worker_style = ProgressStyle::with_template("{spinner:.green} worker {prefix}: {msg}")?;

        spinner.set_style(ProgressStyle::with_template(
            "{spinner:.green} {pos}/{len} {msg}",
        )?);
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message("Discovering files");

        let workers = (1..=scan_worker_count())
            .map(|worker_id| {
                let worker = multi.add(ProgressBar::new_spinner());
                worker.set_style(worker_style.clone());
                worker.set_prefix(format!("{worker_id:02}"));
                worker.set_message("idle");
                worker.enable_steady_tick(Duration::from_millis(100));
                worker
            })
            .collect::<Vec<_>>();

        Ok(Self {
            multi,
            spinner,
            workers: Arc::new(workers),
        })
    }

    fn callback(&self) -> ProgressCallback {
        let spinner = self.spinner.clone();
        let workers = self.workers.clone();

        Arc::new(move |progress| match progress {
            RunProgress::FilesDiscovered(total_files) => match u64::try_from(total_files) {
                Ok(total_files) => {
                    spinner.set_length(total_files);
                    spinner.set_position(0);
                    if total_files == 0 {
                        spinner.set_message("No files to scan");
                    } else {
                        spinner.set_message("Scanning files");
                    }
                }
                Err(err) => {
                    spinner.set_message(format!("Unable to display progress total: {err}"));
                }
            },
            RunProgress::FileFinished => {
                spinner.inc(1);
            }
            RunProgress::MetadataFilesWritten(written_files) => {
                match u64::try_from(written_files) {
                    Ok(written_files) => {
                        spinner.set_position(written_files);
                    }
                    Err(err) => {
                        spinner.set_message(format!("Unable to display metadata progress: {err}"));
                    }
                }
            }
            RunProgress::StorePhaseStarted(total_blobs) => match u64::try_from(total_blobs) {
                Ok(total_blobs) => {
                    spinner.set_length(total_blobs);
                    spinner.set_position(0);
                    if total_blobs == 0 {
                        spinner.set_message("No new data to store");
                    } else {
                        spinner.set_message("Compressing & encrypting");
                    }
                }
                Err(err) => {
                    spinner.set_message(format!("Unable to display store total: {err}"));
                }
            },
            RunProgress::MetadataWriteStarted(total_files) => match u64::try_from(total_files) {
                Ok(total_files) => {
                    spinner.set_length(total_files);
                    spinner.set_position(0);
                    spinner.set_message("Writing metadata to SQLite");
                    for worker in &*workers {
                        worker.finish_and_clear();
                    }
                }
                Err(err) => {
                    spinner.set_message(format!("Unable to display metadata total: {err}"));
                }
            },
            RunProgress::ProcessingFile { worker_id, path } => {
                if let Some(index) = worker_id.checked_sub(1)
                    && let Some(worker) = workers.get(index)
                {
                    worker.set_message(path.display().to_string());
                }
            }
            RunProgress::WorkerFinished(worker_id) => {
                if let Some(index) = worker_id.checked_sub(1)
                    && let Some(worker) = workers.get(index)
                {
                    worker.set_message("idle");
                }
            }
        })
    }

    fn finish(&self, scanned_files: usize, skipped_entries: usize, skipped_files_log: &Path) {
        self.spinner.finish_and_clear();
        for worker in &*self.workers {
            worker.finish_and_clear();
        }

        if let Err(err) = self.multi.clear() {
            tracing::debug!("Failed to clear progress output: {err}");
        }

        println!("Scanned {scanned_files} files.");
        if skipped_entries > 0 {
            println!(
                "Skipped {skipped_entries} entries. See log: {}",
                skipped_files_log.display()
            );
        }
    }
}

fn progress_renderer(quiet: bool) -> Result<Option<RunProgressRenderer>> {
    if quiet {
        return Ok(None);
    }

    Ok(Some(RunProgressRenderer::new()?))
}

/// Resolve the per-backup naming key, prompting for the mnemonic if the cache is
/// absent.
///
/// On the fast path the `{name}.wkey` cache is read directly, so `cron` runs
/// never prompt. If the cache is missing, the user is asked for the recovery
/// mnemonic, the naming key is unsealed from the catalog, and the cache is
/// rewritten. Deleting `{name}.wkey` therefore both forces this prompt and acts
/// as a self-test that the mnemonic can unlock the backup.
pub(crate) fn resolve_naming_key(config_dir: &Path, name: &str) -> Result<NamingKey> {
    if let Some(naming_key) = wkey::load_naming_key(config_dir, name)? {
        return Ok(Arc::new(naming_key));
    }

    let db_file = config_dir.join(format!("{name}.db"));
    if !db_file.exists() {
        return Err(anyhow!(
            "No backup named \"{name}\" found. Create a new backup first."
        ));
    }

    let sealed = SqliteCatalog::open(&db_file)?.sealed_naming_key()?;

    let phrase = Zeroizing::new(rpassword::prompt_password(format!(
        "Enter the recovery mnemonic for \"{name}\" to unlock: "
    ))?);
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase.trim())
        .map_err(|_| anyhow!("Invalid recovery mnemonic"))?;

    let naming_key = unseal_naming_key(&sealed, &mnemonic)
        .map_err(|_| anyhow!("Incorrect mnemonic: could not unlock backup \"{name}\""))?;

    wkey::write_naming_key(config_dir, name, &naming_key)?;

    Ok(Arc::new(naming_key))
}

/// Handle the run action.
///
/// # Errors
/// Returns an error if the configured backup cannot be scanned or metadata cannot be updated.
#[instrument(skip(action, globals))]
pub async fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    let timer = globals.timer.start();

    if let Action::Run {
        name,
        gitignore,
        no_ignore,
        dry_run,
    } = action
    {
        let ignore_rules = if no_ignore {
            IgnoreRules::none()
        } else {
            IgnoreRules {
                backupignore: true,
                gitignore,
            }
        };

        // Resolve the naming key before rendering progress, since unlocking may
        // prompt for the mnemonic interactively.
        let naming_key = resolve_naming_key(&globals.home, &name)?;
        let backup_name = name.clone();

        let progress = progress_renderer(globals.quiet)?;
        let progress_callback = progress.as_ref().map(RunProgressRenderer::callback);

        let result = run(RunBackupRequest {
            name,
            config_dir: globals.home,
            ignore_rules,
            dry_run,
            progress: progress_callback,
            naming_key,
        })
        .await?;

        if let Some(progress) = progress {
            progress.finish(
                result.scanned_files,
                result.skipped_entries,
                &result.skipped_files_log,
            );
        } else if !globals.quiet && result.skipped_entries > 0 {
            println!(
                "Skipped {} entries. See log: {}",
                result.skipped_entries,
                result.skipped_files_log.display()
            );
        }

        if !globals.quiet {
            if !dry_run {
                if result.destination_count == 0 {
                    println!(
                        "No destinations configured — recorded metadata only (no data stored). Add one with `backup edit {backup_name} --to <path>`."
                    );
                } else {
                    println!(
                        "Stored {} new object(s) to {} destination(s).",
                        result.stored_blobs, result.destination_count
                    );
                }
            }

            println!(
                "Backup{} version: {}\n",
                if dry_run { " (dry-run)" } else { "" },
                result.version
            );

            println!(
                "Backup{} completed successfully in: {}.",
                if dry_run { " (dry-run)" } else { "" },
                format_duration(timer.elapsed())
            );
        }
    }

    Ok(())
}

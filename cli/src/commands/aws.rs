//! Native AWS source management uses the same domain helpers as the GUI.

use clap::Subcommand;
use rssh_core::aws::auth::AwsAuth;
use rssh_core::commands::discovery::{
    read_dynamic_discovery_sources_from_db, update_dynamic_discovery_sources_secure,
};
use rssh_core::error::{AppError, AppResult};
use rssh_core::models::{DynamicDiscoveryConfig, DynamicDiscoverySource};
use rssh_core::state::SessionOwner;
use serde::Serialize;
use serde_json::json;

use crate::ctx::CliCtx;

#[derive(Subcommand)]
pub enum AwsCmd {
    /// List native AWS sources as JSON (never includes credentials)
    List,
    /// Add a native AWS discovery source; Region is explicitly supplied
    Add {
        name: String,
        #[arg(long)]
        region: String,
        #[arg(long)]
        only_ssm_available: bool,
    },
    /// Update a native AWS source by its ID
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        region: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        #[arg(long, action = clap::ArgAction::Set)]
        only_ssm_available: Option<bool>,
    },
    /// Remove a native AWS source and its encrypted login credentials
    Remove { id: String },
    /// Sign in through AWS's web page, then paste the authorization code
    Login { id: String },
    /// Remove this source's locally stored login credentials
    Logout { id: String },
    /// Show non-secret login status
    Status { id: String },
    /// Query EC2 in this source's configured Region and print JSON
    Discover { id: String },
}

fn print_json(value: &impl Serialize) -> AppResult<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|e| { AppError::other("serde_failed", json!({"err": e.to_string()})) })?
    );
    Ok(())
}

fn find_source<'a>(
    sources: &'a [DynamicDiscoverySource],
    id: &str,
) -> AppResult<&'a DynamicDiscoverySource> {
    sources
        .iter()
        .find(|s| s.id == id && matches!(s.config, DynamicDiscoveryConfig::AwsNative { .. }))
        .ok_or_else(|| AppError::not_found("aws_source_not_found", json!({})))
}

pub fn run(conn: &CliCtx, action: AwsCmd) -> AppResult<()> {
    match action {
        AwsCmd::List => {
            let sources: Vec<_> = read_dynamic_discovery_sources_from_db(&conn.db)?
                .into_iter()
                .filter(|s| matches!(s.config, DynamicDiscoveryConfig::AwsNative { .. }))
                .collect();
            return print_json(&sources);
        }
        AwsCmd::Add {
            name,
            region,
            only_ssm_available,
        } => {
            let source = DynamicDiscoverySource {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                enabled: true,
                config: DynamicDiscoveryConfig::AwsNative {
                    region,
                    only_ssm_available,
                },
            };
            update_dynamic_discovery_sources_secure(
                &conn.db,
                conn.secret_store().as_ref(),
                &conn.data_dir,
                |sources| {
                    sources.push(source.clone());
                    Ok(())
                },
            )?;
            return print_json(&source);
        }
        AwsCmd::Update {
            id,
            name,
            region,
            enabled,
            only_ssm_available,
        } => {
            let output = update_dynamic_discovery_sources_secure(
                &conn.db,
                conn.secret_store().as_ref(),
                &conn.data_dir,
                |sources| {
                    find_source(sources, &id)?;
                    let source = sources
                        .iter_mut()
                        .find(|s| s.id == id)
                        .expect("validated source");
                    if let Some(name) = name {
                        source.name = name;
                    }
                    if let Some(enabled) = enabled {
                        source.enabled = enabled;
                    }
                    if let DynamicDiscoveryConfig::AwsNative {
                        region: current_region,
                        only_ssm_available: current_filter,
                    } = &mut source.config
                    {
                        if let Some(region) = region {
                            *current_region = region;
                        }
                        if let Some(filter) = only_ssm_available {
                            *current_filter = filter;
                        }
                    }
                    Ok(source.clone())
                },
            )?;
            return print_json(&output);
        }
        AwsCmd::Remove { id } => {
            update_dynamic_discovery_sources_secure(
                &conn.db,
                conn.secret_store().as_ref(),
                &conn.data_dir,
                |sources| {
                    find_source(sources, &id)?;
                    sources.retain(|s| s.id != id);
                    Ok(())
                },
            )?;
            return Ok(());
        }
        _ => {}
    }

    let sources = read_dynamic_discovery_sources_from_db(&conn.db)?;
    let auth = AwsAuth::new(conn.secret_store().clone(), conn.data_dir.clone())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
    runtime.block_on(async {
        match action {
            AwsCmd::Login { id } => {
                let source = find_source(&sources, &id)?;
                let DynamicDiscoveryConfig::AwsNative { region, .. } = &source.config else {
                    unreachable!()
                };
                let login = auth.start(&id, region, &owner).await?;
                eprintln!(
                    "Open this AWS page in your browser, sign in and authorize:\n{}",
                    login.authorization_url
                );
                let code = tokio::task::spawn_blocking(|| {
                    rpassword::prompt_password("Paste the authorization code (hidden): ")
                })
                .await
                .map_err(|e| AppError::other("task_join_failed", json!({"err": e.to_string()})))?;
                let result = match code {
                    Ok(code) => {
                        match auth
                            .complete(&id, &login.login_id, code.trim(), &owner)
                            .await
                        {
                            Ok(status) => {
                                // CLI login is an explicit save action. Recheck
                                // the source under the shared auth lock before
                                // persisting the verified draft.
                                let db = conn.db.clone();
                                let source_id = id.clone();
                                auth.commit_with(&id, &login.login_id, &owner, move || {
                                    let sources = read_dynamic_discovery_sources_from_db(&db)?;
                                    find_source(&sources, &source_id)?;
                                    Ok(status)
                                })
                                .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Err(e) => Err(e.into()),
                };
                if result.is_err() {
                    // Retain the original failure, but never silently hide a cleanup failure.
                    if let Err(error) = auth.cancel(&id, &login.login_id, &owner).await {
                        eprintln!("Login cleanup failed: {}", crate::format_lib_error(&error));
                    }
                }
                print_json(&result?)
            }
            AwsCmd::Logout { id } => {
                find_source(&sources, &id)?;
                auth.logout(&id).await
            }
            AwsCmd::Status { id } => {
                find_source(&sources, &id)?;
                print_json(&auth.status(&id)?)
            }
            AwsCmd::Discover { id } => {
                let source = find_source(&sources, &id)?;
                print_json(&rssh_core::aws::discovery::discover(&auth, source).await?)
            }
            _ => unreachable!("source operations handled before authentication"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_commands_do_not_modify_legacy_cli_sources() {
        let sources = vec![DynamicDiscoverySource {
            id: "legacy".into(),
            name: "AWS CLI".into(),
            enabled: true,
            config: DynamicDiscoveryConfig::AwsSsm {
                profile: "default".into(),
                region: "us-east-1".into(),
                only_ssm_available: false,
            },
        }];
        assert_eq!(
            find_source(&sources, "legacy").unwrap_err().code(),
            "aws_source_not_found"
        );
    }
}

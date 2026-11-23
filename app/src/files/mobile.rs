//! The queue carries the authorized directory and the relative child name together.
//! A URI returned by a single-file picker never acquires sibling access.
use super::{directory_required, validate_relative_path, ResolvedPath, TransferDestination};
use crate::error::AppResult;

#[cfg(android)]
use super::android as provider;
#[cfg(ios)]
use super::ios as provider;

pub(crate) async fn pick_mobile_directory(
    app: &tauri::AppHandle,
    write: bool,
) -> AppResult<Option<String>> {
    if !write {
        return Err(directory_required());
    }
    provider::pick(app).await
}

pub(crate) fn resolve_mobile_targets(
    root: String,
    paths: Vec<String>,
) -> AppResult<Vec<ResolvedPath>> {
    Ok(paths
        .into_iter()
        .map(|relative| {
            let result = validate_relative_path(&relative).map(|()| {
                let mut url = url::Url::parse("rssh-directory:/").expect("constant URL");
                url.query_pairs_mut()
                    .append_pair("root", &root)
                    .append_pair("child", &relative);
                url.to_string()
            });
            ResolvedPath::new(relative, result)
        })
        .collect())
}

pub(super) async fn destination(
    app: &tauri::AppHandle,
    location: &str,
) -> AppResult<TransferDestination> {
    let url = url::Url::parse(location).map_err(|_| directory_required())?;
    if url.scheme() != "rssh-directory" {
        return Err(directory_required());
    }
    let mut query = url.query_pairs();
    let root = query
        .next()
        .filter(|(k, _)| k == "root")
        .ok_or_else(directory_required)?
        .1;
    let relative = query
        .next()
        .filter(|(k, _)| k == "child")
        .ok_or_else(directory_required)?
        .1;
    validate_relative_path(&relative)?;
    let (parent, name) = relative.rsplit_once('/').unwrap_or(("", &relative));
    let directory = provider::directory(app, &root, parent).await?;
    Ok((directory, name.to_owned()))
}

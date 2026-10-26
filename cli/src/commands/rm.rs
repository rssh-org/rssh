//! `rssh <profile|credential|forward> rm <name>` —— 带二次确认的删除。

use rssh_core::error::AppResult;
use rssh_core::secret::cred_secret_key;

use crate::ctx::CliCtx;
use crate::helpers::{confirm, find_id_by_name};

pub fn cmd_rm_profile(conn: &CliCtx, name: &str) -> AppResult<()> {
    let profiles = rssh_core::db::profile::list(conn)?;
    let id = find_id_by_name(&profiles, name, "Profile")?;
    if !confirm(&format!("Delete profile '{name}'?"), false) {
        return Ok(());
    }
    rssh_core::db::profile::delete(conn, &id)?;
    println!("Deleted.");
    Ok(())
}

pub fn cmd_rm_credential(conn: &CliCtx, name: &str) -> AppResult<()> {
    let creds = rssh_core::db::credential::list(conn)?;
    let id = find_id_by_name(&creds, name, "Credential")?;
    if !confirm(&format!("Delete credential '{name}'?"), false) {
        return Ok(());
    }
    rssh_core::db::credential::delete(conn, &id)?;
    if let Err(e) = conn.secret_store().delete(&cred_secret_key(&id)) {
        eprintln!(
            "warning: credential deleted, but secret cleanup failed: {}",
            e.code()
        );
    }
    println!("Deleted.");
    Ok(())
}

pub fn cmd_rm_forward(conn: &CliCtx, name: &str) -> AppResult<()> {
    let forwards = rssh_core::db::forward::list(conn)?;
    let id = find_id_by_name(&forwards, name, "Forward")?;
    if !confirm(&format!("Delete forward '{name}'?"), false) {
        return Ok(());
    }
    rssh_core::db::forward::delete(conn, &id)?;
    println!("Deleted.");
    Ok(())
}

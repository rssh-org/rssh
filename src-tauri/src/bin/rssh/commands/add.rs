//! `rssh <profile|credential|forward> add` —— 交互式新增。

use rssh_lib::error::{AppError, AppResult};
use rssh_lib::models::{Credential, CredentialType, Forward, ForwardRule, ForwardType, Profile};

use crate::ctx::CliCtx;
use crate::helpers::{
    confirm, menu_select, prompt, prompt_default, prompt_optional, read_multiline, read_password,
    upsert_cred_with_secrets,
};

pub fn cmd_add_profile(conn: &CliCtx) -> AppResult<()> {
    let name = prompt("Name: ");
    let host = prompt("Host: ");
    let port: u16 = prompt_default("Port", "22").parse().unwrap_or(22);

    let creds = rssh_lib::db::credential::list(conn)?;
    // credential_id 是必填：空 list / 用户选 0 都视为没填完整，直接 Err 退出。
    // 之前的 .unwrap_or_default() 会写空串入库，让后续 ssh_connect/open 撞 not_found。
    let credential_id = menu_select(
        "Credentials:",
        "Credential",
        &creds,
        "No credentials yet. Run 'rssh credential add' first.",
        |c| format!("{} ({})", c.name, c.username),
    )
    .map(|c| c.id.clone())
    .ok_or_else(|| {
        AppError::config(
            "cli_credential_required",
            serde_json::json!({
                "hint": "Profile must reference a credential. Pick one from the list, or run 'rssh credential add' first."
            }),
        )
    })?;

    let profiles = rssh_lib::db::profile::list(conn)?;
    let bastion_profile_id = menu_select("Bastion (optional):", "Bastion", &profiles, "", |p| {
        format!("{} ({})", p.name, p.host)
    })
    .map(|p| p.id.clone());

    let init_command = prompt_optional("Init command (optional): ");

    let groups = rssh_lib::db::group::list(conn)?;
    let group_id = menu_select("Group (optional):", "Group", &groups, "", |g| {
        g.name.clone()
    })
    .map(|g| g.id.clone());

    let p = Profile {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        host,
        port,
        credential_id,
        bastion_profile_id,
        init_command,
        group_id,
        algorithms: Default::default(),
    };
    rssh_lib::db::profile::insert(conn, &p)?;
    println!("Profile '{}' created.", p.name);
    Ok(())
}

pub fn cmd_add_credential(conn: &CliCtx) -> AppResult<()> {
    let name = prompt("Name: ");
    let username = prompt("Username: ");

    println!("Auth type:");
    println!("  1 - password");
    println!("  2 - key (PEM)");
    println!("  3 - SSH agent (use $SSH_AUTH_SOCK / Pageant)");
    println!("  4 - none");
    let choice = prompt_default("Type #", "1");
    let (credential_type, secret) = match choice.as_str() {
        "2" => {
            println!("Paste private key (end with empty line):");
            let key = read_multiline();
            (CredentialType::Key, Some(key))
        }
        "3" => (CredentialType::Agent, None),
        "4" => (CredentialType::None, None),
        _ => {
            let pw = read_password("Password: ");
            (CredentialType::Password, Some(pw))
        }
    };

    let save_to_remote = confirm("Sync secret to GitHub?", false);

    let c = Credential {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        username,
        credential_type,
        secret,
        save_to_remote,
    };
    upsert_cred_with_secrets(conn, &c)?;
    println!("Credential '{}' created.", c.name);
    Ok(())
}

pub fn cmd_add_forward(conn: &CliCtx) -> AppResult<()> {
    let name = prompt("Name: ");
    let mut rules = vec![prompt_forward_rule(None)];
    while confirm("Add another forwarding rule?", false) {
        rules.push(prompt_forward_rule(None));
    }

    let profiles = rssh_lib::db::profile::list(conn)?;
    if profiles.is_empty() {
        return Err(AppError::config("cli_no_profiles", serde_json::json!({})));
    }
    println!("Profile:");
    for (i, p) in profiles.iter().enumerate() {
        println!("  {} - {} ({})", i + 1, p.name, p.host);
    }
    // 必须真选中一项 —— wrapping_sub(0) 会绕成 usize::MAX，再 unwrap_or_default
    // 写入空 profile_id，DB 里残留孤儿 forward。直接 Err 让 main 输出错误码。
    let pidx = prompt("Profile #: ")
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .ok_or_else(|| AppError::config("cli_invalid_profile_index", serde_json::json!({})))?;
    let profile_id = profiles
        .get(pidx)
        .map(|p| p.id.clone())
        .ok_or_else(|| AppError::config("cli_invalid_profile_index", serde_json::json!({})))?;

    let f = Forward {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        profile_id,
        group_id: None,
        rules,
    };
    rssh_lib::db::forward::insert(conn, &f)?;
    println!("Forward '{}' created.", f.name);
    Ok(())
}

pub(crate) fn prompt_forward_rule(current: Option<&ForwardRule>) -> ForwardRule {
    println!("Type:");
    println!("  1 - local (-L)");
    println!("  2 - remote (-R)");
    println!("  3 - dynamic (-D, SOCKS5)");
    let default_type = match current.map(|rule| rule.forward_type) {
        Some(ForwardType::Remote) => "2",
        Some(ForwardType::Dynamic) => "3",
        _ => "1",
    };
    let forward_type = match prompt_default("Type #", default_type).as_str() {
        "2" => ForwardType::Remote,
        "3" => ForwardType::Dynamic,
        _ => ForwardType::Local,
    };
    let same_type = current.is_some_and(|rule| rule.forward_type == forward_type);
    let default_local_port = if same_type {
        current.unwrap().local_port.to_string()
    } else {
        match forward_type {
            ForwardType::Remote => "80".into(),
            ForwardType::Dynamic => "1080".into(),
            ForwardType::Local => "8080".into(),
        }
    };
    let local_port = prompt_port("Local port", &default_local_port);
    if forward_type == ForwardType::Dynamic {
        return ForwardRule {
            forward_type,
            local_port,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
        };
    }

    let default_host = current
        .map(|rule| rule.remote_host.as_str())
        .unwrap_or("127.0.0.1");
    let default_remote_port = if same_type {
        current.unwrap().remote_port.to_string()
    } else if forward_type == ForwardType::Remote {
        "8080".into()
    } else {
        "80".into()
    };
    ForwardRule {
        forward_type,
        local_port,
        remote_host: prompt_default("Remote host", default_host),
        remote_port: prompt_port("Remote port", &default_remote_port),
    }
}

fn prompt_port(label: &str, default: &str) -> u16 {
    loop {
        let value = prompt_default(label, default);
        match value.parse() {
            Ok(port) => return port,
            Err(_) => eprintln!("Invalid port '{value}'; enter a number from 0 to 65535."),
        }
    }
}

//! Migration v3：固定四厂商散键 → `ai_providers` 实体表。
//!
//! 触发：所有从 v3 之前版本（anthropic/openai/deepseek/glm 四固定 provider，
//! 配置散在 `ai_{name}_model` / `ai_{name}_endpoint` settings 键 +
//! `setting:ai_{name}_key` secret）升上来的用户。
//! Marker：`migration_v3_ai_provider_entities`（settings 表）。
//!
//! 迁移内容，对每个旧厂商名：
//!   1. model / endpoint / api_key **全空** → 视为从未配置，跳过（不生成空行）
//!   2. 否则建一行：
//!        id       = provider-{5位随机}（查重）
//!        name     = 固定显示名（Anthropic / OpenAI / DeepSeek / GLM）
//!        protocol = anthropic-messages / openai-completions / deepseek-thinking
//!                   / openai-completions（GLM 走 OpenAI 兼容协议，用户已确认）
//!        model    = 原值
//!        endpoint = 原值；空则填该厂商官方默认（endpoint 从此必填）
//!   3. api_key：`setting:ai_{旧名}_key` → `setting:ai_{id}_key`（set 新 + delete 旧）
//!   4. 删除旧 model/endpoint settings 键
//!   5. `ai_provider`（active）：旧值是四厂商名且该厂商有迁移行 → 指向新 id；
//!      否则删除（active 悬空比错误指向更安全）
//!
//! 表本身由 schema v28 创建（Db::open 内），本迁移只做数据搬移 —— 需要
//! SecretStore 访问，因此住在 startup migration 而非 schema.rs。

use crate::db::{self, Db};
use crate::error::AppResult;
use crate::secret::{setting_key, SecretStore};

const MIGRATION_MARKER: &str = "migration_v3_ai_provider_entities";

/// 旧厂商名 → (显示名, 协议, 官方默认 endpoint)。顺序即建行顺序。
const LEGACY: &[(&str, &str, &str, &str)] = &[
    (
        "anthropic",
        "Anthropic",
        "anthropic-messages",
        "https://api.anthropic.com/v1/messages",
    ),
    (
        "openai",
        "OpenAI",
        "openai-completions",
        "https://api.openai.com/v1",
    ),
    (
        "deepseek",
        "DeepSeek",
        "deepseek-thinking",
        "https://api.deepseek.com/v1",
    ),
    (
        "glm",
        "GLM",
        "openai-completions",
        "https://open.bigmodel.cn/api/paas/v4",
    ),
];

fn legacy_model_key(name: &str) -> String {
    format!("ai_{name}_model")
}

fn legacy_endpoint_key(name: &str) -> String {
    format!("ai_{name}_endpoint")
}

fn legacy_api_key(name: &str) -> String {
    setting_key(&format!("ai_{name}_key"))
}

pub fn run(db: &Db, store: &dyn SecretStore) -> AppResult<()> {
    if db::settings::get(db, MIGRATION_MARKER)?.is_some() {
        return Ok(());
    }

    let existing_ids = db::ai_provider::ids(db)?;
    let mut new_id_for: Vec<(&str, String)> = Vec::new();

    for (legacy_name, display, protocol, default_endpoint) in LEGACY {
        let model = db::settings::get(db, &legacy_model_key(legacy_name))?
            .unwrap_or_default()
            .trim()
            .to_string();
        let endpoint = db::settings::get(db, &legacy_endpoint_key(legacy_name))?
            .map(|e| e.trim().to_string())
            .unwrap_or_default();
        let api_key = store
            .get(&legacy_api_key(legacy_name))?
            .map(|k| k.trim().to_string())
            .unwrap_or_default();
        if model.is_empty() && endpoint.is_empty() && api_key.is_empty() {
            continue; // never-configured — no row, no residue
        }

        // provider-{5位}，撞库重抽（uuid 前 5 位碰撞概率极低，循环兜底为零）。
        let id = loop {
            let candidate = db::ai_provider::ProviderRow::generate_id();
            if !existing_ids.contains(&candidate) {
                break candidate;
            }
        };
        db::ai_provider::upsert(
            db,
            &db::ai_provider::ProviderRow {
                id: id.clone(),
                name: (*display).to_string(),
                protocol: (*protocol).to_string(),
                model,
                endpoint: if endpoint.is_empty() {
                    (*default_endpoint).to_string()
                } else {
                    endpoint
                },
            },
        )?;
        if !api_key.is_empty() {
            store.set(&setting_key(&format!("ai_{id}_key")), &api_key)?;
        }
        // Move-then-delete：新键写失败时旧数据仍在（幂等重跑也安全）。
        store.delete(&legacy_api_key(legacy_name))?;
        db::settings::delete(db, &legacy_model_key(legacy_name))?;
        db::settings::delete(db, &legacy_endpoint_key(legacy_name))?;
        new_id_for.push((legacy_name, id));
    }

    // active 选择跟着迁；旧值无对应行（未配置 / 悬空）→ 删除。
    let active = db::settings::get(db, "ai_provider")?;
    match active.as_deref() {
        Some(old) => match new_id_for.iter().find(|(name, _)| *name == old) {
            Some((_, new_id)) => db::settings::set(db, "ai_provider", new_id)?,
            None => db::settings::delete(db, "ai_provider")?,
        },
        None => {}
    }

    db::settings::set(db, MIGRATION_MARKER, "1")?;
    if !new_id_for.is_empty() {
        log::info!(
            "migration v3: {} legacy provider(s) became ai_providers rows",
            new_id_for.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-process SecretStore (mirrors sync::config tests).
    #[derive(Default)]
    struct MemStore {
        inner: Mutex<HashMap<String, String>>,
    }
    impl SecretStore for MemStore {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
        fn backend_name(&self) -> &'static str {
            "mem"
        }
    }

    fn fixture() -> (Db, MemStore) {
        (Db::open_in_memory().unwrap(), MemStore::default())
    }

    fn rows(db: &Db) -> Vec<db::ai_provider::ProviderRow> {
        db::ai_provider::list(db).unwrap()
    }

    #[test]
    fn configured_deepseek_becomes_a_row_with_default_endpoint() {
        let (db, ss) = fixture();
        db::settings::set(&db, "ai_deepseek_model", "deepseek-reasoner").unwrap();
        ss.set(&legacy_api_key("deepseek"), "sk-ds").unwrap();
        db::settings::set(&db, "ai_provider", "deepseek").unwrap();

        run(&db, &ss).unwrap();

        let rs = rows(&db);
        assert_eq!(rs.len(), 1);
        let r = &rs[0];
        assert!(r.id.starts_with("provider-") && r.id.len() == "provider-".len() + 5);
        assert_eq!(r.name, "DeepSeek");
        assert_eq!(r.protocol, "deepseek-thinking");
        assert_eq!(r.model, "deepseek-reasoner");
        assert_eq!(r.endpoint, "https://api.deepseek.com/v1");
        // key moved to the new id, active points at the new row
        assert_eq!(
            ss.get(&setting_key(&format!("ai_{}_key", r.id)))
                .unwrap()
                .as_deref(),
            Some("sk-ds")
        );
        assert!(ss.get(&legacy_api_key("deepseek")).unwrap().is_none());
        assert_eq!(
            db::settings::get(&db, "ai_provider").unwrap().as_deref(),
            Some(r.id.as_str())
        );
        // legacy keys gone
        assert!(db::settings::get(&db, "ai_deepseek_model")
            .unwrap()
            .is_none());
    }

    #[test]
    fn glm_maps_to_openai_completions_and_keeps_endpoint() {
        let (db, ss) = fixture();
        db::settings::set(&db, "ai_glm_model", "glm-4.6").unwrap();
        db::settings::set(&db, "ai_glm_endpoint", "https://proxy.example/v1").unwrap();
        ss.set(&legacy_api_key("glm"), "sk-glm").unwrap();

        run(&db, &ss).unwrap();

        let r = &rows(&db)[0];
        assert_eq!(r.protocol, "openai-completions");
        assert_eq!(r.endpoint, "https://proxy.example/v1"); // custom endpoint kept
    }

    #[test]
    fn never_configured_provider_makes_no_row_and_active_clears() {
        let (db, ss) = fixture();
        // Only anthropic configured; active points at never-configured glm.
        ss.set(&legacy_api_key("anthropic"), "sk-a").unwrap();
        db::settings::set(&db, "ai_provider", "glm").unwrap();

        run(&db, &ss).unwrap();

        let rs = rows(&db);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].name, "Anthropic");
        assert!(db::settings::get(&db, "ai_provider").unwrap().is_none());
    }

    #[test]
    fn marker_makes_it_idempotent() {
        let (db, ss) = fixture();
        db::settings::set(&db, "ai_openai_model", "gpt-4o").unwrap();
        run(&db, &ss).unwrap();
        let first = rows(&db);
        assert_eq!(first.len(), 1);
        run(&db, &ss).unwrap();
        assert_eq!(rows(&db).len(), 1, "second run is a no-op");
    }
}

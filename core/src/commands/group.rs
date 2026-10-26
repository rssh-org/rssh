use crate::error::AppError;
use crate::models::Group;
use crate::state::AppState;

pub fn list_groups(state: &AppState) -> Result<Vec<Group>, AppError> {
    crate::db::group::list(&state.db)
}

pub fn create_group(state: &AppState, group: Group) -> Result<(), AppError> {
    crate::db::group::insert(&state.db, &group)
}

pub fn update_group(state: &AppState, group: Group) -> Result<(), AppError> {
    crate::db::group::update(&state.db, &group)
}

pub fn delete_group(state: &AppState, id: String) -> Result<(), AppError> {
    crate::db::group::delete(&state.db, &id)
}

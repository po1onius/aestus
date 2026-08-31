mod credential;
mod model;
mod quota;
mod registration;
mod repository;

pub(crate) use credential::validate_registration_password;
pub use credential::{burn_dummy_password_verification, decode_jwt, issue_jwt, verify_password};
pub use model::{PublicUser, User, schema};
pub use quota::deduct_quota;
pub use registration::{
    bootstrap_admin, create_owner_managed_user, normalize_email, normalize_username,
    register_with_tenant_code, send_register_email_code, verify_register_email_code,
};
pub use repository::{
    find_by_email, find_by_id, find_by_login_identifier, list_by_tenant, list_usage_snapshots,
    update_quota_for_tenant, update_status,
};

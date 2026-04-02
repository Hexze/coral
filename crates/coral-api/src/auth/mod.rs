use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use coral_redis::RateLimitResult;
use database::{AccessRank, DeveloperKeyRepository, Member, MemberRepository};

use crate::{error::ApiError, state::AppState};

#[derive(Clone)]
pub struct AuthenticatedMember(pub Member);

#[derive(Clone)]
pub struct InternalAuth;

#[derive(Clone)]
pub struct DeveloperKeyAuth {
    pub permissions: i64,
}


impl DeveloperKeyAuth {
    pub fn has(&self, perm: i64) -> bool { self.permissions & perm != 0 }

    pub fn require(&self, perm: i64) -> Result<(), ApiError> {
        if self.has(perm) { Ok(()) } else { Err(ApiError::Forbidden("insufficient permissions".into())) }
    }
}


enum AuthResult {
    Personal(Member),
    Developer(Member, i64),
}


pub async fn require_internal_or_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let api_key = extract_api_key(&request).ok_or(StatusCode::UNAUTHORIZED)?;

    if is_internal_key(&state, &api_key) {
        request.extensions_mut().insert(InternalAuth);
        return Ok(next.run(request).await);
    }

    match authenticate_key(&state, &api_key).await? {
        AuthResult::Personal(member) => {
            if AccessRank::from_level(member.access_level) < AccessRank::Admin {
                return Err(StatusCode::FORBIDDEN);
            }
            request.extensions_mut().insert(AuthenticatedMember(member));
        }
        AuthResult::Developer(member, permissions) => {
            request.extensions_mut().insert(AuthenticatedMember(member));
            request.extensions_mut().insert(DeveloperKeyAuth { permissions });
        }
    }
    Ok(next.run(request).await)
}


pub async fn require_moderator(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let api_key = extract_api_key(&request).ok_or(StatusCode::UNAUTHORIZED)?;
    let member = authenticate_personal_key(&state, &api_key).await?;
    if AccessRank::from_level(member.access_level) < AccessRank::Moderator {
        return Err(StatusCode::FORBIDDEN);
    }
    request.extensions_mut().insert(AuthenticatedMember(member));
    Ok(next.run(request).await)
}


pub async fn allow_internal_or_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let api_key = extract_api_key(&request).ok_or(StatusCode::UNAUTHORIZED)?;

    if is_internal_key(&state, &api_key) {
        request.extensions_mut().insert(InternalAuth);
        return Ok(next.run(request).await);
    }

    match authenticate_key(&state, &api_key).await? {
        AuthResult::Personal(member) => {
            request.extensions_mut().insert(AuthenticatedMember(member));
        }
        AuthResult::Developer(member, permissions) => {
            request.extensions_mut().insert(AuthenticatedMember(member));
            request.extensions_mut().insert(DeveloperKeyAuth { permissions });
        }
    }
    Ok(next.run(request).await)
}


async fn authenticate_key(state: &AppState, api_key: &str) -> Result<AuthResult, StatusCode> {
    if let Some(member) = MemberRepository::new(state.db.pool())
        .get_by_api_key(api_key).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        if member.key_locked { return Err(StatusCode::FORBIDDEN); }
        check_rate_limit(state, api_key, PERSONAL_RATE_LIMIT).await?;
        return Ok(AuthResult::Personal(member));
    }

    if let Some(dev_key) = DeveloperKeyRepository::new(state.db.pool())
        .get_by_api_key(api_key).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        if dev_key.locked { return Err(StatusCode::FORBIDDEN); }
        let member = MemberRepository::new(state.db.pool())
            .get_by_id(dev_key.member_id).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        check_rate_limit(state, api_key, dev_key.rate_limit as i64).await?;
        return Ok(AuthResult::Developer(member, dev_key.permissions));
    }

    Err(StatusCode::UNAUTHORIZED)
}


async fn authenticate_personal_key(state: &AppState, api_key: &str) -> Result<Member, StatusCode> {
    let member = MemberRepository::new(state.db.pool())
        .get_by_api_key(api_key).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if member.key_locked { return Err(StatusCode::FORBIDDEN); }
    check_rate_limit(state, api_key, PERSONAL_RATE_LIMIT).await?;
    Ok(member)
}


async fn check_rate_limit(state: &AppState, api_key: &str, limit: i64) -> Result<(), StatusCode> {
    match state.rate_limiter.check_and_record(api_key, limit).await {
        Ok(RateLimitResult::Allowed { .. }) => Ok(()),
        Ok(RateLimitResult::Exceeded) => Err(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}


pub const PERSONAL_RATE_LIMIT: i64 = 600;


fn is_internal_key(state: &AppState, api_key: &str) -> bool {
    state.internal_api_key.as_ref().is_some_and(|k| k == api_key)
}


fn extract_api_key(request: &Request) -> Option<String> {
    if let Some(header) = request.headers().get("X-API-Key") {
        return header.to_str().ok().map(String::from);
    }
    request.uri().query().and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "key")
            .map(|(_, v)| v.into_owned())
    })
}

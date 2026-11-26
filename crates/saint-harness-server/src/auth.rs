use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

pub async fn bearer_auth_middleware(
    expected_token: Option<String>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = expected_token.filter(|t| !t.is_empty()) else {
        return Ok(next.run(req).await);
    };
    if req.uri().path() == "/health" {
        return Ok(next.run(req).await);
    }
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(token) if token == expected => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

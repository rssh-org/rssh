//! AWS control-plane requests signed by the official SigV4 implementation.
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use super::auth::AwsCredentials;
use crate::error::{AppError, AppResult};

pub(crate) fn failure(code: &'static str) -> AppError {
    AppError::other(code, json!({}))
}

pub(crate) fn invalid_response(stage: &'static str) -> AppError {
    // Fixed stage names identify the failed check without exposing AWS data.
    log::warn!("AWS response rejected: stage={stage}");
    AppError::other("aws_auth_response_rejected", json!({ "stage": stage }))
}

pub fn validate_region(region: &str) -> AppResult<()> {
    // Region is a single endpoint label, never a URL or user-controlled host.
    static REGION: OnceLock<regex::Regex> = OnceLock::new();
    let valid = REGION.get_or_init(|| regex::Regex::new(r"^[a-z]{2}(?:-[a-z]+)+-[0-9]+$").unwrap());
    if region.len() > 64 || !valid.is_match(region) {
        return Err(failure("aws_region_invalid"));
    }
    if region.starts_with("us-iso") || region.starts_with("eu-isoe") {
        return Err(failure("aws_region_invalid"));
    }
    Ok(())
}

pub fn endpoint(region: &str, service: &str) -> AppResult<String> {
    validate_region(region)?;
    if !matches!(service, "ec2" | "ssm" | "sts" | "kms" | "ssmmessages") {
        return Err(failure("aws_auth_request_failed"));
    }
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    Ok(format!("https://{service}.{region}.{suffix}/"))
}

pub(crate) fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("rssh/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| failure("aws_auth_request_failed"))
}

/// Return SigV4 additions (Authorization, date, security token). Callers retain
/// the supplied original headers. WSS upgrades are signed as HTTPS requests.
pub fn signed_headers(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> AppResult<Vec<(String, String)>> {
    signed_headers_at(
        credentials,
        region,
        service,
        method,
        url,
        headers,
        body,
        SystemTime::now(),
    )
}

#[allow(clippy::too_many_arguments)]
fn signed_headers_at(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    now: SystemTime,
) -> AppResult<Vec<(String, String)>> {
    validate_region(region)?;
    let url = url
        .strip_prefix("wss://")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| url.to_owned());
    let identity = aws_credential_types::Credentials::new(
        &credentials.access_key_id,
        &credentials.secret_access_key,
        Some(credentials.session_token.clone()),
        None,
        "rssh-aws-signin",
    )
    .into();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(service)
        .time(now)
        .settings(SigningSettings::default())
        .build()
        .map_err(|_| failure("aws_auth_request_failed"))?
        .into();
    let request = SignableRequest::new(
        method,
        &url,
        headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        SignableBody::Bytes(body),
    )
    .map_err(|_| failure("aws_auth_request_failed"))?;
    let (instructions, _) = sign(request, &params)
        .map_err(|_| failure("aws_auth_request_failed"))?
        .into_parts();
    Ok(instructions
        .headers()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect())
}

pub(crate) async fn response_bytes(
    response: reqwest::Response,
    limit: usize,
) -> AppResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(invalid_response("response_content_length_limit"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| failure("aws_auth_request_failed"))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(invalid_response("response_stream_size_limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn service_error_code(body: &[u8]) -> String {
    // Never expose arbitrary response bodies: some AWS errors echo inputs.
    let parsed = serde_json::from_slice::<Value>(body).ok();
    #[derive(Deserialize)]
    struct XmlError {
        #[serde(rename = "Code")]
        code: String,
    }
    #[derive(Deserialize)]
    struct XmlErrors {
        #[serde(rename = "Error")]
        error: XmlError,
    }
    #[derive(Deserialize)]
    struct XmlResponse {
        #[serde(rename = "Error")]
        error: Option<XmlError>,
        #[serde(rename = "Errors")]
        errors: Option<XmlErrors>,
    }
    let xml_code = std::str::from_utf8(body)
        .ok()
        .and_then(|text| quick_xml::de::from_str::<XmlResponse>(text).ok())
        .and_then(|response| {
            response
                .error
                .or(response.errors.map(|errors| errors.error))
        })
        .map(|error| error.code);
    parsed
        .as_ref()
        .and_then(|v| v.get("__type").or(v.get("code")).or(v.get("error")))
        .and_then(Value::as_str)
        .or(xml_code.as_deref())
        .and_then(|s| s.rsplit('#').next())
        .filter(|s| s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or("RequestFailed")
        .to_owned()
}

fn service_error(service: &str, status: u16, body: &[u8]) -> AppError {
    let code = service_error_code(body);
    AppError::other(
        "aws_api_failed",
        json!({"service":service,"status":status,"code":code}),
    )
}

async fn request(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    retryable: bool,
) -> AppResult<Vec<u8>> {
    let url = endpoint(region, service)?;
    request_at(credentials, region, service, &url, headers, body, retryable).await
}

#[allow(clippy::too_many_arguments)]
async fn request_at(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    retryable: bool,
) -> AppResult<Vec<u8>> {
    let http = client()?;
    let deadline = Instant::now() + Duration::from_secs(40);
    for attempt in 0..3 {
        let additions = signed_headers(credentials, region, service, "POST", url, &headers, &body)?;
        let mut request = http
            .post(url)
            .timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(30)),
            )
            .body(body.clone());
        for (key, value) in headers.iter().chain(additions.iter()) {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|secs| Duration::from_secs(secs.min(2)));
        let response_body = response_bytes(response, 16 * 1024 * 1024).await?;
        if status.is_success() {
            return Ok(response_body);
        }
        // AWS also returns HTTP 400 for these throttling codes. Keep other
        // client errors final, and preserve the caller's idempotency decision.
        let throttled = status.as_u16() == 400
            && matches!(
                service_error_code(&response_body).as_str(),
                "Throttling"
                    | "ThrottlingException"
                    | "ThrottledException"
                    | "RequestThrottled"
                    | "RequestThrottledException"
                    | "TooManyRequestsException"
                    | "RequestLimitExceeded"
                    | "EC2ThrottledException"
            );
        if retryable
            && attempt < 2
            && (status.as_u16() == 429 || status.is_server_error() || throttled)
        {
            let mut jitter = [0u8; 1];
            getrandom::getrandom(&mut jitter).map_err(|_| failure("aws_auth_request_failed"))?;
            let delay = retry_after.unwrap_or_else(|| {
                Duration::from_millis(200 * (1 << attempt) + u64::from(jitter[0]))
            });
            if Instant::now() + delay < deadline {
                tokio::time::sleep(delay).await;
                continue;
            }
        }
        return Err(service_error(service, status.as_u16(), &response_body));
    }
    Err(failure("aws_auth_request_failed"))
}

pub async fn json_request(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    target: &str,
    body: &Value,
) -> AppResult<Value> {
    let body = serde_json::to_vec(body).map_err(|_| failure("aws_auth_request_failed"))?;
    let action = target.rsplit('.').next().unwrap_or("");
    // StartSession has no idempotency key: a retry could leak a second session.
    let retryable = action.starts_with("Describe")
        || action.starts_with("Get")
        || matches!(action, "ResumeSession" | "TerminateSession");
    let bytes = request(
        credentials,
        region,
        service,
        vec![
            ("content-type".into(), "application/x-amz-json-1.1".into()),
            ("x-amz-target".into(), target.into()),
        ],
        body,
        retryable,
    )
    .await?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid_response("service_response_json"))
}

pub(crate) async fn query_request(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    pairs: &[(String, String)],
) -> AppResult<String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(k, v)| (k, v)))
        .finish();
    let bytes = request(
        credentials,
        region,
        service,
        vec![(
            "content-type".into(),
            "application/x-www-form-urlencoded; charset=utf-8".into(),
        )],
        body.into_bytes(),
        true,
    )
    .await?;
    String::from_utf8(bytes).map_err(|_| invalid_response("query_response_utf8"))
}

#[derive(Deserialize)]
struct IdentityResponse {
    #[serde(rename = "GetCallerIdentityResult")]
    result: CallerIdentity,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CallerIdentity {
    pub account: String,
    pub arn: String,
}

pub async fn caller_identity(
    credentials: &AwsCredentials,
    region: &str,
) -> AppResult<CallerIdentity> {
    let body = query_request(
        credentials,
        region,
        "sts",
        &[
            ("Action".into(), "GetCallerIdentity".into()),
            ("Version".into(), "2011-06-15".into()),
        ],
    )
    .await?;
    parse_caller_identity(&body)
}

#[cfg(test)]
pub(crate) async fn caller_identity_at(
    credentials: &AwsCredentials,
    region: &str,
    url: &str,
) -> AppResult<CallerIdentity> {
    let bytes = request_at(
        credentials,
        region,
        "sts",
        url,
        vec![(
            "content-type".into(),
            "application/x-www-form-urlencoded; charset=utf-8".into(),
        )],
        b"Action=GetCallerIdentity&Version=2011-06-15".to_vec(),
        true,
    )
    .await?;
    parse_caller_identity(
        std::str::from_utf8(&bytes).map_err(|_| invalid_response("sts_response_utf8"))?,
    )
}

fn parse_caller_identity(body: &str) -> AppResult<CallerIdentity> {
    let response: IdentityResponse =
        quick_xml::de::from_str(body).map_err(|_| invalid_response("sts_response_xml"))?;
    let result = response.result;
    if result.account.len() != 12
        || !result.account.bytes().all(|b| b.is_ascii_digit())
        || !result.arn.starts_with("arn:")
        || result.arn.split(':').nth(4) != Some(result.account.as_str())
    {
        return Err(invalid_response("sts_response_identity"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_credentials() -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: "test-session-token".into(),
            account_id: String::new(),
            arn: String::new(),
            expires_at: 0,
        }
    }

    #[test]
    fn regional_endpoints_reject_urls_and_unsupported_partitions() {
        for region in [
            "us-east-1.evil.test",
            "us-east-1/path",
            "https://aws",
            "us-iso-east-1",
            "x\n",
        ] {
            assert!(endpoint(region, "ssm").is_err());
        }
        assert_eq!(
            endpoint("cn-north-1", "ssm").unwrap(),
            "https://ssm.cn-north-1.amazonaws.com.cn/"
        );
        assert_eq!(
            endpoint("us-gov-west-1", "sts").unwrap(),
            "https://sts.us-gov-west-1.amazonaws.com/"
        );
    }

    #[test]
    fn caller_identity_requires_consistent_account_and_arn() {
        let xml = "<GetCallerIdentityResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><GetCallerIdentityResult><Arn>arn:aws:iam::123456789012:user/demo</Arn><Account>123456789012</Account></GetCallerIdentityResult></GetCallerIdentityResponse>";
        assert_eq!(parse_caller_identity(xml).unwrap().account, "123456789012");
        assert!(parse_caller_identity(
            &xml.replace("<Account>123456789012", "<Account>999999999999")
        )
        .is_err());
    }

    #[test]
    fn query_errors_expose_only_bounded_error_codes() {
        let error=service_error("ec2",403,b"<Response><Errors><Error><Code>UnauthorizedOperation</Code><Message>secret-do-not-display</Message></Error></Errors></Response>").to_string();
        assert!(error.contains("UnauthorizedOperation"));
        assert!(!error.contains("secret-do-not-display"));
        let error = service_error(
            "sts",
            403,
            b"<ErrorResponse><Error><Code>AccessDenied</Code></Error></ErrorResponse>",
        )
        .to_string();
        assert!(error.contains("AccessDenied"));
    }

    #[tokio::test]
    async fn http_400_throttling_retries_json_and_query_requests_until_recovery() {
        for (service, response) in [
            ("ssm", r#"{"__type":"com.amazonaws.ssm#ThrottlingException"}"#),
            (
                "ec2",
                "<Response><Errors><Error><Code>RequestLimitExceeded</Code></Error></Errors></Response>",
            ),
        ] {
            let mut server = mockito::Server::new_async().await;
            let throttled = server
                .mock("POST", "/")
                .with_status(400)
                .with_header("retry-after", "0")
                .with_body(response)
                .expect(1)
                .create_async()
                .await;
            let recovered = server
                .mock("POST", "/")
                .with_status(200)
                .with_body("recovered")
                .expect(1)
                .create_async()
                .await;
            let result = request_at(
                &test_credentials(),
                "us-east-1",
                service,
                &server.url(),
                vec![],
                b"request".to_vec(),
                true,
            )
            .await
            .unwrap();
            assert_eq!(result, b"recovered");
            throttled.assert_async().await;
            recovered.assert_async().await;
        }
    }

    #[tokio::test]
    async fn http_400_non_throttling_and_non_retryable_requests_are_not_retried() {
        for (code, retryable) in [
            ("ValidationException", true),
            ("AccessDeniedException", true),
            ("UnrecognizedClientException", true),
            ("ThrottlingException", false),
        ] {
            let mut server = mockito::Server::new_async().await;
            let rejected = server
                .mock("POST", "/")
                .with_status(400)
                .with_header("retry-after", "0")
                .with_body(json!({ "__type": code }).to_string())
                .expect(1)
                .create_async()
                .await;
            let error = request_at(
                &test_credentials(),
                "us-east-1",
                "ssm",
                &server.url(),
                vec![],
                b"request".to_vec(),
                retryable,
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), "aws_api_failed");
            assert!(error.to_string().contains(code));
            rejected.assert_async().await;
        }
    }

    #[tokio::test]
    async fn http_400_throttling_preserves_the_three_attempt_limit() {
        let mut server = mockito::Server::new_async().await;
        let throttled = server
            .mock("POST", "/")
            .with_status(400)
            .with_header("retry-after", "0")
            .with_body(r#"{"__type":"ThrottlingException"}"#)
            .expect(3)
            .create_async()
            .await;
        let error = request_at(
            &test_credentials(),
            "us-east-1",
            "ssm",
            &server.url(),
            vec![],
            b"request".to_vec(),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "aws_api_failed");
        throttled.assert_async().await;
    }

    #[test]
    fn sigv4_covers_session_token_and_payload_and_wss_uses_https() {
        let credentials = test_credentials();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1440938160);
        let sign = |url, body| {
            signed_headers_at(
                &credentials,
                "us-east-1",
                "ssmmessages",
                "GET",
                url,
                &[],
                body,
                now,
            )
            .unwrap()
        };
        let https = sign(
            "https://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/test",
            b"",
        );
        assert_eq!(
            https,
            sign(
                "wss://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/test",
                b""
            )
        );
        assert!(https
            .iter()
            .any(|(k, v)| k == "x-amz-security-token" && v == "test-session-token"));
        assert_ne!(
            https,
            sign(
                "https://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/test",
                b"changed"
            )
        );
    }
}

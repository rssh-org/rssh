//! EC2 Query API discovery. Results are transient launch targets, never rows.
use std::collections::HashSet;
use std::future::Future;

use serde::Deserialize;
use serde_json::{json, Value};

use super::api::{self, failure};
use super::auth::AwsAuth;
use crate::error::AppResult;
use crate::models::{
    ConnectorSpec, DynamicDiscoveredTarget, DynamicDiscoveryConfig, DynamicDiscoveryPlatform,
    DynamicDiscoverySource,
};

const MAX_PAGES: usize = 10_000;

#[derive(Deserialize, Default)]
struct Items<T> {
    #[serde(rename = "item", default)]
    items: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ec2Page {
    #[serde(default)]
    reservation_set: Items<Reservation>,
    next_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Reservation {
    owner_id: String,
    #[serde(default)]
    instances_set: Items<Instance>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Instance {
    instance_id: String,
    instance_state: InstanceState,
    private_ip_address: Option<String>,
    ip_address: Option<String>,
    #[serde(default)]
    tag_set: Items<Tag>,
}

#[derive(Deserialize, Default)]
struct InstanceState {
    name: String,
}

#[derive(Deserialize, Default)]
struct Tag {
    key: String,
    value: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SsmPage {
    #[serde(default)]
    instance_information_list: Vec<SsmInstance>,
    next_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SsmInstance {
    instance_id: String,
    ping_status: String,
}

fn next_page(token: Option<String>, seen: &mut HashSet<String>) -> AppResult<Option<String>> {
    let token = token.filter(|token| !token.is_empty());
    if let Some(token) = &token {
        if token.len() > 64 * 1024 || !seen.insert(token.clone()) || seen.len() >= MAX_PAGES {
            return Err(failure("aws_pagination_invalid"));
        }
    }
    Ok(token)
}

async fn ec2_pages<F, Fut>(request: F) -> AppResult<Vec<Reservation>>
where
    F: Fn(Vec<(String, String)>) -> Fut,
    Fut: Future<Output = AppResult<String>>,
{
    let mut seen = HashSet::new();
    let mut token = None;
    let mut rows = Vec::new();
    loop {
        let mut args = vec![
            ("Action".into(), "DescribeInstances".into()),
            ("Version".into(), "2016-11-15".into()),
            ("MaxResults".into(), "100".into()),
        ];
        if let Some(token) = token {
            args.push(("NextToken".into(), token));
        }
        let xml = request(args).await?;
        let page: Ec2Page =
            quick_xml::de::from_str(&xml).map_err(|_| failure("aws_auth_invalid_response"))?;
        rows.extend(page.reservation_set.items);
        token = next_page(page.next_token, &mut seen)?;
        if token.is_none() {
            return Ok(rows);
        }
    }
}

async fn ssm_online<F, Fut>(request: F) -> AppResult<HashSet<String>>
where
    F: Fn(Value) -> Fut,
    Fut: Future<Output = AppResult<Value>>,
{
    let mut seen = HashSet::new();
    let mut token = None;
    let mut ids = HashSet::new();
    loop {
        let mut body =
            json!({"MaxResults":50,"Filters":[{"Key":"PingStatus","Values":["Online"]}]});
        if let Some(token) = token {
            body["NextToken"] = Value::String(token);
        }
        let page: SsmPage = serde_json::from_value(request(body).await?)
            .map_err(|_| failure("aws_auth_invalid_response"))?;
        ids.extend(
            page.instance_information_list
                .into_iter()
                .filter(|row| row.ping_status == "Online")
                .map(|row| row.instance_id),
        );
        token = next_page(page.next_token, &mut seen)?;
        if token.is_none() {
            return Ok(ids);
        }
    }
}

pub async fn discover(
    auth: &AwsAuth,
    source: &DynamicDiscoverySource,
) -> AppResult<Vec<DynamicDiscoveredTarget>> {
    let DynamicDiscoveryConfig::AwsNative {
        region,
        only_ssm_available,
    } = &source.config
    else {
        return Err(failure("aws_source_invalid"));
    };
    api::validate_region(region)?;
    let credentials = auth.credentials(&source.id).await?;
    let rows = ec2_pages(|pairs| {
        let credentials = &credentials;
        async move { api::query_request(credentials, region, "ec2", &pairs).await }
    })
    .await?;
    let online = if *only_ssm_available {
        Some(
            ssm_online(|body| {
                let credentials = &credentials;
                async move {
                    api::json_request(
                        credentials,
                        region,
                        "ssm",
                        "AmazonSSM.DescribeInstanceInformation",
                        &body,
                    )
                    .await
                }
            })
            .await?,
        )
    } else {
        None
    };
    targets(
        source,
        region,
        &credentials.account_id,
        rows,
        online.as_ref(),
    )
}

fn targets(
    source: &DynamicDiscoverySource,
    region: &str,
    account_id: &str,
    rows: Vec<Reservation>,
    online: Option<&HashSet<String>>,
) -> AppResult<Vec<DynamicDiscoveredTarget>> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for reservation in rows {
        if reservation.owner_id != account_id {
            return Err(failure("aws_identity_mismatch"));
        }
        for instance in reservation.instances_set.items {
            let state = instance.instance_state.name;
            if matches!(state.as_str(), "terminated" | "shutting-down")
                || online
                    .is_some_and(|ids| state != "running" || !ids.contains(&instance.instance_id))
            {
                continue;
            }
            if !instance.instance_id.strip_prefix("i-").is_some_and(|s| {
                matches!(s.len(), 8 | 17) && s.bytes().all(|b| b.is_ascii_hexdigit())
            }) {
                return Err(failure("aws_auth_invalid_response"));
            }
            if !seen.insert(instance.instance_id.clone()) {
                continue;
            }
            let name = instance
                .tag_set
                .items
                .into_iter()
                .find(|tag| tag.key == "Name")
                .and_then(|tag| tag.value)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| instance.instance_id.clone());
            let mut details = vec![instance.instance_id.as_str(), state.as_str(), region];
            details.extend(instance.private_ip_address.as_deref());
            details.extend(instance.ip_address.as_deref());
            targets.push(DynamicDiscoveredTarget {
                id: format!(
                    "aws_native:{}:{account_id}:{region}:{}",
                    source.id, instance.instance_id
                ),
                source_id: source.id.clone(),
                source_name: source.name.clone(),
                platform: DynamicDiscoveryPlatform::AwsNative,
                name,
                sub: details.join(" · "),
                connector_spec: ConnectorSpec::AwsNative {
                    source_id: source.id.clone(),
                    region: region.into(),
                    instance_id: instance.instance_id,
                    account_id: account_id.into(),
                },
            });
        }
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const PAGE: &str = r#"<DescribeInstancesResponse xmlns="http://ec2.amazonaws.com/doc/2016-11-15/"><reservationSet><item><ownerId>123456789012</ownerId><instancesSet><item><instanceId>i-1234567890abcdef0</instanceId><instanceState><name>running</name></instanceState><privateIpAddress>10.0.0.1</privateIpAddress><tagSet><item><key>Name</key><value>dev &amp; staging</value></item></tagSet></item><item><instanceId>i-1234567890abcdef1</instanceId><instanceState><name>stopped</name></instanceState></item></instancesSet></item></reservationSet></DescribeInstancesResponse>"#;
    fn source() -> DynamicDiscoverySource {
        DynamicDiscoverySource {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Cloud".into(),
            enabled: true,
            config: DynamicDiscoveryConfig::AwsNative {
                region: "us-east-1".into(),
                only_ssm_available: false,
            },
        }
    }

    #[tokio::test]
    async fn ec2_follows_pagination_and_keeps_stopped_instances() {
        let calls = Mutex::new(Vec::new());
        let rows = ec2_pages(|args| {
            let mut calls = calls.lock().unwrap();
            calls.push(args);
            let page = if calls.len() == 1 {
                PAGE.replace(
                    "</DescribeInstancesResponse>",
                    "<nextToken>second&amp;page</nextToken></DescribeInstancesResponse>",
                )
            } else {
                "<DescribeInstancesResponse><reservationSet/></DescribeInstancesResponse>".into()
            };
            async move { Ok(page) }
        })
        .await
        .unwrap();
        assert!(calls.lock().unwrap()[1].contains(&("NextToken".into(), "second&page".into())));
        let targets = targets(&source(), "us-east-1", "123456789012", rows, None).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "dev & staging");
        assert!(targets[1].sub.contains("stopped"));
    }

    #[tokio::test]
    async fn repeated_page_token_fails_instead_of_looping_or_returning_partial_results() {
        let error=ec2_pages(|_|async {Ok("<DescribeInstancesResponse><nextToken>same</nextToken></DescribeInstancesResponse>".into())}).await.err().unwrap();
        assert_eq!(error.code(), "aws_pagination_invalid");
    }

    #[tokio::test]
    async fn ssm_missing_lists_are_empty_and_still_follow_the_next_token() {
        let calls = Mutex::new(0);
        let online = ssm_online(|body| {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            let response = match *calls {
                1 => {
                    assert!(body.get("NextToken").is_none());
                    json!({ "NextToken": "next" })
                }
                2 => {
                    assert_eq!(body["NextToken"], "next");
                    json!({
                        "InstanceInformationList": [{
                            "InstanceId": "i-1234567890abcdef0", "PingStatus": "Online"
                        }],
                        "NextToken": "last"
                    })
                }
                3 => {
                    assert_eq!(body["NextToken"], "last");
                    json!({})
                }
                _ => panic!("SSM pagination continued past an empty final page"),
            };
            async move { Ok(response) }
        })
        .await
        .unwrap();
        assert_eq!(*calls.lock().unwrap(), 3);
        assert_eq!(online, HashSet::from(["i-1234567890abcdef0".into()]));
    }

    #[tokio::test]
    async fn ssm_pages_are_joined_and_filter_targets_without_persisting_profiles() {
        let calls = Mutex::new(0);
        let online=ssm_online(|body| {let mut calls=calls.lock().unwrap();*calls+=1;assert_eq!(body["MaxResults"],50);let response=if *calls==1 {json!({"InstanceInformationList":[],"NextToken":"next"})}else{assert_eq!(body["NextToken"],"next");json!({"InstanceInformationList":[{"InstanceId":"i-1234567890abcdef0","PingStatus":"Online"}]})};async move {Ok(response)}}).await.unwrap();
        let page: Ec2Page = quick_xml::de::from_str(PAGE).unwrap();
        let source = source();
        let rows = targets(
            &source,
            "us-east-1",
            "123456789012",
            page.reservation_set.items,
            Some(&online),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        let spec = serde_json::to_value(&rows[0].connector_spec).unwrap();
        assert_eq!(spec["type"], "aws_native");
        assert_eq!(spec["source_id"], source.id);
        assert!(spec.get("profile").is_none());
    }

    #[test]
    fn account_mismatch_never_creates_a_connector() {
        let page: Ec2Page = quick_xml::de::from_str(PAGE).unwrap();
        assert_eq!(
            targets(
                &source(),
                "us-east-1",
                "999999999999",
                page.reservation_set.items,
                None
            )
            .unwrap_err()
            .code(),
            "aws_identity_mismatch"
        );
    }
}

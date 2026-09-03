use crate::vcp_modules::settings_manager::{Settings, SettingsState};
use tauri::{AppHandle, Manager};

#[derive(Clone)]
pub(crate) struct ConnectionSettings {
    pub(crate) profile_id: String,
    pub(crate) ws_url: String,
    pub(crate) http_url: String,
    pub(crate) token: String,
    pub(crate) prerender_enabled: bool,
}

struct SelectedSyncProfile<'a> {
    profile_id: &'a str,
    ws_url: &'a str,
    http_url: &'a str,
    token: &'a str,
}

pub(crate) async fn load_connection_settings(
    app: &AppHandle,
) -> Result<ConnectionSettings, String> {
    let state = app.state::<SettingsState>();
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), state)
        .await
        .map_err(|error| format!("无法读取同步配置: {error}"))?;
    resolve_connection_settings(&settings, cfg!(target_os = "android"))
}

pub(crate) fn resolve_connection_settings(
    settings: &Settings,
    is_android: bool,
) -> Result<ConnectionSettings, String> {
    let selected = select_sync_profile(settings)?;
    let resolved = ConnectionSettings {
        profile_id: selected.profile_id.to_string(),
        ws_url: authenticated_websocket_url(selected.ws_url, selected.token)?,
        http_url: normalized_http_url(selected.http_url)?,
        token: selected.token.trim().to_string(),
        prerender_enabled: settings.sync_prerender_enabled,
    };
    reject_mobile_loopback(&resolved, is_android)?;
    Ok(resolved)
}

fn select_sync_profile(settings: &Settings) -> Result<SelectedSyncProfile<'_>, String> {
    let profile_id = match settings.active_connection_profile_id.trim() {
        "" => "lan",
        value => value,
    };
    let matches = settings
        .connection_profiles
        .iter()
        .filter(|profile| profile.id.trim() == profile_id)
        .collect::<Vec<_>>();
    let selected = match matches.as_slice() {
        [profile] => SelectedSyncProfile {
            profile_id,
            ws_url: &profile.sync_server_url,
            http_url: &profile.sync_http_url,
            token: &profile.sync_token,
        },
        [] if settings.connection_profiles.is_empty() => legacy_profile(settings, profile_id),
        [] => return Err(format!("当前连接配置档不存在: {profile_id}")),
        _ => return Err(format!("当前连接配置档重复: {profile_id}")),
    };
    require_complete_profile(selected)
}

fn legacy_profile<'a>(settings: &'a Settings, profile_id: &'a str) -> SelectedSyncProfile<'a> {
    SelectedSyncProfile {
        profile_id,
        ws_url: &settings.sync_server_url,
        http_url: &settings.sync_http_url,
        token: &settings.sync_token,
    }
}

fn require_complete_profile(
    selected: SelectedSyncProfile<'_>,
) -> Result<SelectedSyncProfile<'_>, String> {
    if [selected.ws_url, selected.http_url, selected.token]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        Err("当前连接配置档的同步 URL 或令牌未配置".to_string())
    } else {
        Ok(selected)
    }
}

fn parse_endpoint(raw: &str, label: &str, schemes: &[&str]) -> Result<url::Url, String> {
    let endpoint =
        url::Url::parse(raw.trim()).map_err(|error| format!("{label}格式非法: {error}"))?;
    if !schemes.contains(&endpoint.scheme()) || endpoint.host().is_none() {
        return Err(format!(
            "{label}必须包含主机并使用 {}",
            schemes.join(" 或 ")
        ));
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err(format!("{label}不得包含用户凭据"));
    }
    Ok(endpoint)
}

fn authenticated_websocket_url(server_url: &str, token: &str) -> Result<String, String> {
    let mut url = parse_endpoint(server_url, "同步 WebSocket URL", &["ws", "wss"])?;
    if url.fragment().is_some() {
        return Err("同步 WebSocket URL 不得包含 fragment".to_string());
    }
    let retained = url
        .query_pairs()
        .filter(|(key, _)| key != "token")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    let mut query = url.query_pairs_mut();
    query.extend_pairs(retained);
    query.append_pair("token", token.trim());
    drop(query);
    Ok(url.to_string())
}

fn normalized_http_url(server_url: &str) -> Result<String, String> {
    let url = parse_endpoint(server_url, "同步 HTTP URL", &["http", "https"])?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err("同步 HTTP URL 不得包含 query 或 fragment".to_string());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn reject_mobile_loopback(settings: &ConnectionSettings, is_android: bool) -> Result<(), String> {
    if !is_android {
        return Ok(());
    }
    for raw in [&settings.ws_url, &settings.http_url] {
        let endpoint = url::Url::parse(raw).map_err(|error| error.to_string())?;
        if endpoint.host().is_some_and(is_loopback_host) {
            return Err("移动端同步地址不得指向 localhost 或回环 IP".to_string());
        }
    }
    Ok(())
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(value) => value.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(value) => value.is_loopback(),
        url::Host::Ipv6(value) => value.is_loopback(),
    }
}

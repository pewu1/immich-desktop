use std::path::Path;
use tauri::Url;

pub fn server_url(input: &str) -> Result<Url, String> {
    let mut url = Url::parse(input.trim()).map_err(|_| "Enter a valid server URL")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("Use HTTPS (HTTP is allowed only for loopback development)".into());
    }
    if url.host_str().is_none_or(|h| {
        h.chars()
            .any(|c| !c.is_ascii_alphanumeric() && !".-:[]".contains(c))
    }) || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "Use the server origin only, without a path, credentials, query or fragment".into(),
        );
    }
    url.set_path("/");
    Ok(url)
}

pub fn asset_id(input: &str) -> Result<String, String> {
    let id = uuid::Uuid::parse_str(input).map_err(|_| "Invalid asset ID")?;
    if id.get_version_num() != 4 || id.to_string() != input.to_ascii_lowercase() {
        return Err("Expected a canonical UUID v4 asset ID".into());
    }
    Ok(id.to_string())
}

/// Preserve valid names exactly. Reject unsafe/unsupported names rather than silently rename.
pub fn filename(name: &str) -> Result<&str, String> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 240
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(name, "." | "..")
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        || Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
    {
        return Err("Original filename is unsafe or unsupported on a target platform".into());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_policy_rejects_ambiguous_or_insecure_urls() {
        for url in [
            "http://photos.example",
            "https://user:secret@photos.example",
            "https://photos.example/path",
            "https://photos.example/?token=secret",
            "file:///tmp/a",
            "https://*.example",
        ] {
            assert!(server_url(url).is_err(), "{url}");
        }
        assert_eq!(
            server_url("https://Photos.example:443").unwrap().as_str(),
            "https://photos.example/"
        );
        assert!(server_url("http://127.0.0.1:2283").is_ok());
    }
    #[test]
    fn validates_asset_ids_and_cross_platform_names() {
        assert!(asset_id("11111111-1111-4111-8111-111111111111").is_ok());
        for id in [
            "../../secret",
            "11111111111141118111111111111111",
            "11111111-1111-1111-8111-111111111111",
        ] {
            assert!(asset_id(id).is_err());
        }
        for name in [
            "../a.jpg",
            "a\\b.png",
            ".",
            "CON.jpg",
            "LPT1.png",
            "trailing. ",
            "a:stream",
            "a\0b",
        ] {
            assert!(filename(name).is_err(), "{name}");
        }
        assert_eq!(filename("Zażółć 東京.HEIC").unwrap(), "Zażółć 東京.HEIC");
    }
}

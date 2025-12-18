//! Qobuz app credentials scraping.
//!
//! Extracts app_id and app_secret from the Qobuz web player bundles.

use base64::{Engine, engine::general_purpose::URL_SAFE};
use reqwest::Client;
use std::collections::HashMap;
use tracing::{debug, instrument, trace};

use crate::{Error, Result};

const LOGIN_URL: &str = "https://play.qobuz.com/login";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/124 Safari/537.36";

/// Scraped Qobuz app credentials.
#[derive(Debug, Clone, Default)]
pub struct AppCredentials {
    pub app_id: String,
    pub app_secret: String,
}

/// Fetch app credentials from Qobuz web player.
#[instrument(skip_all)]
pub async fn fetch_app_credentials() -> Result<AppCredentials> {
    let client = Client::builder().user_agent(USER_AGENT).build()?;

    // Fetch login page
    debug!("Fetching {}", LOGIN_URL);
    let html = client.get(LOGIN_URL).send().await?.text().await?;
    debug!(bytes = html.len(), "Got HTML");

    // Extract script URLs
    let script_urls = extract_script_urls(&html);
    debug!(count = script_urls.len(), "Found script URLs");
    for url in &script_urls {
        trace!(url, "Script URL");
    }

    let mut app_id = String::new();
    let mut seeds: HashMap<String, String> = HashMap::new();
    let mut secrets: HashMap<String, String> = HashMap::new();

    // Scan each bundle
    for url in &script_urls {
        if !url.contains("play.qobuz.com") {
            trace!(url, "Skipping non-qobuz URL");
            continue;
        }

        debug!(url, "Fetching bundle");

        let bundle = match client.get(url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(text) => text,
                Err(e) => {
                    debug!(error = %e, "Failed to read response");
                    continue;
                }
            },
            Err(e) => {
                debug!(error = %e, "Failed to fetch");
                continue;
            }
        };

        trace!(bytes = bundle.len(), "Got bundle");

        // Scan for app_id
        if app_id.is_empty()
            && let Some(id) = scan_app_id(&bundle)
        {
            debug!(app_id = %id, "Found app_id");
            app_id = id;
        }

        // Scan for seeds
        let seeds_before = seeds.len();
        scan_seeds(&bundle, &mut seeds);
        if seeds.len() > seeds_before {
            debug!(
                new_seeds = seeds.len() - seeds_before,
                timezones = ?seeds.keys().collect::<Vec<_>>(),
                "Found seeds"
            );
        }

        // Derive secrets from seeds + info + extras
        let secrets_before = secrets.len();
        scan_and_derive_secrets(&bundle, &seeds, &mut secrets);
        if secrets.len() > secrets_before {
            debug!(
                new_secrets = secrets.len() - secrets_before,
                "Derived secrets"
            );
        }

        // Early exit if we have what we need
        if !app_id.is_empty() && !secrets.is_empty() {
            debug!("Got all credentials");
            break;
        }
    }

    debug!(
        app_id = %app_id,
        seeds = ?seeds.keys().collect::<Vec<_>>(),
        secrets_count = secrets.len(),
        "Final results"
    );

    // Pick any secret (they're timezone-based but all work)
    let app_secret = secrets
        .values()
        .next()
        .cloned()
        .ok_or_else(|| Error::InvalidResponse("no app secret found".into()))?;

    if app_id.is_empty() {
        return Err(Error::InvalidResponse("no app id found".into()));
    }

    Ok(AppCredentials { app_id, app_secret })
}

/// Extract script src URLs from HTML.
fn extract_script_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();

    // Find <script src="...">
    let mut pos = 0;
    while let Some(script_pos) = html[pos..].find("<script") {
        let start = pos + script_pos;
        let Some(tag_end) = html[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + tag_end];

        if let Some(src_pos) = tag.find("src=") {
            let src_start = src_pos + 4;
            if src_start < tag.len() {
                let quote = tag.as_bytes()[src_start];
                if quote == b'"' || quote == b'\'' {
                    let src_content_start = src_start + 1;
                    if let Some(src_end) = tag[src_content_start..].find(quote as char) {
                        let url = &tag[src_content_start..src_content_start + src_end];
                        if url.contains(".js") {
                            urls.push(absolutize(url));
                        }
                    }
                }
            }
        }

        pos = start + tag_end + 1;
    }

    // Find <link rel="preload" as="script" href="...">
    pos = 0;
    while let Some(link_pos) = html[pos..].find("<link") {
        let start = pos + link_pos;
        let Some(tag_end) = html[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + tag_end];
        let tag_lower = tag.to_lowercase();

        if tag_lower.contains("rel=\"preload\"")
            && tag_lower.contains("as=\"script\"")
            && let Some(href_pos) = tag.find("href=")
        {
            let href_start = href_pos + 5;
            if href_start < tag.len() {
                let quote = tag.as_bytes()[href_start];
                if quote == b'"' || quote == b'\'' {
                    let href_content_start = href_start + 1;
                    if let Some(href_end) = tag[href_content_start..].find(quote as char) {
                        let url = &tag[href_content_start..href_content_start + href_end];
                        if url.contains(".js") {
                            urls.push(absolutize(url));
                        }
                    }
                }
            }
        }

        pos = start + tag_end + 1;
    }

    urls.sort();
    urls.dedup();
    urls
}

/// Make relative URLs absolute.
fn absolutize(url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("https://play.qobuz.com{url}")
    } else {
        format!("https://play.qobuz.com/{url}")
    }
}

/// Scan for app_id pattern: production:{api:{appId:"XXXXXXXXX"
fn scan_app_id(bundle: &str) -> Option<String> {
    let needle = "production:{api:{appId:\"";
    let pos = bundle.find(needle)?;
    let start = pos + needle.len();

    let mut id = String::new();
    for c in bundle[start..].chars().take(9) {
        if c.is_ascii_digit() {
            id.push(c);
        } else {
            break;
        }
    }

    if id.len() == 9 { Some(id) } else { None }
}

/// Scan for seed patterns: .initialSeed("SEED",window.utimezone.<tz>)
fn scan_seeds(bundle: &str, seeds: &mut HashMap<String, String>) {
    let needle = ".initialSeed(\"";
    let mut pos = 0;

    while let Some(found) = bundle[pos..].find(needle) {
        let start = pos + found + needle.len();

        // Find end of seed string
        let Some(quote_end) = bundle[start..].find('"') else {
            break;
        };
        let seed = &bundle[start..start + quote_end];

        // Find timezone
        let tz_needle = ",window.utimezone.";
        let Some(tz_pos) = bundle[start + quote_end..].find(tz_needle) else {
            pos = start + quote_end;
            continue;
        };
        let tz_start = start + quote_end + tz_pos + tz_needle.len();

        // Extract timezone name (alphabetic chars)
        let tz: String = bundle[tz_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();

        if !tz.is_empty() && !seed.is_empty() {
            // Capitalize first letter
            let mut tz_cap = tz.to_lowercase();
            if let Some(first) = tz_cap.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            seeds.insert(tz_cap, seed.to_string());
        }

        pos = tz_start;
    }
}

/// Derive secrets from seeds + info + extras patterns.
fn scan_and_derive_secrets(
    bundle: &str,
    seeds: &HashMap<String, String>,
    secrets: &mut HashMap<String, String>,
) {
    for (tz, seed) in seeds {
        // Look for /<Timezone> anchor then info:"...",extras:"..."
        let anchor = format!("/{tz}");
        let mut pos = 0;
        let mut anchor_count = 0;

        trace!(tz, anchor, "Searching for timezone anchor");

        while let Some(found) = bundle[pos..].find(&anchor) {
            anchor_count += 1;
            let scan_start = pos + found;

            // Look for info: and extras: nearby
            let search_window = &bundle[scan_start..std::cmp::min(scan_start + 500, bundle.len())];

            let info = extract_quoted_value(search_window, "info:");
            let extras = extract_quoted_value(search_window, "extras:");

            trace!(
                tz,
                anchor_count,
                info_found = info.is_some(),
                extras_found = extras.is_some(),
                "Found anchor"
            );

            if let (Some(info), Some(extras)) = (&info, &extras) {
                // Derive secret: base64url_decode(seed + info + extras)[:-44]
                let combined = format!("{seed}{info}{extras}");
                trace!(
                    tz,
                    combined = combined,
                    combined_len = combined.len(),
                    seed_len = seed.len(),
                    info_len = info.len(),
                    extras_len = extras.len(),
                    "Attempting to derive secret"
                );

                if combined.len() > 44 {
                    let encoded = &combined[..combined.len() - 44];
                    trace!(tz, encoded = encoded,);
                    match URL_SAFE.decode(encoded) {
                        Ok(decoded) => match String::from_utf8(decoded) {
                            Ok(secret) => {
                                debug!(
                                    tz,
                                    secret_len = secret.len(),
                                    "Successfully derived secret"
                                );
                                secrets.insert(tz.clone(), secret);
                                break;
                            }
                            Err(e) => {
                                trace!(tz, error = %e, "UTF-8 decode failed");
                            }
                        },
                        Err(e) => {
                            trace!(tz, error = %e, encoded_len = encoded.len(), "Base64 decode failed");
                        }
                    }
                } else {
                    trace!(
                        tz,
                        combined_len = combined.len(),
                        "Combined too short (need >44)"
                    );
                }
            }

            pos = scan_start + anchor.len();
        }

        if anchor_count == 0 {
            trace!(tz, "No anchors found for timezone");
        }
    }
}

/// Extract a quoted value after a key like `info:"value"` or `info:'value'`.
fn extract_quoted_value(text: &str, key: &str) -> Option<String> {
    let key_pos = text.find(key)?;
    let after_key = key_pos + key.len();

    // Skip optional space
    let mut start = after_key;
    while start < text.len() && text.as_bytes()[start] == b' ' {
        start += 1;
    }

    if start >= text.len() {
        return None;
    }

    let quote = text.as_bytes()[start];
    if quote != b'"' && quote != b'\'' {
        return None;
    }

    let content_start = start + 1;
    let end = text[content_start..].find(quote as char)?;
    Some(text[content_start..content_start + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_app_id() {
        let bundle = r#"foo production:{api:{appId:"123456789" bar"#;
        assert_eq!(scan_app_id(bundle), Some("123456789".to_string()));
    }

    #[test]
    fn test_scan_seeds() {
        let bundle = r#"x.initialSeed("abc123",window.utimezone.europe)"#;
        let mut seeds = HashMap::new();
        scan_seeds(bundle, &mut seeds);
        assert_eq!(seeds.get("Europe"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_extract_quoted_value() {
        assert_eq!(
            extract_quoted_value(r#"info:"hello""#, "info:"),
            Some("hello".to_string())
        );
        assert_eq!(
            extract_quoted_value(r#"info: "world""#, "info:"),
            Some("world".to_string())
        );
    }
}

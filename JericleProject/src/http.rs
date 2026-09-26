//! Shared HTTP client. Yahoo's public endpoints reject requests without a browser
//! user agent, so one is always attached.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

pub fn client(timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .context("building http client")
}

pub async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("GET {url} -> HTTP {status}");
    }
    let text = resp.text().await.with_context(|| format!("reading body of {url}"))?;
    Ok(text)
}

pub async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T> {
    let text = get_text(client, url).await?;
    serde_json::from_str(&text).with_context(|| format!("decoding json from {url}"))
}

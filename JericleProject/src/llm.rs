//! Optional local-LLM rerank of the top headlines.
//!
//! Disabled by default. If the endpoint is unreachable, times out, or returns
//! something unparseable, the existing ranking is returned untouched — this step
//! is a bonus, never a dependency.

use anyhow::Result;
use serde::Deserialize;
use std::time::Duration;

use crate::config::LlmConfig;
use crate::model::NewsItem;

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize)]
struct Verdict {
    id: String,
    salience: Option<f64>,
    sentiment: Option<f64>,
}

/// Rerank in place-ish: returns a new vector ordered by the model's salience.
pub async fn rerank(client: &reqwest::Client, cfg: &LlmConfig, items: &[NewsItem]) -> Vec<NewsItem> {
    if !cfg.enabled || items.is_empty() {
        return items.to_vec();
    }
    let Ok(verdicts) = call(client, cfg, items).await else {
        return items.to_vec();
    };
    if verdicts.is_empty() {
        return items.to_vec();
    }

    let by_id: std::collections::HashMap<String, Verdict> =
        verdicts.into_iter().map(|v| (v.id.clone(), v)).collect();

    let mut out: Vec<NewsItem> = items
        .iter()
        .map(|item| {
            let mut cloned = item.clone();
            if let Some(v) = by_id.get(&item.id) {
                // Blend rather than replace, so one bad model response cannot dominate.
                if let Some(s) = v.salience {
                    cloned.salience = (item.salience * 0.5 + s.clamp(0.0, 100.0) * 0.5).min(100.0);
                }
                if let Some(s) = v.sentiment {
                    cloned.sentiment = (item.sentiment * 0.5 + s.clamp(-1.0, 1.0) * 0.5).clamp(-1.0, 1.0);
                }
            }
            cloned
        })
        .collect();

    out.sort_by(|a, b| {
        b.salience
            .partial_cmp(&a.salience)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

async fn call(client: &reqwest::Client, cfg: &LlmConfig, items: &[NewsItem]) -> Result<Vec<Verdict>> {
    let listing: Vec<String> = items
        .iter()
        .take(25)
        .map(|n| format!("{{\"id\":\"{}\",\"headline\":\"{}\"}}", n.id, n.title))
        .collect();

    let prompt = format!(
        "You rank US market news by likely price impact. For each item output a JSON object \
         with id, salience (0-100) and sentiment (-1 to 1, positive is bullish). \
         Reply with a JSON array only, no prose.\n\n{}",
        listing.join("\n")
    );

    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": "You are a precise financial news ranking engine. Output JSON only."},
            {"role": "user", "content": prompt},
        ],
        "temperature": 0.1,
        "stream": false,
    });

    let resp = client
        .post(format!("{}/v1/chat/completions", cfg.url.trim_end_matches('/')))
        .timeout(Duration::from_millis(cfg.timeout_ms))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json::<ChatResponse>()
        .await?;

    let content = resp.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();
    let json = extract_json_array(&content).ok_or_else(|| anyhow::anyhow!("no json array in llm reply"))?;
    Ok(serde_json::from_str::<Vec<Verdict>>(&json)?)
}

/// Models sometimes wrap JSON in prose or a code fence.
fn extract_json_array(text: &str) -> Option<String> {
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            if end > start {
                return Some(text[start..=end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_array_from_prose() {
        let s = "Sure! Here you go:\n```json\n[{\"id\":\"a\",\"salience\":80}]\n```\nHope that helps.";
        let arr = extract_json_array(s).unwrap();
        let v: Vec<Verdict> = serde_json::from_str(&arr).unwrap();
        assert_eq!(v[0].salience, Some(80.0));
    }
}

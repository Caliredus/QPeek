//! Her tazelemede bağlı hesapların limitlerini `%LOCALAPPDATA%\QPeek\limits.json` dosyasına yazar.
//! Amaç: ajanlar ve betikler limitleri kendileri sorgulamadan, QPeek'in son okumasından alabilsin.
//! Dosya önce geçici dosyaya yazılır, sonra yerine taşınır; okuyan taraf yarım dosya görmez.

use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::backend::Card;

/// Dosya biçimi değişirse artırılır.
pub const SCHEMA: u32 = 1;

pub fn path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("QPeek").join("limits.json"))
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Tazeleme sonucunu dosya içeriğine çevirir. Tüm çekim başarısızsa `error` dolar, `accounts` boş kalır.
pub fn to_json(result: &Result<Vec<Card>, String>, now: DateTime<Utc>, refresh_every_secs: u64) -> Value {
    let (accounts, error) = match result {
        Ok(cards) => (cards.iter().map(card_json).collect::<Vec<_>>(), Value::Null),
        Err(e) => (Vec::new(), Value::String(e.clone())),
    };
    json!({
        "schema": SCHEMA,
        "app": concat!("QPeek ", env!("CARGO_PKG_VERSION")),
        "generated_at": ts(now),
        "refresh_every_secs": refresh_every_secs,
        "error": error,
        "accounts": accounts,
    })
}

fn card_json(c: &Card) -> Value {
    let error = if c.failed {
        c.metrics.first().map(|m| Value::String(m.label_en.clone())).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let windows: Vec<Value> = if c.failed {
        Vec::new()
    } else {
        c.bars()
            .map(|m| {
                json!({
                    "label": m.label_en,
                    "window_minutes": m.window_minutes,
                    "used_percent": round1(m.percent),
                    "remaining_percent": round1(100.0 - m.percent),
                    "resets_at": m.resets_at.map(ts),
                })
            })
            .collect()
    };
    let notes: Vec<Value> = if c.failed {
        Vec::new()
    } else {
        c.metrics
            .iter()
            .filter(|m| m.info)
            .map(|m| json!({ "label": m.label_en, "text": m.detail_en }))
            .collect()
    };
    json!({
        "provider": c.id,
        "name": c.name,
        "email": if c.email.is_empty() { Value::Null } else { Value::String(c.email.clone()) },
        "account": c.account_key,
        "plan": if c.plan.is_empty() { Value::Null } else { Value::String(c.plan.clone()) },
        "ok": !c.failed,
        "error": error,
        "windows": windows,
        "notes": notes,
    })
}

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// Dosyayı yazar; hata QPeek'i durdurmaz, yalnız döndürülür.
pub fn write(result: &Result<Vec<Card>, String>, refresh_every_secs: u64) -> Result<(), String> {
    let path = path().ok_or("LOCALAPPDATA yok")?;
    let dir = path.parent().ok_or("geçersiz yol")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&to_json(result, Utc::now(), refresh_every_secs))
        .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Metric;

    fn metric(label: &str, pct: f32, minutes: Option<u64>, info: bool) -> Metric {
        Metric {
            label_tr: label.into(),
            label_en: label.into(),
            percent: pct,
            detail_tr: "ayrıntı".into(),
            detail_en: "detail".into(),
            pace_tr: String::new(),
            pace_en: String::new(),
            pace_level: 0,
            info,
            window_minutes: minutes,
            resets_at: minutes.map(|_| "2026-11-28T05:00:00Z".parse().unwrap()),
        }
    }

    fn card(id: &str, email: &str, key: &str, failed: bool, metrics: Vec<Metric>) -> Card {
        Card {
            id: id.into(),
            name: id.into(),
            email: email.into(),
            account_key: key.into(),
            plan: if failed { String::new() } else { "ChatGPT Plus".into() },
            metrics,
            failed,
        }
    }

    #[test]
    fn writes_accounts_windows_and_errors() {
        let now: DateTime<Utc> = "2026-11-28T02:30:00Z".parse().unwrap();
        let cards = vec![
            card(
                "codex",
                "a@example.com",
                "default",
                false,
                vec![
                    metric("Session", 12.34, Some(300), false),
                    metric("Weekly", 40.0, Some(10080), false),
                    metric("Reset credits", 0.0, None, true),
                ],
            ),
            card("codex", "", "extra-1", true, vec![metric("Network error", 0.0, None, true)]),
        ];
        let v = to_json(&Ok(cards), now, 300);
        assert_eq!(v["schema"], 1);
        assert_eq!(v["generated_at"], "2026-11-28T02:30:00Z");
        assert_eq!(v["error"], Value::Null);
        let a = &v["accounts"][0];
        assert_eq!(a["email"], "a@example.com");
        assert_eq!(a["ok"], true);
        assert_eq!(a["windows"][0]["window_minutes"], 300);
        assert_eq!(a["windows"][0]["used_percent"], 12.3);
        assert_eq!(a["windows"][0]["remaining_percent"], 87.7);
        assert_eq!(a["windows"][0]["resets_at"], "2026-11-28T05:00:00Z");
        assert_eq!(a["windows"].as_array().unwrap().len(), 2);
        assert_eq!(a["notes"][0]["text"], "detail");
        let b = &v["accounts"][1];
        assert_eq!(b["ok"], false);
        assert_eq!(b["email"], Value::Null);
        assert_eq!(b["error"], "Network error");
        assert!(b["windows"].as_array().unwrap().is_empty());
    }

    #[test]
    fn whole_fetch_failure_is_reported() {
        let now: DateTime<Utc> = "2026-11-28T02:30:00Z".parse().unwrap();
        let v = to_json(&Err("core missing".into()), now, 300);
        assert_eq!(v["error"], "core missing");
        assert!(v["accounts"].as_array().unwrap().is_empty());
    }
}
